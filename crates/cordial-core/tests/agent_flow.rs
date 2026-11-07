//! End-to-end harness tests that need no model weights.
//!
//! The scripted backend replays exactly what LFM3 would emit, so these tests
//! cover the parts that break in practice: tool-call round trips, approvals,
//! denials, effort budgets, context trimming and cancellation.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use cordial_core::backend::{Backend, ScriptFailure, ScriptedBackend};
use cordial_core::events::{AgentEvent, EventSink, StopReason};
use cordial_core::permissions::PermissionPolicy;
use cordial_core::session::{Attachment, AttachmentKind, Message, Role, Session};
use cordial_core::settings::{Effort, PermissionMode};
use cordial_core::tools::ToolRuntime;
use cordial_core::{Agent, AgentConfig, ApprovalGate, TurnResult};

struct Fixture {
    workspace: PathBuf,
    scratch: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "cordial-test-{tag}-{}-{}",
            std::process::id(),
            cordial_core::util::now_ms()
        ));
        let workspace = base.join("workspace");
        let scratch = base.join("scratch");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(
            workspace.join("notes.txt"),
            "The plan is to ship on Friday.",
        )
        .unwrap();
        std::fs::write(workspace.join("image.png"), [0x89, b'P', b'N', b'G']).unwrap();
        Self { workspace, scratch }
    }

    fn runtime(&self, effort: Effort) -> ToolRuntime {
        ToolRuntime::new(self.workspace.clone(), self.scratch.clone(), effort).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(root) = self.workspace.parent() {
            std::fs::remove_dir_all(root).ok();
        }
    }
}

fn config(effort: Effort, mode: PermissionMode) -> AgentConfig {
    AgentConfig {
        effort,
        permission_mode: mode,
        web_enabled: false, // tests never touch the network
        memory: String::new(),
        skill: String::new(),
        thinking: false,
        max_output_tokens: 256,
        max_tool_rounds: effort.max_tool_rounds(),
        max_tool_result_chars: 2_000,
        keep_image_turns: 2,
        sampling: cordial_llama::SamplingParams::default(),
    }
}

/// Collects events and optionally answers approval requests.
fn responder(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    gate: ApprovalGate,
    allow: bool,
) -> (Arc<Mutex<Vec<AgentEvent>>>, tokio::task::JoinHandle<()>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let log_clone = Arc::clone(&log);
    let handle = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            if let AgentEvent::ToolCallNeedsApproval { call_id, .. } = &event {
                // Answer as the user would, while the agent waits on the gate.
                gate.resolve(call_id, allow).await;
            }
            if !matches!(event, AgentEvent::TextDelta { .. }) {
                // Keep the log readable: deltas are replayed by the renderer.
                log_clone.lock().unwrap().push(event);
            }
        }
    });
    (log, handle)
}

async fn run_turn(
    fixture: &Fixture,
    script: Vec<&str>,
    cfg: AgentConfig,
    session: &mut Session,
    approvals: Option<(ApprovalGate, bool)>,
) -> (TurnResult, Vec<AgentEvent>) {
    let runtime = fixture.runtime(cfg.effort);
    let mut backend = Backend::Scripted(ScriptedBackend::new(script));
    let gate = approvals
        .as_ref()
        .map(|(gate, _)| gate.clone())
        .unwrap_or_default();
    let allow = approvals.as_ref().map(|(_, allow)| *allow).unwrap_or(true);
    let mut policy = PermissionPolicy::new(cfg.permission_mode);
    let (sink, receiver) = EventSink::channel();
    let (log, handle) = responder(receiver, gate.clone(), allow);

    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: cfg,
        store: None,
    };
    let result = agent
        .run(session, "What is in my notes?".to_string(), Vec::new())
        .await
        .expect("turn should not fail");
    drop(agent);
    drop(sink);
    // Dropping the sink closes the channel, so the collector drains everything
    // the turn emitted and then finishes.
    let _ = handle.await;
    let events = log.lock().unwrap().clone();
    (result, events)
}

#[tokio::test]
async fn plain_answer_is_persisted_and_streamed() {
    let fixture = Fixture::new("plain");
    let mut session = Session::new();
    let (result, events) = run_turn(
        &fixture,
        vec!["The notes say the plan is to ship on Friday."],
        config(Effort::Medium, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;

    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(result.rounds, 1);
    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[1].role, Role::Assistant);
    assert!(session.messages[1].content.contains("Friday"));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::AssistantMessage { .. })));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnFinished {
            stop: StopReason::Answered,
            ..
        }
    )));
}

#[tokio::test]
async fn tool_call_runs_and_feeds_back_into_the_model() {
    let fixture = Fixture::new("tool");
    let mut session = Session::new();
    let script = vec![
        "Reading the notes.\n</think>\n<tool_call>\n<function=read_file>\n<parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>",
        "It says you plan to ship on Friday.",
    ];
    let (result, events) = run_turn(
        &fixture,
        script,
        config(Effort::Medium, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;

    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(result.rounds, 2);
    // user, assistant(tool call), tool result, assistant(answer)
    assert_eq!(session.messages.len(), 4);
    assert_eq!(session.messages[2].role, Role::Tool);
    assert!(session.messages[2].content.contains("ship on Friday"));
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCallFinished { ok: true, .. })));
}

#[tokio::test]
async fn manual_mode_waits_for_approval() {
    let fixture = Fixture::new("manual");
    let mut session = Session::new();
    let script = vec![
        "<tool_call>\n<function=read_file>\n<parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>",
        "Approved, so here is the answer.",
    ];
    let gate = ApprovalGate::new();
    let (result, events) = run_turn(
        &fixture,
        script,
        config(Effort::Medium, PermissionMode::Manual),
        &mut session,
        Some((gate, true)),
    )
    .await;

    assert_eq!(result.stop, StopReason::Answered);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCallNeedsApproval { .. })));
    assert!(session.messages.iter().any(|m| m.role == Role::Tool));
}

#[tokio::test]
async fn denial_is_reported_to_the_model() {
    let fixture = Fixture::new("deny");
    let mut session = Session::new();
    let script = vec![
        "<tool_call>\n<function=read_file>\n<parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>",
        "I could not read the file, so I cannot say.",
    ];
    let gate = ApprovalGate::new();
    let (result, events) = run_turn(
        &fixture,
        script,
        config(Effort::Medium, PermissionMode::Manual),
        &mut session,
        Some((gate, false)),
    )
    .await;

    assert_eq!(result.stop, StopReason::Answered);
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCallDenied { .. })));
    let tool_message = session
        .messages
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("denial recorded as a tool message");
    assert!(tool_message.content.contains("declined"));
}

#[tokio::test]
async fn writes_need_approval_even_in_auto_mode() {
    let fixture = Fixture::new("write");
    let mut session = Session::new();
    let script = vec![
        concat!(
            "<tool_call>\n<function=write_file>\n",
            "<parameter=path>\nplan.md\n</parameter>\n",
            "<parameter=content>\nShip Friday.\n</parameter>\n",
            "</function>\n</tool_call>"
        ),
        "Written.",
    ];
    let gate = ApprovalGate::new();
    let (_, events) = run_turn(
        &fixture,
        script,
        config(Effort::Medium, PermissionMode::Auto),
        &mut session,
        Some((gate, true)),
    )
    .await;

    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCallNeedsApproval { .. })));
    assert!(fixture.workspace.join("plan.md").exists());
}

#[tokio::test]
async fn tool_budget_triggers_a_final_answer() {
    let fixture = Fixture::new("budget");
    let mut session = Session::new();
    let mut cfg = config(Effort::Low, PermissionMode::Auto);
    cfg.max_tool_rounds = 1;
    let script = vec![
        "<tool_call>\n<function=read_file>\n<parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>",
        "Summarising with what I have: Friday.",
    ];
    let (result, _) = run_turn(&fixture, script, cfg, &mut session, None).await;

    assert_eq!(result.stop, StopReason::ToolBudget);
    let last = session.messages.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert!(last.content.contains("Friday"));
}

#[tokio::test]
async fn context_overflow_trims_old_turns_and_retries() {
    let fixture = Fixture::new("overflow");
    let mut session = Session::new();
    // Two completed turns so there is something to trim.
    session.push(Message::user("first question", vec![]));
    session.push(Message::assistant("first answer", None, vec![]));

    let runtime = fixture.runtime(Effort::Medium);
    let mut backend = Backend::Scripted(ScriptedBackend::new(vec!["Recovered answer."]));
    if let Backend::Scripted(scripted) = &mut backend {
        scripted.fail_once(ScriptFailure::Overflow);
    }
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let (sink, _receiver) = EventSink::channel();
    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: config(Effort::Medium, PermissionMode::Auto),
        store: None,
    };
    let result = agent
        .run(&mut session, "second question".to_string(), Vec::new())
        .await
        .unwrap();

    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(session.messages.len(), 2, "old turn should be trimmed away");
    assert_eq!(session.messages[0].content, "second question");
    assert_eq!(session.messages[1].content, "Recovered answer.");
}

#[tokio::test]
async fn cancellation_stops_before_generating() {
    let fixture = Fixture::new("cancel");
    let mut session = Session::new();
    let runtime = fixture.runtime(Effort::Medium);
    let cancel = Arc::new(AtomicBool::new(true));
    let mut backend = Backend::Scripted(
        ScriptedBackend::new(vec!["should never be used"]).with_cancel(Arc::clone(&cancel)),
    );
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let (sink, _receiver) = EventSink::channel();
    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: config(Effort::Medium, PermissionMode::Auto),
        store: None,
    };
    let result = agent
        .run(&mut session, "stop me".to_string(), Vec::new())
        .await
        .unwrap();
    assert_eq!(result.stop, StopReason::Cancelled);
    assert_eq!(result.rounds, 0);
}

#[tokio::test]
async fn text_attachments_are_read_into_the_prompt() {
    let fixture = Fixture::new("attach");
    let mut session = Session::new();
    let runtime = fixture.runtime(Effort::Medium);
    let mut backend = Backend::Scripted(ScriptedBackend::new(vec!["Got it."]));
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let (sink, _receiver) = EventSink::channel();
    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: config(Effort::Medium, PermissionMode::Auto),
        store: None,
    };
    let attachment = Attachment {
        kind: AttachmentKind::Text,
        name: "notes.txt".into(),
        path: fixture.workspace.join("notes.txt").display().to_string(),
    };
    agent
        .run(&mut session, "Read this".to_string(), vec![attachment])
        .await
        .unwrap();
    drop(agent);

    let user_message = &session.messages[0];
    assert!(user_message.content.contains("Attached file `notes.txt`"));
    assert!(user_message.content.contains("ship on Friday"));

    // The rendered prompt must include the file body and no media markers.
    if let Backend::Scripted(scripted) = &backend {
        let request = scripted.requests.first().expect("one request");
        assert!(request.prompt.contains("ship on Friday"));
        assert!(!request.prompt.contains("<__media__>"));
    }
}

#[tokio::test]
async fn user_memory_and_skill_are_included_across_sessions() {
    use cordial_core::settings::Settings;
    let fixture = Fixture::new("memory");
    let runtime = fixture.runtime(Effort::Medium);
    let mut backend = Backend::Scripted(ScriptedBackend::new(vec![
        "First answer.",
        "Second answer.",
    ]));
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Manual);
    let (sink, _receiver) = EventSink::channel();
    let settings = Settings {
        memory: "I prefer metric units.".into(),
        skill: "Summarise as a short checklist.".into(),
        ..Settings::default()
    };
    let mut config = AgentConfig::from_settings(&settings);
    config.thinking = false;
    config.web_enabled = false;
    {
        let mut agent = Agent {
            backend: &mut backend,
            tools: &runtime,
            policy: &mut policy,
            approvals: &gate,
            events: &sink,
            config,
            store: None,
        };
        agent
            .run(&mut Session::new(), "First chat".into(), vec![])
            .await
            .unwrap();
        agent
            .run(&mut Session::new(), "Second chat".into(), vec![])
            .await
            .unwrap();
    }
    let Backend::Scripted(scripted) = backend else {
        panic!("expected scripted backend")
    };
    assert_eq!(scripted.requests.len(), 2);
    for request in scripted.requests {
        assert!(request.prompt.contains("I prefer metric units."));
        assert!(request.prompt.contains("Summarise as a short checklist."));
        assert!(request.prompt.contains("tool permissions still apply"));
    }
}

#[tokio::test]
async fn ambiguous_search_arguments_trigger_repair_not_execution() {
    let fixture = Fixture::new("repair-search");
    let mut cfg = config(Effort::Medium, PermissionMode::Auto);
    cfg.web_enabled = true;
    let mut session = Session::new();
    let (result, events) = run_turn(
        &fixture,
        vec![
            r#"{"query":"llama.cpp latest release notes","top_n":10}"#,
            "<tool_call><function=read_file><parameter=path>notes.txt</parameter></function></tool_call>",
            "The notes say the plan is to ship on Friday.",
        ],
        cfg,
        &mut session,
        None,
    ).await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::GenerationReset { .. })));
    assert!(!session.messages.iter().any(|m| m.content.contains("top_n")));
    assert!(session
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .any(|c| c.name == "read_file"));
    assert!(!session
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .any(|c| c.name == "web_search"));
}

#[tokio::test]
async fn repeated_malformed_tool_arguments_fail_instead_of_becoming_an_answer() {
    let fixture = Fixture::new("repair-fails");
    let mut cfg = config(Effort::Medium, PermissionMode::Auto);
    cfg.web_enabled = true;
    let mut session = Session::new();
    let (result, events) = run_turn(
        &fixture,
        vec![r#"{"query":"test"}"#, r#"{"query":"test"}"#],
        cfg,
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Failed);
    assert!(events.iter().any(
        |e| matches!(e, AgentEvent::Error { message, .. } if message.contains("couldn't complete"))
    ));
    assert!(!session.messages.iter().any(|m| m.role == Role::Assistant));
}

#[tokio::test]
async fn disabled_web_tools_are_never_executed_even_if_model_calls_them() {
    let fixture = Fixture::new("web-off");
    let mut session = Session::new();
    let (result, events) = run_turn(
        &fixture,
        vec!["<tool_call><function=web_search><parameter=query>test</parameter></function></tool_call>", "Web is unavailable."],
        config(Effort::Medium, PermissionMode::Auto),
        &mut session,
        None,
    ).await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "web_search")));
    assert!(session
        .messages
        .iter()
        .any(|m| m.role == Role::Tool && m.content.contains("not available")));
}

#[tokio::test]
async fn task_plan_progress_is_recorded() {
    let fixture = Fixture::new("task-plan");
    let mut session = Session::new();
    let open = "<tool_call><function=update_plan><parameter=steps>";
    let close = "</parameter></function></tool_call>";
    let first = format!(
        "{}{}{}",
        open,
        r#"[{"title":"Read notes","status":"running"},{"title":"Report","status":"pending"}]"#,
        close
    );
    let last = format!(
        "{}{}{}",
        open,
        r#"[{"title":"Read notes","status":"done"},{"title":"Report","status":"done"}]"#,
        close
    );
    let script = vec![first.as_str(), last.as_str(), "Done."];
    let (result, events) = run_turn(
        &fixture,
        script,
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(session.plan.len(), 2);
    assert!(session
        .plan
        .iter()
        .all(|s| s.status == cordial_core::session::PlanStatus::Done));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::PlanUpdated { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn completed_plan_is_not_reused_for_a_new_task() {
    use cordial_core::session::{PlanStatus, PlanStep};
    let fixture = Fixture::new("new-task-plan");
    let mut session = Session::new();
    session.plan.push(PlanStep {
        id: 0,
        title: "Old task".into(),
        status: PlanStatus::Done,
        note: String::new(),
    });
    let (result, events) = run_turn(
        &fixture,
        vec![
            "Done.", "Done.", "Done.", "Done.", "Done.", "Done.", "Done.",
        ],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(session.plan.is_empty());
    assert!(session
        .messages
        .last()
        .unwrap()
        .content
        .contains("Task incomplete"));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::PlanUpdated { steps, .. } if steps.is_empty())));
}

#[tokio::test]
async fn task_planning_retries_before_executing_other_tools() {
    let fixture = Fixture::new("task-planning-first");
    let mut session = Session::new();
    let premature = "<tool_call><function=read_file><parameter=path>notes.txt</parameter></function></tool_call>";
    let plan = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Read notes\",\"status\":\"running\"}]</parameter></function></tool_call>";
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Read notes\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let (_, events) = run_turn(
        &fixture,
        vec![premature, plan, done, "Done."],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "read_file")));
    assert!(session
        .plan
        .iter()
        .all(|step| step.status == cordial_core::session::PlanStatus::Done));
}

#[tokio::test]
async fn unfinished_plan_gets_a_reconciliation_opportunity() {
    let fixture = Fixture::new("reconcile-plan");
    let mut session = Session::new();
    let start = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Check work\",\"status\":\"running\"}]</parameter></function></tool_call>";
    let finish = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Check work\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let (result, events) = run_turn(
        &fixture,
        vec![start, "Finished.", finish, "Verified."],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(session
        .plan
        .iter()
        .all(|step| step.status == cordial_core::session::PlanStatus::Done));
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Notice { message } if message == "Continuing the task checklist.")));
}

#[tokio::test]
async fn blocked_steps_are_not_reported_as_task_completion() {
    let fixture = Fixture::new("blocked-task");
    let mut session = Session::new();
    let blocked = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Research\",\"status\":\"blocked\",\"note\":\"Network unavailable\"}]</parameter></function></tool_call>";
    let (result, _) = run_turn(
        &fixture,
        vec![blocked, "Done."],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(session
        .messages
        .last()
        .unwrap()
        .content
        .contains("Task incomplete"));
    assert_eq!(
        session.plan[0].status,
        cordial_core::session::PlanStatus::Blocked
    );
}

#[tokio::test]
async fn task_stops_after_repeated_failed_fetches() {
    let fixture = Fixture::new("failed-fetch-loop");
    let mut session = Session::new();
    let plan = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Research\",\"status\":\"running\"}]</parameter></function></tool_call>";
    let fetch = "<tool_call><function=web_fetch><parameter=url>file://invalid</parameter></function></tool_call>";
    let mut cfg = config(Effort::Task, PermissionMode::Auto);
    cfg.web_enabled = true;
    let (result, events) = run_turn(
        &fixture,
        vec![plan, fetch, fetch, fetch, fetch],
        cfg,
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Failed);
    assert!(session
        .messages
        .last()
        .unwrap()
        .content
        .contains("repeated attempts"));
    assert_eq!(
        events
            .iter()
            .filter(
                |e| matches!(e, AgentEvent::ToolCallFinished { name, .. } if name == "web_fetch")
            )
            .count(),
        3
    );
}

#[tokio::test]
async fn task_context_exhaustion_gives_explicit_incomplete_message() {
    let fixture = Fixture::new("task-overflow");
    let mut session = Session::new();
    let runtime = fixture.runtime(Effort::Task);
    let mut scripted = ScriptedBackend::new(Vec::<String>::new());
    scripted.fail_once(ScriptFailure::Overflow);
    let mut backend = Backend::Scripted(scripted);
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let (sink, _receiver) = EventSink::channel();
    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: config(Effort::Task, PermissionMode::Auto),
        store: None,
    };
    let result = agent
        .run(&mut session, "Research this".into(), vec![])
        .await
        .unwrap();
    assert_eq!(result.stop, StopReason::ContextFull);
    assert!(session
        .messages
        .last()
        .unwrap()
        .content
        .contains("Task incomplete"));
    assert!(session.messages.last().unwrap().content.contains("context"));
}

#[tokio::test]
async fn task_overflow_reduces_prompt_tool_results_without_deleting_history() {
    use cordial_core::session::{PlanStatus, PlanStep};
    let fixture = Fixture::new("compact-task");
    let runtime = fixture.runtime(Effort::Task);
    let mut session = Session::new();
    session.push(Message::tool_result("old", "x".repeat(3000)));
    session.plan.push(PlanStep {
        id: 0,
        title: "Research".into(),
        status: PlanStatus::Running,
        note: String::new(),
    });
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Research\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let mut scripted = ScriptedBackend::new(vec![done, "Done."]);
    scripted.fail_once(ScriptFailure::Overflow);
    let mut backend = Backend::Scripted(scripted);
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let (sink, _receiver) = EventSink::channel();
    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: config(Effort::Task, PermissionMode::Auto),
        store: None,
    };
    let result = agent
        .run(&mut session, "Research this".into(), vec![])
        .await
        .unwrap();
    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(session.messages[0].content.len(), 3000);
    if let Backend::Scripted(scripted) = &backend {
        assert!(scripted.requests[0]
            .prompt
            .contains("[cut: content was longer]"));
    } else {
        panic!("expected scripted backend");
    }
}

#[tokio::test]
async fn repeated_unchanged_checklist_prompts_work_instead_of_looping() {
    let fixture = Fixture::new("unchanged-plan");
    let mut session = Session::new();
    let plan = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Read notes\",\"status\":\"running\"}]</parameter></function></tool_call>";
    let read = "<tool_call><function=read_file><parameter=path>notes.txt</parameter></function></tool_call>";
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Read notes\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let (result, events) = run_turn(
        &fixture,
        vec![plan, plan, plan, read, done, "The notes say Friday."],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(session.plan.len(), 1);
    assert_eq!(
        session.plan[0].status,
        cordial_core::session::PlanStatus::Done
    );
    assert!(session
        .messages
        .iter()
        .any(|message| message.role == Role::Tool && message.content.contains("ship on Friday")));
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Notice { message } if message.contains("checklist has not changed"))));
}

#[tokio::test]
async fn empty_task_responses_are_retried_but_bounded() {
    let fixture = Fixture::new("empty-task");
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Answer\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let mut session = Session::new();
    let (result, events) = run_turn(
        &fixture,
        vec![done, "", "Final answer."],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Notice { message } if message.contains("Retrying an empty task response"))));
    assert!(session
        .messages
        .iter()
        .any(|m| m.role == Role::Assistant && m.content == "Final answer."));

    let mut session = Session::new();
    let (result, _) = run_turn(
        &fixture,
        vec![done, "", "", ""],
        config(Effort::Task, PermissionMode::Auto),
        &mut session,
        None,
    )
    .await;
    assert_eq!(result.stop, StopReason::EmptyAnswer);
}

#[tokio::test]
async fn truncated_task_answer_retries_and_never_claims_completion() {
    use cordial_core::backend::GenStop;
    let fixture = Fixture::new("truncated-task");
    let runtime = fixture.runtime(Effort::Task);
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Answer\",\"status\":\"done\"}]</parameter></function></tool_call>";
    for (replies, stops, expected) in [
        (
            vec![done, "Incomplete ans", "Complete answer."],
            vec![GenStop::EndOfTurn, GenStop::MaxTokens, GenStop::EndOfTurn],
            StopReason::Answered,
        ),
        (
            vec![done, "Incomplete ans", "Still incom"],
            vec![GenStop::EndOfTurn, GenStop::MaxTokens, GenStop::MaxTokens],
            StopReason::Failed,
        ),
    ] {
        let mut session = Session::new();
        let mut backend = Backend::Scripted(ScriptedBackend::new(replies).with_stops(stops));
        let gate = ApprovalGate::new();
        let mut policy = PermissionPolicy::new(PermissionMode::Auto);
        let (sink, _receiver) = EventSink::channel();
        let mut agent = Agent {
            backend: &mut backend,
            tools: &runtime,
            policy: &mut policy,
            approvals: &gate,
            events: &sink,
            config: config(Effort::Task, PermissionMode::Auto),
            store: None,
        };
        let result = agent
            .run(&mut session, "Answer the task".into(), vec![])
            .await
            .unwrap();
        assert_eq!(result.stop, expected);
        let answer = &session.messages.last().unwrap().content;
        if expected == StopReason::Failed {
            assert!(answer.starts_with("Task incomplete: the response was cut off"));
        } else {
            assert_eq!(answer, "Complete answer.");
        }
    }
}

#[tokio::test]
async fn overflow_drops_tool_screenshots_from_prompt_but_keeps_user_images_and_history() {
    use cordial_core::session::{PlanStatus, PlanStep};
    let fixture = Fixture::new("overflow-tool-images");
    let runtime = fixture.runtime(Effort::Task);
    let mut session = Session::new();
    let mut screenshot = Message::tool_result("old", "Page text remains available.");
    screenshot.attachments.push(Attachment {
        kind: AttachmentKind::Image,
        name: "screenshot.png".into(),
        path: fixture
            .workspace
            .join("screenshot.png")
            .display()
            .to_string(),
    });
    session.push(screenshot);
    session.plan.push(PlanStep {
        id: 0,
        title: "Answer".into(),
        status: PlanStatus::Running,
        note: String::new(),
    });
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Answer\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let mut scripted = ScriptedBackend::new(vec![done, "Done."]);
    scripted.fail_once(ScriptFailure::Overflow);
    let mut backend = Backend::Scripted(scripted);
    let gate = ApprovalGate::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let (sink, _receiver) = EventSink::channel();
    let mut agent = Agent {
        backend: &mut backend,
        tools: &runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config: config(Effort::Task, PermissionMode::Auto),
        store: None,
    };
    let user_image = fixture.workspace.join("image.png");
    let result = agent
        .run(
            &mut session,
            "Use my image".into(),
            vec![Attachment {
                kind: AttachmentKind::Image,
                name: "image.png".into(),
                path: user_image.display().to_string(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(result.stop, StopReason::Answered);
    assert_eq!(session.messages[0].attachments.len(), 1);
    if let Backend::Scripted(scripted) = &backend {
        assert!(scripted
            .requests
            .iter()
            .all(|request| request.media == vec![user_image.clone()]));
        assert!(scripted.requests[0]
            .prompt
            .contains("Page text remains available"));
    } else {
        panic!("expected scripted backend");
    }
}

#[tokio::test]
async fn repeated_fetch_nudges_task_to_work_before_aborting() {
    let fixture = Fixture::new("fetch-progress");
    let mut session = Session::new();
    let plan = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Read notes\",\"status\":\"running\"}]</parameter></function></tool_call>";
    let fetch = "<tool_call><function=web_fetch><parameter=url>file://invalid</parameter></function></tool_call>";
    let read = "<tool_call><function=read_file><parameter=path>notes.txt</parameter></function></tool_call>";
    let done = "<tool_call><function=update_plan><parameter=steps>[{\"title\":\"Read notes\",\"status\":\"done\"}]</parameter></function></tool_call>";
    let mut cfg = config(Effort::Task, PermissionMode::Auto);
    cfg.web_enabled = true;
    let (result, events) = run_turn(&fixture, vec![plan, fetch, fetch, read, done, "The notes say Friday."], cfg, &mut session, None).await;
    assert_eq!(result.stop, StopReason::Answered);
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Notice { message } if message.contains("Avoiding a repeated source fetch"))));
    assert!(session.messages.iter().any(|m| m.role == Role::Tool && m.content.contains("ship on Friday")));
}
