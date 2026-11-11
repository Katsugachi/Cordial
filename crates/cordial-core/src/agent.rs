//! The agentic loop.
//!
//! One user turn is: render the prompt, generate, parse tool calls, get
//! permission, run the tools, repeat until the model answers or the effort
//! budget runs out. Cancellation, context overflow and empty answers are handled
//! here so the UI only ever sees clean events.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use cordial_llama::SamplingParams;
use tokio::sync::oneshot;

use crate::backend::{Backend, GenStop, GenerateOutput, GenerateRequest};
use crate::error::CoreError;
use crate::events::{AgentEvent, EventSink, StopReason};
use crate::permissions::{Permission, PermissionPolicy};
use crate::prompt::{self, PromptOptions, DEFAULT_MEDIA_MARKER};
use crate::protocol::parse_reply;
use crate::session::{
    Attachment, AttachmentKind, Message, PlanStatus, PlanStep, Role, Session, SessionStore,
    ToolCall, ToolResultStatus,
};
use crate::settings::{Effort, PermissionMode, Settings};
use crate::tools::{self, ToolRuntime, ToolSpec};
use crate::util::{now_ms, truncate_chars};

/// How long an approval prompt may stay open before the call is denied.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub effort: Effort,
    pub permission_mode: PermissionMode,
    pub web_enabled: bool,
    pub memory: String,
    pub skill: String,
    pub thinking: bool,
    pub max_output_tokens: u32,
    pub max_tool_rounds: u32,
    pub max_tool_result_chars: usize,
    pub keep_image_turns: usize,
    pub sampling: SamplingParams,
}

impl AgentConfig {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            effort: settings.effort,
            permission_mode: settings.permission_mode,
            web_enabled: settings.web_enabled,
            memory: truncate_chars(&settings.memory, 4000),
            skill: truncate_chars(&settings.skill, 4000),
            // Low effort skips reasoning: it is the fastest, coolest mode, and
            // short factual answers do not need a thinking block.
            thinking: settings.effort != Effort::Low,
            max_output_tokens: settings.effort.max_output_tokens(),
            max_tool_rounds: settings.effort.max_tool_rounds(),
            max_tool_result_chars: settings.effort.fetch_chars(),
            keep_image_turns: 2,
            sampling: SamplingParams {
                temperature: settings.effort.temperature(),
                ..SamplingParams::default()
            },
        }
    }
}

// Treat fragments as the same fetched resource, while preserving query strings
// (which may identify genuinely different pages).
fn fetch_key(raw: &str) -> String {
    let raw = raw.trim();
    match url::Url::parse(raw) {
        Ok(mut url) => {
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => raw.to_string(),
    }
}

// A bare search-argument object is ambiguous, so never execute it directly.
// Only use this signal to ask the model to regenerate an explicit tool call.
fn looks_like_search_arguments(content: &str, user_text: &str) -> bool {
    let request = user_text.to_ascii_lowercase();
    if request.contains("json") || request.contains("code") || request.contains("object") {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content.trim()) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    object
        .get("query")
        .and_then(|v| v.as_str())
        .is_some_and(|query| !query.trim().is_empty())
        && object
            .keys()
            .all(|key| matches!(key.as_str(), "query" | "top_n" | "limit" | "num_results"))
}

/// Resolves "may this tool run?" questions raised by the agent.
#[derive(Debug, Clone, Default)]
pub struct ApprovalGate {
    pending: Arc<tokio::sync::Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}

impl ApprovalGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers interest in a decision. The receiver resolves to `true` when
    /// the user approves.
    pub async fn register(&self, call_id: &str) -> oneshot::Receiver<bool> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(call_id.to_string(), sender);
        receiver
    }

    /// Answers a pending request. Returns false when nothing was waiting.
    pub async fn resolve(&self, call_id: &str, allow: bool) -> bool {
        match self.pending.lock().await.remove(call_id) {
            Some(sender) => sender.send(allow).is_ok(),
            None => false,
        }
    }

    /// Drops every pending request, denying them.
    pub async fn deny_all(&self) {
        let mut guard = self.pending.lock().await;
        for (_, sender) in guard.drain() {
            let _ = sender.send(false);
        }
    }

    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

#[derive(Debug, Clone)]
pub struct TurnResult {
    pub stop: StopReason,
    pub rounds: u32,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub tokens_per_second: f32,
}

/// Runs one user turn against one session.
pub struct Agent<'a> {
    pub backend: &'a mut Backend,
    pub tools: &'a ToolRuntime,
    pub policy: &'a mut PermissionPolicy,
    pub approvals: &'a ApprovalGate,
    pub events: &'a EventSink,
    pub config: AgentConfig,
    /// Optional durable checkpoint target. The desktop app sets this so a long
    /// Task turn keeps its transcript and checklist even if the window closes.
    pub store: Option<&'a SessionStore>,
}

impl<'a> Agent<'a> {
    pub async fn run(
        &mut self,
        session: &mut Session,
        user_text: String,
        attachments: Vec<Attachment>,
    ) -> Result<TurnResult, CoreError> {
        let turn_id = format!("turn-{}", now_ms());
        self.events.emit(AgentEvent::TurnStarted {
            turn_id: turn_id.clone(),
            session_id: session.id.clone(),
        });

        // A checkpoint may have been saved while a tool was running. Never
        // replay it automatically: a write might already have taken effect.
        // Close only the trailing unresolved calls with an explicit unknown
        // outcome before accepting the next user message.
        let recovered = recover_interrupted_calls(session);
        if !recovered.is_empty() {
            self.events.emit(AgentEvent::Notice {
                message: "An earlier tool call was interrupted; its outcome is unknown. Check the workspace before retrying writes.".to_string(),
            });
            for result in recovered {
                session.push(result);
            }
        }

        // A finished checklist belongs to the preceding task, not a new turn.
        // Keep unfinished plans so the user can ask the agent to continue them.
        if self.config.effort.is_task() && plan_all_done(session) {
            session.plan.clear();
            self.events.emit(AgentEvent::PlanUpdated {
                turn_id: turn_id.clone(),
                steps: vec![],
            });
        }
        let original_user_text = user_text.clone();
        let message = self.user_message(user_text, &attachments);
        session.retitle(&message.content);
        session.push(message);
        self.checkpoint(session);

        let mut rounds = 0u32;
        let mut prompt_tokens = 0u32;
        let mut output_tokens = 0u32;
        let mut best_speed: f32 = 0.0;
        let mut thinking = self.config.thinking;
        let mut retried_without_thinking = false;
        let mut repaired_tool_call = false;
        let mut retry_instruction = "";
        let mut plan_nudges = 0u32;
        let mut planning_attempts = 0u32;
        let mut reconcile_next = false;
        let mut suppress_plan_next = false;
        let mut suppress_research_next = false;
        let mut research_streak = 0u32;
        let mut unchanged_plan_updates = 0u32;
        let mut empty_task_retries = 0u32;
        let mut truncated_task_retry = false;
        let mut tool_context = (self.config.max_tool_result_chars, true, true);
        let mut repeated_fetches: HashMap<String, u32> = HashMap::new();
        let mut python_succeeded = false;
        // Bounded, heuristic loop detection: identical calls and failures can
        // indicate stalling, but successful or novel calls are not proof of progress.
        let mut recent_calls = VecDeque::new();
        let mut failed_calls = 0u32;

        let outcome = loop {
            if self.cancelled() {
                break StopReason::Cancelled;
            }

            // Reserve a few initial generations for planning. Presenting only
            // update_plan avoids burying it among unrelated tools for small
            // models; after the retries, permit work but report an absent plan.
            let planning = self.config.effort.is_task()
                && session.plan.is_empty()
                && planning_attempts < 3;
            let mut tools_for_prompt = tools::specs(self.config.web_enabled);
            if !self.tools.has_python_sandbox() {
                tools_for_prompt.retain(|tool| tool.name != "python_calculate");
            }
            if planning {
                planning_attempts += 1;
            }
            let reconciling_plan = reconcile_next;
            if planning || reconciling_plan {
                tools_for_prompt.retain(|tool| tool.name == "update_plan");
            } else if suppress_plan_next {
                tools_for_prompt.retain(|tool| tool.name != "update_plan");
            }
            let research_paused = suppress_research_next;
            if research_paused {
                tools_for_prompt
                    .retain(|tool| tool.name != "web_search" && tool.name != "web_fetch");
            }
            reconcile_next = false;
            suppress_plan_next = false;
            suppress_research_next = false;
            let output = match self
                .generate_with_retry(
                    session,
                    &turn_id,
                    &tools_for_prompt,
                    retry_instruction,
                    thinking,
                    &mut tool_context,
                )
                .await
            {
                Ok(output) => output,
                Err(err) => {
                    self.events.emit(AgentEvent::Error {
                        turn_id: Some(turn_id.clone()),
                        message: err.to_string(),
                    });
                    let stop = match err {
                        CoreError::ContextOverflow { .. } => StopReason::ContextFull,
                        _ => StopReason::Failed,
                    };
                    if stop == StopReason::ContextFull && self.config.effort.is_task() {
                        let message = Message::assistant("Task incomplete: the conversation exceeded the model's context before the work was finished. Review the checklist and start a follow-up with the remaining steps.", None, vec![]);
                        self.events.emit(AgentEvent::AssistantMessage {
                            turn_id: turn_id.clone(),
                            message: message.clone(),
                        });
                        session.push(message);
                        self.checkpoint(session);
                    }
                    break stop;
                }
            };

            rounds = rounds.saturating_add(1);
            prompt_tokens = prompt_tokens.saturating_add(output.prompt_tokens);
            output_tokens = output_tokens.saturating_add(output.output_tokens);
            best_speed = best_speed.max(output.tokens_per_second);

            if output.stop == GenStop::Cancelled {
                let parsed = parse_reply(&output.text, thinking);
                if !parsed.content.trim().is_empty() || parsed.thinking.is_some() {
                    let content = python_execution_notice(
                        parsed.content,
                        &original_user_text,
                        python_succeeded,
                    );
                    let partial = Message::assistant(content, parsed.thinking, vec![]);
                    self.events.emit(AgentEvent::AssistantMessage {
                        turn_id: turn_id.clone(),
                        message: partial.clone(),
                    });
                    session.push(partial);
                    self.checkpoint(session);
                }
                break StopReason::Cancelled;
            }

            let parsed = parse_reply(&output.text, thinking);
            retry_instruction = "";
            if planning && !parsed.calls.iter().any(|call| call.name == "update_plan") {
                self.events.emit(AgentEvent::GenerationReset {
                    turn_id: turn_id.clone(),
                });
                retry_instruction = "First call update_plan with a JSON checklist. Example: <tool_call><function=update_plan><parameter=steps>[{\"title\":\"Inspect inputs\",\"status\":\"running\"},{\"title\":\"Complete and verify work\",\"status\":\"pending\"}]</parameter></function></tool_call>. Do not answer yet.";
                continue;
            }
            if self.config.effort.is_task()
                && output.stop == GenStop::MaxTokens
                && parsed.calls.is_empty()
            {
                if !truncated_task_retry {
                    truncated_task_retry = true;
                    thinking = false;
                    retry_instruction = "The previous response was cut off by the output limit. Continue the task or provide a concise, complete answer covering every part. Do not claim unfinished work is done.";
                    self.events.emit(AgentEvent::GenerationReset {
                        turn_id: turn_id.clone(),
                    });
                    self.events.emit(AgentEvent::Notice {
                        message: "Retrying a truncated task response.".to_string(),
                    });
                    continue;
                }
                let content = task_verification_notice(format!(
                    "Task incomplete: the response was cut off by the output limit.\n\n{}",
                    parsed.content
                ));
                let content =
                    python_execution_notice(content, &original_user_text, python_succeeded);
                let message = Message::assistant(content, parsed.thinking, vec![]);
                self.events.emit(AgentEvent::AssistantMessage {
                    turn_id: turn_id.clone(),
                    message: message.clone(),
                });
                session.push(message);
                self.checkpoint(session);
                break StopReason::Failed;
            }
            let ambiguous_search = self.config.web_enabled
                && parsed.calls.is_empty()
                && looks_like_search_arguments(&parsed.content, &original_user_text);
            if parsed.calls.is_empty() && (parsed.malformed.is_some() || ambiguous_search) {
                self.events.emit(AgentEvent::GenerationReset {
                    turn_id: turn_id.clone(),
                });
                if !repaired_tool_call {
                    repaired_tool_call = true;
                    retry_instruction = "Your last response was not a valid tool call. If you need a tool, include its function name and parameters using this exact format: <tool_call><function=web_search><parameter=query>search terms</parameter></function></tool_call>. Do not output only a JSON arguments object. Otherwise answer the user in plain language.";
                    continue;
                }
                self.events.emit(AgentEvent::Error {
                    turn_id: Some(turn_id.clone()),
                    message: "I couldn't complete the tool request. Please try again.".to_string(),
                });
                break StopReason::Failed;
            }

            if parsed.calls.is_empty() {
                if self.config.effort.is_task() && plan_nudges < 3 && !plan_complete(session)
                {
                    plan_nudges += 1;
                    reconcile_next = !session.plan.is_empty();
                    retry_instruction = if session.plan.is_empty() {
                        "First create a checklist using this exact tool format, changing the titles to fit the request: <tool_call><function=update_plan><parameter=steps>[{\"title\":\"Inspect inputs\",\"status\":\"running\"},{\"title\":\"Complete and verify work\",\"status\":\"pending\"}]</parameter></function></tool_call>. Do not answer yet."
                    } else {
                        "Review the tool results and the current checklist. Call update_plan now with every checklist step and its current status. Use done only for work already verified, blocked with a note for work you cannot do, and pending for unfinished work. Do not give a final answer yet."
                    };
                    self.events.emit(AgentEvent::GenerationReset {
                        turn_id: turn_id.clone(),
                    });
                    self.events.emit(AgentEvent::Notice {
                        message: "Continuing the task checklist.".to_string(),
                    });
                    continue;
                }
                if parsed.content.trim().is_empty() {
                    // Reasoning ran long and the answer never arrived: retry once
                    // with reasoning switched off, which usually fixes it.
                    if parsed.truncated_thinking && thinking && !retried_without_thinking {
                        retried_without_thinking = true;
                        self.events.emit(AgentEvent::GenerationReset {
                            turn_id: turn_id.clone(),
                        });
                        thinking = false;
                        self.events.emit(AgentEvent::Notice {
                            message: "Retrying without reasoning.".to_string(),
                        });
                        continue;
                    }
                    if self.config.effort.is_task() && empty_task_retries < 2 {
                        empty_task_retries += 1;
                        thinking = false;
                        retry_instruction = "Your previous response was empty. Continue the task using the available evidence and tools, or explain what is blocked. Do not silently stop.";
                        self.events.emit(AgentEvent::GenerationReset {
                            turn_id: turn_id.clone(),
                        });
                        self.events.emit(AgentEvent::Notice {
                            message: "Retrying an empty task response.".to_string(),
                        });
                        continue;
                    }
                    if !parsed.truncated_thinking {
                        self.events.emit(AgentEvent::Notice {
                            message: "No answer. Try rephrasing.".to_string(),
                        });
                    }
                    break StopReason::EmptyAnswer;
                }
                let content = if self.config.effort.is_task() {
                    let answer = if !plan_all_done(session) {
                        format!("Task incomplete: the checklist has unfinished or blocked steps, or was not created.\n\n{}", parsed.content)
                    } else {
                        parsed.content
                    };
                    task_verification_notice(answer)
                } else {
                    parsed.content
                };
                let content =
                    python_execution_notice(content, &original_user_text, python_succeeded);
                let message = Message::assistant(content, parsed.thinking, vec![]);
                self.events.emit(AgentEvent::AssistantMessage {
                    turn_id: turn_id.clone(),
                    message: message.clone(),
                });
                session.push(message);
                self.checkpoint(session);
                break StopReason::Answered;
            }

            let calls: Vec<ToolCall> = parsed
                .calls
                .into_iter()
                .enumerate()
                .map(|(index, call)| call.into_call(format!("{turn_id}-{rounds}-{index}")))
                .collect();

            if let Some(reason) = parsed.malformed {
                self.events.emit(AgentEvent::Notice {
                    message: format!("The model produced a malformed tool call ({reason})."),
                });
            }

            let assistant =
                Message::assistant(parsed.content.clone(), parsed.thinking, calls.clone());
            self.events.emit(AgentEvent::AssistantMessage {
                turn_id: turn_id.clone(),
                message: assistant.clone(),
            });
            session.push(assistant);
            self.checkpoint(session);

            let mut first_call_ok = false;
            let mut repeated_call = false;
            if self.config.effort == Effort::Horizon {
                let call = &calls[0];
                let mut args: Vec<_> = call.arguments.iter().map(|arg| (&arg.name, &arg.value)).collect();
                args.sort_unstable();
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                call.name.hash(&mut hasher);
                args.hash(&mut hasher);
                let signature = hasher.finish();
                repeated_call = recent_calls.iter().filter(|&&old| old == signature).count() >= 2;
                recent_calls.push_back(signature);
                if recent_calls.len() > 8 {
                    recent_calls.pop_front();
                }
            }
            for (index, call) in calls.iter().enumerate() {
                if index > 0 {
                    // The model is instructed to issue one call at a time. Do
                    // not execute a batch of unreviewed actions after a single
                    // generation, even if it ignores that instruction.
                    let detail = "Only one tool call may run per response. This call was not executed; review the first result before requesting another action.";
                    let mut result = Message::tool_result(call.id.clone(), detail);
                    result.tool_status = Some(ToolResultStatus::Failed);
                    self.events.emit(AgentEvent::ToolMessage {
                        turn_id: turn_id.clone(),
                        message: result.clone(),
                    });
                    session.push(result);
                    self.checkpoint(session);
                    retry_instruction = "Issue one tool call at a time and wait for its result before requesting another action.";
                    continue;
                }
                if repeated_call {
                    let mut result = Message::tool_result(call.id.clone(), "Horizon stopped before repeating this call again. Review prior results and continue manually if appropriate.");
                    result.tool_status = Some(ToolResultStatus::Failed);
                    self.events.emit(AgentEvent::ToolMessage {
                        turn_id: turn_id.clone(),
                        message: result.clone(),
                    });
                    session.push(result);
                    self.checkpoint(session);
                    continue;
                }
                if !tools_for_prompt.iter().any(|tool| tool.name == call.name) {
                    let message = if research_paused
                        && (call.name == "web_search" || call.name == "web_fetch")
                    {
                        "Research is paused for this generation. Use the evidence already gathered to continue the task; research tools will be available again."
                    } else {
                        "Error: this tool is not available."
                    };
                    let mut result = Message::tool_result(call.id.clone(), message);
                    result.tool_status = Some(ToolResultStatus::Failed);
                    self.events.emit(AgentEvent::ToolMessage {
                        turn_id: turn_id.clone(),
                        message: result.clone(),
                    });
                    session.push(result);
                    self.checkpoint(session);
                    continue;
                }
                if self.config.effort.is_task() && call.name == "web_fetch" {
                    let key = fetch_key(call.arg("url").unwrap_or(""));
                    if repeated_fetches.get(&key).copied().unwrap_or(0) >= 2 {
                        let detail = "This source was already fetched twice in this task. Use its earlier results or choose a different source; continue the remaining work.";
                        self.events.emit(AgentEvent::ToolCallFinished {
                            turn_id: turn_id.clone(),
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            ok: false,
                            summary: "skipped repeated fetch".to_string(),
                            detail: detail.to_string(),
                            thumbnail: None,
                        });
                        let mut result = Message::tool_result(call.id.clone(), detail);
                        result.tool_status = Some(ToolResultStatus::Failed);
                        self.events.emit(AgentEvent::ToolMessage {
                            turn_id: turn_id.clone(),
                            message: result.clone(),
                        });
                        session.push(result);
                        self.checkpoint(session);
                        retry_instruction = "Do not refetch that source. Use earlier evidence, try a different source if needed, and continue the remaining task.";
                        continue;
                    }
                }
                let previous_statuses: Vec<_> =
                    session.plan.iter().map(|step| step.status).collect();
                let ok = self
                    .execute_call(session, &turn_id, call, reconciling_plan)
                    .await;
                first_call_ok = ok;
                if call.name == "python_calculate" && ok {
                    python_succeeded = true;
                }
                if self.config.effort.is_task() {
                    if call.name == "web_search" || call.name == "web_fetch" {
                        research_streak += 1;
                        if research_streak >= 4 {
                            research_streak = 0;
                            suppress_research_next = true;
                            retry_instruction = "Use the evidence already gathered to work on the remaining task now. Cite the sources you actually opened, or clearly report missing evidence. You can research more later if needed.";
                            self.events.emit(AgentEvent::Notice {
                                message: "Pausing repeated research briefly to continue the task."
                                    .to_string(),
                            });
                        }
                    } else {
                        research_streak = 0;
                    }
                }
                if self.config.effort.is_task() && call.name == "update_plan" && ok {
                    let statuses: Vec<_> = session.plan.iter().map(|step| step.status).collect();
                    if !previous_statuses.is_empty()
                        && previous_statuses == statuses
                        && !plan_complete(session)
                    {
                        unchanged_plan_updates += 1;
                    } else {
                        unchanged_plan_updates = 0;
                    }
                    if unchanged_plan_updates >= 2 {
                        unchanged_plan_updates = 0;
                        suppress_plan_next = true;
                        retry_instruction = "The checklist has not progressed. Work on the next step now, using an appropriate tool or the supplied attachments. Do not update the checklist again until work is done or blocked.";
                        self.events.emit(AgentEvent::Notice {
                            message: "The checklist has not changed; continuing with the work."
                                .to_string(),
                        });
                    }
                } else if call.name != "update_plan" {
                    unchanged_plan_updates = 0;
                }
                if self.config.effort.is_task() && call.name == "web_fetch" {
                    let key = fetch_key(call.arg("url").unwrap_or(""));
                    let count = repeated_fetches.entry(key).or_default();
                    *count += 1;
                    if *count == 2 {
                        retry_instruction = "The same source was fetched twice. Use the results already obtained, work on the remaining questions, or choose a different source if needed.";
                        self.events.emit(AgentEvent::Notice {
                            message: "Avoiding a repeated source fetch; continuing the task."
                                .to_string(),
                        });
                    }
                }
                if self.cancelled() {
                    break;
                }
            }
            if self.cancelled() {
                break StopReason::Cancelled;
            }

            if self.config.effort == Effort::Horizon {
                failed_calls = if first_call_ok { 0 } else { failed_calls.saturating_add(1) };
            }
            let stalled = self.config.effort == Effort::Horizon
                && (repeated_call || failed_calls >= 4);
            if stalled || (self.config.effort != Effort::Horizon && rounds >= self.config.max_tool_rounds) {
                // Give the model one final chance with tools removed, so the
                // user gets a summary instead of silence.
                let notice = if stalled {
                    "Tool use stopped because calls were repeated or failed. Summarize verified work and what remains blocked; do not call tools or claim unverified work is complete."
                } else {
                    "Tool budget reached. Answer now using the information you already have; do not call tools."
                };
                let output = match self
                    .generate_with_retry(
                        session,
                        &turn_id,
                        &[],
                        notice,
                        thinking,
                        &mut tool_context,
                    )
                    .await
                {
                    Ok(output) => output,
                    Err(_) => break if stalled { StopReason::Stalled } else { StopReason::ToolBudget },
                };
                output_tokens = output_tokens.saturating_add(output.output_tokens);
                prompt_tokens = prompt_tokens.saturating_add(output.prompt_tokens);
                best_speed = best_speed.max(output.tokens_per_second);
                let parsed = parse_reply(&output.text, thinking);
                let content = if parsed.content.trim().is_empty() {
                    "Stopped before I could summarise.".to_string()
                } else {
                    parsed.content
                };
                let content = if self.config.effort.is_task() {
                    let answer = if stalled {
                        format!("Task stopped: repeated or unsuccessful tool calls suggest a loop. Review the checklist and results before continuing; productivity is not verified.\n\n{content}")
                    } else if !plan_all_done(session) {
                        format!("Task incomplete: the tool budget was reached before all checklist steps were done.\n\n{content}")
                    } else {
                        content
                    };
                    task_verification_notice(answer)
                } else {
                    content
                };
                let content =
                    python_execution_notice(content, &original_user_text, python_succeeded);
                let message = Message::assistant(content, parsed.thinking, vec![]);
                self.events.emit(AgentEvent::AssistantMessage {
                    turn_id: turn_id.clone(),
                    message: message.clone(),
                });
                session.push(message);
                self.checkpoint(session);
                break if stalled { StopReason::Stalled } else { StopReason::ToolBudget };
            }
        };

        let result = TurnResult {
            stop: outcome,
            rounds,
            prompt_tokens,
            output_tokens,
            tokens_per_second: best_speed,
        };
        self.events.emit(AgentEvent::TurnFinished {
            turn_id,
            stop: result.stop,
            rounds: result.rounds,
            prompt_tokens: result.prompt_tokens,
            output_tokens: result.output_tokens,
            tokens_per_second: result.tokens_per_second,
        });
        Ok(result)
    }

    /// Builds the user message, folding text attachments into the prompt text.
    fn user_message(&self, text: String, attachments: &[Attachment]) -> Message {
        let mut content = text.trim().to_string();
        let mut kept = Vec::new();
        for attachment in attachments {
            match attachment.kind {
                AttachmentKind::Image => kept.push(attachment.clone()),
                AttachmentKind::Text => {
                    let limit = self.config.effort.fetch_chars();
                    match self
                        .tools
                        .read_attachment_text(Path::new(&attachment.path), limit)
                    {
                        Some(body) => {
                            content.push_str(&format!(
                                "\n\nAttached file `{}`:\n\n{body}",
                                attachment.name
                            ));
                            kept.push(attachment.clone());
                        }
                        None => {
                            content.push_str(&format!(
                                "\n\n(The attached file `{}` could not be read.)",
                                attachment.name
                            ));
                        }
                    }
                }
            }
        }
        Message::user(content, kept)
    }

    async fn execute_call(
        &mut self,
        session: &mut Session,
        turn_id: &str,
        call: &ToolCall,
        reconciling_plan: bool,
    ) -> bool {
        let risk_label = tools::spec_for(&call.name)
            .map(|spec| spec.risk.label().to_string())
            .unwrap_or_else(|| "changes files".to_string());
        let preview = call.preview();

        if let Permission::Ask { .. } = self.policy.evaluate(call) {
            // Register the wait *before* announcing the request. Otherwise a
            // fast UI could answer a request the harness is not yet waiting on,
            // and the turn would stall until the approval timeout.
            let receiver = self.approvals.register(&call.id).await;
            self.events.emit(AgentEvent::ToolCallNeedsApproval {
                turn_id: turn_id.to_string(),
                call_id: call.id.clone(),
                name: call.name.clone(),
                preview: preview.clone(),
                risk: risk_label.clone(),
            });
            let allowed = match tokio::time::timeout(APPROVAL_TIMEOUT, receiver).await {
                Ok(Ok(decision)) => decision,
                _ => false,
            };
            if !allowed {
                self.events.emit(AgentEvent::ToolCallDenied {
                    turn_id: turn_id.to_string(),
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                });
                let mut result = Message::tool_result(
                    call.id.clone(),
                    "The user declined this tool call. Do not try it again; answer with what you already have or ask what they would prefer.",
                );
                result.tool_status = Some(ToolResultStatus::Denied);
                self.events.emit(AgentEvent::ToolMessage {
                    turn_id: turn_id.to_string(),
                    message: result.clone(),
                });
                session.push(result);
                self.checkpoint(session);
                return false;
            }
            // Remember the decision for the rest of the session in auto mode;
            // manual mode keeps asking, which is the point of manual mode.
            if self.config.permission_mode == PermissionMode::Auto {
                self.policy.grant(&call.name);
            }
        }

        self.events.emit(AgentEvent::ToolCallStarted {
            turn_id: turn_id.to_string(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            preview,
            risk: risk_label,
        });

        let outcome = if call.name == "update_plan" {
            apply_plan(session, call, reconciling_plan)
        } else {
            self.tools.execute(call).await
        };
        if call.name == "update_plan" && outcome.ok {
            self.events.emit(AgentEvent::PlanUpdated {
                turn_id: turn_id.to_string(),
                steps: session.plan.clone(),
            });
        }
        self.events.emit(AgentEvent::ToolCallFinished {
            turn_id: turn_id.to_string(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            ok: outcome.ok,
            summary: outcome.summary.clone(),
            detail: truncate_chars(&outcome.text, 700),
            thumbnail: thumbnail_for(&outcome.media),
        });

        let mut message = Message::tool_result(call.id.clone(), outcome.text.clone());
        message.tool_status = Some(if outcome.ok {
            ToolResultStatus::Ok
        } else {
            ToolResultStatus::Failed
        });
        message.attachments = outcome
            .media
            .iter()
            .map(|path| Attachment {
                kind: AttachmentKind::Image,
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "screenshot.png".to_string()),
                path: path.display().to_string(),
            })
            .collect();
        self.events.emit(AgentEvent::ToolMessage {
            turn_id: turn_id.to_string(),
            message: message.clone(),
        });
        session.push(message);
        self.checkpoint(session);
        outcome.ok
    }

    /// Generates, trimming old turns if the prompt does not fit.
    async fn generate_with_retry(
        &mut self,
        session: &mut Session,
        turn_id: &str,
        tools: &[ToolSpec],
        system_suffix: &str,
        thinking: bool,
        tool_context: &mut (usize, bool, bool),
    ) -> Result<GenerateOutput, CoreError> {
        let mut attempts = 0;
        let (mut tool_result_limit, mut keep_tool_images, mut keep_current_thinking) =
            *tool_context;
        loop {
            attempts += 1;
            let request = self.build_request(
                session,
                turn_id,
                tools,
                system_suffix,
                thinking,
                (tool_result_limit, keep_tool_images, keep_current_thinking),
            );
            match self.backend.generate(request, self.events).await {
                Ok(output) => {
                    *tool_context = (tool_result_limit, keep_tool_images, keep_current_thinking);
                    return Ok(output);
                }
                Err(err @ CoreError::ContextOverflow { .. }) => {
                    let max_attempts = if self.config.effort.is_task() {
                        6
                    } else {
                        4
                    };
                    if attempts >= max_attempts {
                        return Err(err);
                    }
                    self.events.emit(AgentEvent::GenerationReset {
                        turn_id: turn_id.to_string(),
                    });
                    if session.trim_oldest_turn() {
                        self.events.emit(AgentEvent::Notice {
                            message: "Removed the oldest turn from context.".to_string(),
                        });
                    } else if self.config.effort.is_task()
                        && keep_current_thinking
                        && session
                            .messages
                            .iter()
                            .rev()
                            .take_while(|m| m.role != crate::session::Role::User)
                            .any(|m| {
                                m.role == crate::session::Role::Assistant
                                    && m.thinking
                                        .as_deref()
                                        .is_some_and(|text| !text.trim().is_empty())
                            })
                    {
                        keep_current_thinking = false;
                        self.events.emit(AgentEvent::Notice {
                            message: "Omitted earlier reasoning from the model context to make room; the transcript and tool results are unchanged.".to_string(),
                        });
                    } else if keep_tool_images
                        && session.messages.iter().any(|m| {
                            m.role == crate::session::Role::Tool
                                && m.attachments
                                    .iter()
                                    .any(|a| a.kind == AttachmentKind::Image)
                        })
                    {
                        keep_tool_images = false;
                        self.events.emit(AgentEvent::Notice {
                            message: "Omitted tool screenshots from the model context to make room; user attachments and conversation history are unchanged.".to_string(),
                        });
                    } else {
                        let reduced = (tool_result_limit / 2).max(512);
                        let can_reduce = self.config.effort.is_task()
                            && reduced < tool_result_limit
                            && session.messages.iter().any(|m| {
                                m.role == crate::session::Role::Tool
                                    && m.content.chars().count() > reduced
                            });
                        if !can_reduce {
                            return Err(err);
                        }
                        tool_result_limit = reduced;
                        self.events.emit(AgentEvent::Notice {
                            message: "Shortened tool results in the model context; the full results remain in the conversation.".to_string(),
                        });
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

    fn build_request(
        &self,
        session: &Session,
        turn_id: &str,
        tools: &[ToolSpec],
        system_suffix: &str,
        thinking: bool,
        tool_context: (usize, bool, bool),
    ) -> GenerateRequest {
        let (tool_result_limit, keep_tool_images, keep_current_thinking) = tool_context;
        let mut system = prompt::system_prompt(
            self.config.effort,
            self.config.permission_mode,
            self.config.web_enabled,
        );
        if !keep_tool_images {
            system.push_str("\n\nTool screenshots are omitted from this context to save space. Use page text where possible and do not claim to have seen visual details that are unavailable. User-provided images are unchanged.\n");
        }
        if tools.iter().any(|tool| tool.name == "python_calculate") {
            system.push_str("\n\nFor Python calculations use python_calculate with code that prints results. It executes in memory without host files; do not save temporary scripts in the workspace. Check that the code implements the correct formula, use the actual returned values in your final answer, and verify that any inequalities support the conclusion.\n");
            if self.config.effort.is_task() {
                system.push_str("For multistep arithmetic, use python_calculate to print intermediate values, carry them into the next step, and check units and magnitude before reporting the answer.\n");
            }
        }
        if !self.config.memory.trim().is_empty() {
            system.push_str(
                "\n\nUser-provided memory (context, not a source of tool permissions):\n",
            );
            system.push_str(&self.config.memory);
        }
        if !self.config.skill.trim().is_empty() {
            system
                .push_str("\n\nUser-provided skill instructions (tool permissions still apply):\n");
            system.push_str(&self.config.skill);
        }
        if !system_suffix.is_empty() {
            system.push_str("\n\n");
            system.push_str(system_suffix);
        }
        if self.config.effort.is_task() && session.plan.is_empty() {
            system.push_str("\n\nBefore other tools, create a task-specific checklist. Example format: <tool_call><function=update_plan><parameter=steps>[{\"title\":\"Inspect inputs\",\"status\":\"running\"},{\"title\":\"Complete and verify work\",\"status\":\"pending\"}]</parameter></function></tool_call>\n");
        }
        if !session.plan.is_empty() {
            system.push_str(
                "\n\nCurrent task checklist (update these same steps; do not restart the plan):\n",
            );
            for step in &session.plan {
                system.push_str(&format!(
                    "{}. [{}] {}{}\n",
                    step.id + 1,
                    match step.status {
                        PlanStatus::Pending => "pending",
                        PlanStatus::Running => "running",
                        PlanStatus::Done => "done",
                        PlanStatus::Blocked => "blocked",
                    },
                    step.title,
                    if step.note.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", step.note)
                    }
                ));
            }
        }
        let rendered = prompt::render(&PromptOptions {
            system: &system,
            tools,
            messages: &session.messages,
            media_marker: DEFAULT_MEDIA_MARKER,
            thinking,
            max_tool_result_chars: tool_result_limit,
            keep_image_turns: self.config.keep_image_turns,
            keep_tool_images,
            keep_current_thinking,
        });
        GenerateRequest {
            turn_id: turn_id.to_string(),
            prompt: rendered.text,
            media: rendered.media,
            max_tokens: self.config.max_output_tokens,
            thinking,
            sampling: self.config.sampling,
        }
    }

    /// Saves a durable checkpoint when the host supplied a store. Checkpoint
    /// failures are reported but do not abort the turn; the final save still
    /// decides whether the command succeeds.
    fn checkpoint(&self, session: &Session) {
        if let Some(store) = self.store {
            if let Err(err) = store.save(session) {
                self.events.emit(AgentEvent::Notice {
                    message: format!("Could not save a task checkpoint: {err}"),
                });
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.backend
            .cancel_flag()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
}

fn recover_interrupted_calls(session: &Session) -> Vec<Message> {
    let Some(index) = session
        .messages
        .iter()
        .rposition(|message| message.role != Role::Tool)
    else {
        return vec![];
    };
    let assistant = &session.messages[index];
    if assistant.role != Role::Assistant {
        return vec![];
    }
    assistant.tool_calls.iter().filter(|call| {
        !session.messages[index + 1..].iter().any(|message| message.tool_call_id.as_deref() == Some(call.id.as_str()))
    }).map(|call| {
        let mut result = Message::tool_result(call.id.clone(), "The previous tool call was interrupted. Its outcome is unknown; do not assume it failed or repeat a write without checking the workspace and requesting any required approval.");
        result.tool_status = Some(ToolResultStatus::Interrupted);
        result
    }).collect()
}

fn plan_all_done(session: &Session) -> bool {
    !session.plan.is_empty()
        && session
            .plan
            .iter()
            .all(|step| step.status == PlanStatus::Done)
}

// A model can describe running code without making a tool call. Report the
// factual execution status for this turn rather than trusting that claim. This
// does not establish that a successful program was relevant or correct.
fn python_execution_notice(answer: String, user_text: &str, python_succeeded: bool) -> String {
    if python_succeeded {
        return answer;
    }
    let answer_lower = answer.to_ascii_lowercase();
    let request_lower = user_text.to_ascii_lowercase();
    let mentions_execution = |text: &str| {
        text.contains("python")
            && [
                "run", "ran", "execut", "using", "use ", "used", "calculat", "verif", "output",
                "check",
            ]
            .iter()
            .any(|term| text.contains(term))
    };
    if mentions_execution(&answer_lower) || mentions_execution(&request_lower) {
        format!("{answer}\n\nPython tool status: no Python calculation completed successfully in this turn.")
    } else {
        answer
    }
}

fn task_verification_notice(answer: String) -> String {
    format!("{answer}\n\nTask results and checklist are model-generated, not independently verified. Check calculations, sources and file changes before relying on them.")
}

fn plan_complete(session: &Session) -> bool {
    !session.plan.is_empty()
        && session
            .plan
            .iter()
            .all(|step| matches!(step.status, PlanStatus::Done | PlanStatus::Blocked))
}

fn apply_plan(session: &mut Session, call: &ToolCall, reconciling: bool) -> tools::ToolOutcome {
    let Some(raw) = call.arg("steps") else {
        return tools::ToolOutcome::fail("missing steps", "Error: `steps` is required.");
    };
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(err) => return tools::ToolOutcome::fail("invalid plan", format!("Error: {err}")),
    };
    let Some(items) = value.as_array() else {
        return tools::ToolOutcome::fail("invalid plan", "Error: `steps` must be a JSON array.");
    };
    if items.is_empty() || items.len() > 50 {
        return tools::ToolOutcome::fail("invalid plan", "Error: provide between 1 and 50 steps.");
    }
    let mut steps = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(title) = item.get("title").and_then(|v| v.as_str()).map(str::trim) else {
            return tools::ToolOutcome::fail(
                "invalid plan",
                format!("Error: step {index} needs a title."),
            );
        };
        if title.is_empty() || title.chars().count() > 200 {
            return tools::ToolOutcome::fail(
                "invalid plan",
                format!("Error: step {index} title must be 1-200 characters."),
            );
        }
        let status = match item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
        {
            "pending" => PlanStatus::Pending,
            "running" => PlanStatus::Running,
            "done" => PlanStatus::Done,
            "blocked" => PlanStatus::Blocked,
            other => {
                return tools::ToolOutcome::fail(
                    "invalid plan",
                    format!("Error: unknown status `{other}`."),
                )
            }
        };
        let note = item.get("note").and_then(|v| v.as_str()).unwrap_or("");
        if note.chars().count() > 500 {
            return tools::ToolOutcome::fail(
                "invalid plan",
                format!("Error: step {index} note is too long."),
            );
        }
        steps.push(PlanStep {
            id: index,
            title: title.to_string(),
            status,
            note: note.to_string(),
        });
    }
    if reconciling
        && (session.plan.len() != steps.len()
            || session
                .plan
                .iter()
                .zip(&steps)
                .any(|(old, new)| old.title != new.title))
    {
        return tools::ToolOutcome::fail("invalid plan", "Error: during checklist reconciliation, keep every existing step title in the same order. Update statuses and notes without replacing the task.");
    }
    let running = steps
        .iter()
        .filter(|step| step.status == PlanStatus::Running)
        .count();
    if running > 1 {
        return tools::ToolOutcome::fail(
            "invalid plan",
            "Error: keep at most one step running at a time.",
        );
    }
    session.plan = steps;
    let done = session
        .plan
        .iter()
        .filter(|step| step.status == PlanStatus::Done)
        .count();
    tools::ToolOutcome::ok(
        format!("plan {done}/{}", session.plan.len()),
        format!(
            "Checklist updated: {done}/{} steps done.",
            session.plan.len()
        ),
    )
}

/// Encodes the first image a tool produced as a data URL for the UI.
///
/// Screenshots stay under a few hundred kilobytes; anything bigger is skipped so
/// a huge image can never stall the event stream.
fn thumbnail_for(media: &[std::path::PathBuf]) -> Option<String> {
    use base64::Engine as _;

    let path = media.first()?;
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 900_000 {
        return None;
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod context_compaction_tests {
    use super::*;
    use crate::backend::{ScriptFailure, ScriptedBackend};
    use crate::session::Role;

    #[test]
    fn python_notice_is_bound_to_successful_execution_in_the_current_turn() {
        let claim = "Using Python to verify the answer: 42.".to_string();
        assert!(
            python_execution_notice(claim.clone(), "Calculate this", false)
                .contains("no Python calculation completed successfully")
        );
        assert_eq!(
            python_execution_notice(claim.clone(), "Calculate this", true),
            claim
        );
        assert!(
            python_execution_notice("The result is 42.".into(), "Please use Python", false)
                .contains("no Python calculation completed successfully")
        );
        assert_eq!(
            python_execution_notice(
                "Python is a programming language.".into(),
                "What is Python?",
                false
            ),
            "Python is a programming language."
        );
    }

    #[test]
    fn fetch_guard_normalizes_fragments_without_collapsing_queries() {
        assert_eq!(
            fetch_key("https://EXAMPLE.com/page#first"),
            fetch_key("https://example.com/page#second")
        );
        assert_ne!(
            fetch_key("https://example.com/page?q=one"),
            fetch_key("https://example.com/page?q=two")
        );
    }

    #[tokio::test]
    async fn overflow_omits_current_reasoning_without_changing_transcript() {
        let settings = Settings {
            effort: Effort::Task,
            ..Settings::default()
        };
        let mut cfg = AgentConfig::from_settings(&settings);
        cfg.thinking = false;
        let root =
            std::env::temp_dir().join(format!("cordial-reasoning-test-{}", uuid::Uuid::new_v4()));
        let tools = ToolRuntime::new(root.clone(), root.join("scratch"), Effort::Task).unwrap();
        let mut scripted = ScriptedBackend::new(vec!["Done.", "Done again."]);
        scripted.fail_once(ScriptFailure::Overflow);
        let mut backend = Backend::Scripted(scripted);
        let gate = ApprovalGate::new();
        let mut policy = PermissionPolicy::new(PermissionMode::Auto);
        let (sink, _receiver) = EventSink::channel();
        let mut session = Session::new();
        session.push(Message::user("Research this", vec![]));
        session.push(Message::assistant(
            "Visible action",
            Some("private long reasoning".into()),
            vec![],
        ));
        session.push(Message::tool_result("call", "source result"));
        let mut agent = Agent {
            backend: &mut backend,
            tools: &tools,
            policy: &mut policy,
            approvals: &gate,
            events: &sink,
            config: cfg,
            store: None,
        };
        let mut context = (6000, true, true);
        agent
            .generate_with_retry(&mut session, "t", &[], "", false, &mut context)
            .await
            .unwrap();
        agent
            .generate_with_retry(&mut session, "t", &[], "", false, &mut context)
            .await
            .unwrap();
        drop(agent);
        assert!(!context.2);
        assert_eq!(
            session.messages[1].thinking.as_deref(),
            Some("private long reasoning")
        );
        assert_eq!(session.messages[1].role, Role::Assistant);
        if let Backend::Scripted(scripted) = &backend {
            assert_eq!(scripted.requests.len(), 2);
            for request in &scripted.requests {
                assert!(!request.prompt.contains("private long reasoning"));
                assert!(request.prompt.contains("Visible action"));
                assert!(request.prompt.contains("source result"));
            }
        } else {
            panic!("expected scripted backend");
        }
    }
}
