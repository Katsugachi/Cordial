//! The agentic loop.
//!
//! One user turn is: render the prompt, generate, parse tool calls, get
//! permission, run the tools, repeat until the model answers or the effort
//! budget runs out. Cancellation, context overflow and empty answers are handled
//! here so the UI only ever sees clean events.

use std::collections::HashMap;
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
use crate::session::{Attachment, AttachmentKind, Message, Session, ToolCall};
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

        let message = self.user_message(user_text, &attachments);
        session.retitle(&message.content);
        session.push(message);

        let mut rounds = 0u32;
        let mut prompt_tokens = 0u32;
        let mut output_tokens = 0u32;
        let mut best_speed: f32 = 0.0;
        let mut thinking = self.config.thinking;
        let mut retried_without_thinking = false;

        let outcome = loop {
            if self.cancelled() {
                break StopReason::Cancelled;
            }

            let tools_for_prompt = tools::specs(self.config.web_enabled);
            let output = match self
                .generate_with_retry(session, &turn_id, &tools_for_prompt, "", thinking)
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
                    break stop;
                }
            };

            rounds += 1;
            prompt_tokens = prompt_tokens.saturating_add(output.prompt_tokens);
            output_tokens = output_tokens.saturating_add(output.output_tokens);
            best_speed = best_speed.max(output.tokens_per_second);

            if output.stop == GenStop::Cancelled {
                let parsed = parse_reply(&output.text, thinking);
                if !parsed.content.trim().is_empty() || parsed.thinking.is_some() {
                    let partial = Message::assistant(parsed.content, parsed.thinking, vec![]);
                    self.events.emit(AgentEvent::AssistantMessage {
                        turn_id: turn_id.clone(),
                        message: partial.clone(),
                    });
                    session.push(partial);
                }
                break StopReason::Cancelled;
            }

            let parsed = parse_reply(&output.text, thinking);

            if parsed.calls.is_empty() {
                if parsed.content.trim().is_empty() {
                    // Reasoning ran long and the answer never arrived: retry once
                    // with reasoning switched off, which usually fixes it.
                    if parsed.truncated_thinking && thinking && !retried_without_thinking {
                        retried_without_thinking = true;
                        thinking = false;
                        self.events.emit(AgentEvent::Notice {
                            message: "Retrying without reasoning.".to_string(),
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
                let message = Message::assistant(parsed.content, parsed.thinking, vec![]);
                self.events.emit(AgentEvent::AssistantMessage {
                    turn_id: turn_id.clone(),
                    message: message.clone(),
                });
                session.push(message);
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

            for call in &calls {
                self.execute_call(session, &turn_id, call).await;
                if self.cancelled() {
                    break;
                }
            }
            if self.cancelled() {
                break StopReason::Cancelled;
            }

            if rounds >= self.config.max_tool_rounds {
                // Give the model one final chance with tools removed, so the
                // user gets a summary instead of silence.
                let notice = "Tool budget reached. Answer now using the information you already have; do not call tools.";
                let output = match self
                    .generate_with_retry(session, &turn_id, &[], notice, thinking)
                    .await
                {
                    Ok(output) => output,
                    Err(_) => break StopReason::ToolBudget,
                };
                output_tokens = output_tokens.saturating_add(output.output_tokens);
                prompt_tokens = prompt_tokens.saturating_add(output.prompt_tokens);
                best_speed = best_speed.max(output.tokens_per_second);
                let parsed = parse_reply(&output.text, thinking);
                let content = if parsed.content.trim().is_empty() {
                    "Tool limit reached before I could summarise.".to_string()
                } else {
                    parsed.content
                };
                let message = Message::assistant(content, parsed.thinking, vec![]);
                self.events.emit(AgentEvent::AssistantMessage {
                    turn_id: turn_id.clone(),
                    message: message.clone(),
                });
                session.push(message);
                break StopReason::ToolBudget;
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

    async fn execute_call(&mut self, session: &mut Session, turn_id: &str, call: &ToolCall) {
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
                session.push(Message::tool_result(
                    call.id.clone(),
                    "The user declined this tool call. Do not try it again; answer with what you already have or ask what they would prefer.",
                ));
                return;
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

        let outcome = self.tools.execute(call).await;
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
    }

    /// Generates, trimming old turns if the prompt does not fit.
    async fn generate_with_retry(
        &mut self,
        session: &mut Session,
        turn_id: &str,
        tools: &[ToolSpec],
        system_suffix: &str,
        thinking: bool,
    ) -> Result<GenerateOutput, CoreError> {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let request = self.build_request(session, turn_id, tools, system_suffix, thinking);
            match self.backend.generate(request, self.events).await {
                Ok(output) => return Ok(output),
                Err(err @ CoreError::ContextOverflow { .. }) => {
                    if attempts >= 4 || !session.trim_oldest_turn() {
                        return Err(err);
                    }
                    self.events.emit(AgentEvent::Notice {
                        message: "Removed the oldest turn from context.".to_string(),
                    });
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
    ) -> GenerateRequest {
        let mut system = prompt::system_prompt(
            self.config.effort,
            self.config.permission_mode,
            self.config.web_enabled,
        );
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
        let rendered = prompt::render(&PromptOptions {
            system: &system,
            tools,
            messages: &session.messages,
            media_marker: DEFAULT_MEDIA_MARKER,
            thinking,
            max_tool_result_chars: self.config.max_tool_result_chars,
            keep_image_turns: self.config.keep_image_turns,
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

    fn cancelled(&self) -> bool {
        self.backend
            .cancel_flag()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }
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
