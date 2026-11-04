//! Behavioural tests against the real LFM3-VL-2B weights.
//!
//! These are `#[ignore]`d because they download ~1.3 GB and run inference. Run
//! them with:
//!
//! ```bash
//! CORDIAL_MODEL_DIR=~/Library/Application\ Support/Cordial/models \
//!   cargo test -p cordial-core --test live_model -- --ignored --nocapture
//! ```
//!
//! They assert on behaviour a reviewer cares about: the model answers the
//! question, does not invent facts it cannot check, uses tools when it should,
//! keeps answers short, and can see an image.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cordial_core::backend::{engine_options, Backend, LlamaBackend};
use cordial_core::events::EventSink;
use cordial_core::permissions::PermissionPolicy;
use cordial_core::session::{Attachment, AttachmentKind, Role, Session};
use cordial_core::settings::{Effort, PermissionMode};
use cordial_core::tools::ToolRuntime;
use cordial_core::{Agent, AgentConfig, ApprovalGate, ModelManager};
use cordial_llama::SamplingParams;

struct Live {
    backend: Backend,
    runtime: ToolRuntime,
    workspace: PathBuf,
    max_tool_rounds_override: Option<u32>,
    last_diagnostics: Vec<String>,
    // Kept alive for the whole test so the workspace and scratch folders are
    // not deleted while the agent is still using them.
    _workspace_dir: tempdir::TempDir,
    _scratch: tempdir::TempDir,
}

mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}

impl Live {
    /// Loads the model. Returns `None` only when no model was requested at all;
    /// when `CORDIAL_MODEL_DIR` is pinned (as CI does) a missing or unloadable
    /// model is a failure, so a broken release cannot pass by skipping.
    async fn boot() -> Option<Self> {
        let pinned = std::env::var("CORDIAL_MODEL_DIR").is_ok();
        let model_dir = std::env::var("CORDIAL_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                cordial_core::Paths::discover()
                    .map(|paths| paths.models_dir())
                    .unwrap_or_else(|_| PathBuf::from("models"))
            });

        let manager = ModelManager::new(&model_dir).expect("create model manager");
        let (sink, mut receiver) = EventSink::channel();
        let sink_task = tokio::spawn(async move { while receiver.recv().await.is_some() {} });

        let paths = match manager.ensure_defaults(&sink).await {
            Ok(paths) => paths,
            Err(err) if pinned => {
                sink_task.abort();
                eprintln!("MODEL BOOT FAILED: {err}");
                panic!("CORDIAL_MODEL_DIR is set but the model is unusable: {err}");
            }
            Err(err) => {
                eprintln!("skipping live tests (no CORDIAL_MODEL_DIR): {err}");
                sink_task.abort();
                return None;
            }
        };

        let options = engine_options(
            &paths.weights,
            paths.projector.as_deref(),
            8_192,
            (std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4) as i32)
                - 1,
            SamplingParams::default(),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let (backend, ready) =
            LlamaBackend::spawn(options, sink.clone(), cancel).expect("start inference worker");
        match ready.await {
            Ok(Ok(info)) => eprintln!(
                "loaded {} on {} ({}k context, vision: {})",
                info.description,
                info.backend,
                info.context_tokens / 1024,
                info.vision
            ),
            Ok(Err(err)) if pinned => {
                sink_task.abort();
                eprintln!("MODEL LOAD FAILED: {err}");
                panic!("CORDIAL_MODEL_DIR is set but the model did not load: {err}");
            }
            Ok(Err(err)) => {
                eprintln!("skipping live tests, model did not load: {err}");
                sink_task.abort();
                return None;
            }
            Err(err) if pinned => {
                panic!("inference worker failed to report readiness: {err}");
            }
            Err(err) => {
                eprintln!("skipping live tests: {err}");
                sink_task.abort();
                return None;
            }
        }

        let workspace = tempdir::TempDir::new("cordial-live-workspace");
        std::fs::write(
            workspace.path().join("notes.txt"),
            "Project Meridian ships on the 14th of November. Owner: Sam. Budget: 42,000 AUD.",
        )
        .unwrap();
        let scratch = tempdir::TempDir::new("cordial-live-scratch");
        let runtime = ToolRuntime::new(
            workspace.path().to_path_buf(),
            scratch.path().to_path_buf(),
            Effort::Medium,
        )
        .expect("create live tool runtime");

        Some(Self {
            backend: Backend::Llama(backend),
            runtime,
            workspace: workspace.path().to_path_buf(),
            max_tool_rounds_override: None,
            last_diagnostics: Vec::new(),
            _workspace_dir: workspace,
            _scratch: scratch,
        })
    }

    async fn turn(
        &mut self,
        effort: Effort,
        web_enabled: bool,
        text: &str,
        attachments: Vec<Attachment>,
    ) -> (Session, String) {
        let mut session = Session::new();
        let gate = ApprovalGate::new();
        let mut policy = PermissionPolicy::new(PermissionMode::Auto);
        let (sink, mut receiver) = EventSink::channel();
        // Only the test harness auto-approves writes in its disposable fixture.
        let test_gate = gate.clone();
        let drain = tokio::spawn(async move {
            let mut text = String::new();
            let mut diagnostics: Vec<String> = Vec::new();
            while let Some(event) = receiver.recv().await {
                match event {
                    cordial_core::AgentEvent::ToolCallNeedsApproval { call_id, .. } => {
                        test_gate.resolve(&call_id, true).await;
                    }
                    cordial_core::AgentEvent::TextDelta { text: delta, .. } => {
                        text.push_str(&delta)
                    }
                    cordial_core::AgentEvent::Error { message, .. } => {
                        diagnostics.push(format!("error: {message}"))
                    }
                    cordial_core::AgentEvent::Notice { message } => {
                        diagnostics.push(format!("notice: {message}"))
                    }
                    cordial_core::AgentEvent::ToolCallFinished {
                        name, summary, ok, ..
                    } => {
                        eprintln!("EVAL progress tool {name} ok={ok}: {summary}");
                        diagnostics.push(format!("tool {name} ok={ok}: {summary}"));
                    }
                    _ => {}
                }
            }
            (text, diagnostics)
        });

        let config = AgentConfig {
            effort,
            permission_mode: PermissionMode::Auto,
            web_enabled,
            memory: String::new(),
            skill: String::new(),
            thinking: effort != Effort::Low,
            max_output_tokens: 1_536,
            max_tool_rounds: self
                .max_tool_rounds_override
                .unwrap_or_else(|| effort.max_tool_rounds()),
            max_tool_result_chars: 6_000,
            keep_image_turns: 2,
            sampling: SamplingParams {
                temperature: effort.temperature(),
                ..SamplingParams::default()
            },
        };

        let mut agent = Agent {
            backend: &mut self.backend,
            tools: &self.runtime,
            policy: &mut policy,
            approvals: &gate,
            events: &sink,
            config,
            store: None,
        };
        let result = agent.run(&mut session, text.to_string(), attachments).await;
        drop(agent);
        drop(sink);
        let (streamed, diagnostics) = drain.await.unwrap_or_default();
        for line in &diagnostics {
            eprintln!("  · {line}");
        }
        self.last_diagnostics = diagnostics;
        let stop = result
            .map(|r| format!("{:?}", r.stop))
            .unwrap_or_else(|e| format!("error: {e}"));

        let answer = session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == Role::Assistant)
            .map(|message| message.content.clone())
            .unwrap_or_default();

        eprintln!(
            "\n--- {text}\n({stop}, {} words)\n{answer}\n---\n",
            answer.split_whitespace().count()
        );
        if answer.is_empty() {
            eprintln!("(streamed {streamed:?})");
        }
        (session, answer)
    }
}

/// A generated, offline evaluation. The model sees fixture values only at run
/// time; assertions inspect the actual artifacts rather than the model's claim.
#[tokio::test]
#[ignore = "runs a generated multi-step task against the real model"]
async fn generated_complex_tasks_verify_artifacts() {
    use cordial_core::session::{ToolArgument, ToolCall};
    let Some(mut live) = Live::boot().await else {
        panic!("model unavailable")
    };
    let seed = (cordial_core::util::now_ms() as u64) % 9;
    let a = 11 + seed;
    let b = 23 + seed;
    let c = 37 + seed;
    std::fs::write(
        live.workspace.join("orders.csv"),
        format!("item,units,price\nA,{a},2\nB,{b},3\nC,{c},4\n"),
    )
    .unwrap();
    let (session, answer) = live.turn(Effort::Task, false,
        "Read orders.csv. Calculate units times price for each row and the grand total. Write a report to totals.txt containing each item and its total, plus the grand total. Verify the file and tell me when finished.", vec![]).await;
    let artifact = std::fs::read_to_string(live.workspace.join("totals.txt")).unwrap_or_default();
    let expected = [a * 2, b * 3, c * 4, a * 2 + b * 3 + c * 4];
    let missing: Vec<_> = expected
        .iter()
        .filter(|v| !artifact.contains(&v.to_string()))
        .collect();
    eprintln!("EVAL csv artifact={artifact:?}, expected={expected:?}, missing={missing:?}, plan={:?}, answer={answer:?}", session.plan);

    let call = ToolCall { id: "fixture".into(), name: "docx_create".into(), arguments: vec![
        ToolArgument { name: "path".into(), value: "worksheet.docx".into() },
        ToolArgument { name: "content".into(), value: format!("Question 1: What is {a} plus {b}?\nAnswer: blank\nQuestion 2: What is {c} minus {a}?\nAnswer: blank") },
    ]};
    assert!(
        live.runtime.execute(&call).await.ok,
        "could not create DOCX fixture"
    );
    let (word_session, word_answer) = live.turn(Effort::Task, false,
        "Read worksheet.docx, answer both questions and save a completed Word document in the workspace. Verify the output and report its filename.", vec![]).await;
    let mut found = false;
    for entry in std::fs::read_dir(&live.workspace).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("docx")
            || path.ends_with("worksheet.docx")
        {
            continue;
        }
        let call = ToolCall {
            id: "verify".into(),
            name: "docx_read".into(),
            arguments: vec![ToolArgument {
                name: "path".into(),
                value: entry.file_name().to_string_lossy().into_owned(),
            }],
        };
        let out = live.runtime.execute(&call).await;
        eprintln!(
            "EVAL docx {} ok={} text={:?}",
            path.display(),
            out.ok,
            out.text
        );
        if out.ok
            && out.text.contains(&(a + b).to_string())
            && out.text.contains(&(c - a).to_string())
        {
            found = true;
        }
    }
    eprintln!(
        "EVAL docx expected={} and {}, found={found}, plan={:?}, answer={word_answer:?}",
        a + b,
        c - a,
        word_session.plan
    );
    // Cross-check with a separate implementation using Python's standard ZIP/XML
    // parser instead of trusting the application's own DOCX reader.
    let verifier =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ci/verify_task_artifacts.py");
    let check = std::process::Command::new("python3")
        .arg(verifier)
        .arg(&live.workspace)
        .args([a.to_string(), b.to_string(), c.to_string()])
        .output()
        .expect("independent artifact verification requires python3");
    eprintln!(
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(
        check.status.success(),
        "independent artifact verification failed"
    );
    assert!(
        missing.is_empty(),
        "CSV artifact missing expected numbers: {missing:?}"
    );
    assert!(found, "no completed DOCX artifact contained both answers");
    for (label, task) in [("CSV", &session), ("DOCX", &word_session)] {
        assert!(
            !task.plan.is_empty(),
            "{label} task never created a checklist"
        );
        assert!(
            task.plan
                .iter()
                .all(|step| step.status == cordial_core::session::PlanStatus::Done),
            "{label} task did not finish its checklist: {:?}",
            task.plan
        );
        let updates = task
            .messages
            .iter()
            .flat_map(|message| &message.tool_calls)
            .filter(|call| call.name == "update_plan")
            .count();
        assert!(
            updates >= 2,
            "{label} task did not show checklist progression"
        );
    }
}

/// Research-oriented smoke tests in two subjects. These do not replace a
/// held-out benchmark or establish correctness for arbitrary worksheets.
#[tokio::test]
#[ignore = "uses the real model and live web search"]
async fn research_worksheets_architecture_and_english() {
    let Some(mut live) = Live::boot().await else {
        panic!("model unavailable")
    };
    let seed = cordial_core::util::now_ms() % 5;
    let width = 12 + seed;
    let length = 19 + seed;
    std::fs::write(live.workspace.join("architecture.txt"), format!(
        "Architecture worksheet\n1. Research who designed the Sydney Opera House and in what year it opened. Cite a source.\n2. A rectangular gallery measures {length} metres by {width} metres. Calculate its floor area and wall perimeter, showing the method.\n3. Explain one difference between floor area and perimeter.\n"
    )).unwrap();
    let (architecture, arch_answer) = live.turn(Effort::Task, true,
        "Complete every question in architecture.txt. Research factual claims, cite source URLs, and explain each answer.", vec![]).await;
    std::fs::write(live.workspace.join("english.txt"),
        "English worksheet\n1. In the sentence 'The wind whispered secrets to the street', identify and explain the figure of speech.\n2. In 'The library was a lighthouse in the storm', identify and explain the figure of speech.\n3. Research who wrote Sonnet 18 and the year it was first published. Cite a source.\n").unwrap();
    let (english, english_answer) = live.turn(Effort::Task, true,
        "Complete every question in english.txt. Research factual claims, cite source URLs, and explain each answer.", vec![]).await;
    let names = |s: &Session| -> Vec<String> {
        s.messages
            .iter()
            .flat_map(|m| &m.tool_calls)
            .map(|c| c.name.clone())
            .collect()
    };
    let arch_calls = names(&architecture);
    let english_calls = names(&english);
    eprintln!("EVAL architecture answer={arch_answer:?}, plan={:?}, calls={arch_calls:?}, area={}, perimeter={}", architecture.plan, length * width, 2 * (length + width));
    eprintln!(
        "EVAL english answer={english_answer:?}, plan={:?}, calls={english_calls:?}",
        english.plan
    );
    for (label, session, calls, answer) in [
        ("architecture", &architecture, &arch_calls, &arch_answer),
        ("english", &english, &english_calls, &english_answer),
    ] {
        assert!(
            calls.iter().any(|c| c == "web_search") && calls.iter().any(|c| c == "web_fetch"),
            "{label}: did not search and open a source"
        );
        let cited_and_fetched = session
            .messages
            .iter()
            .flat_map(|m| &m.tool_calls)
            .filter(|c| c.name == "web_fetch")
            .any(|call| {
                call.arg("url").is_some_and(|url| {
                    answer.contains(url)
                        && session.messages.iter().any(|m| {
                            m.tool_call_id.as_deref() == Some(call.id.as_str())
                                && m.content.starts_with("Text of ")
                        })
                })
            });
        assert!(
            cited_and_fetched,
            "{label}: no cited source was successfully opened"
        );
        assert!(
            !session.plan.is_empty()
                && session
                    .plan
                    .iter()
                    .all(|s| s.status == cordial_core::session::PlanStatus::Done),
            "{label}: unfinished checklist"
        );
    }
    let arch = arch_answer.to_lowercase();
    assert!(
        arch.contains("utzon")
            && arch.contains("1973")
            && arch.contains(&(length * width).to_string())
            && arch.contains(&(2 * (length + width)).to_string()),
        "architecture answers did not match the independent key"
    );
    let eng = english_answer.to_lowercase();
    assert!(
        eng.contains("personification")
            && eng.contains("metaphor")
            && eng.contains("shakespeare")
            && eng.contains("1609"),
        "English answers did not match the independent key"
    );
}

/// The real worksheet after the same PDF.js text-and-image preparation used by
/// the desktop UI. Fixture paths are supplied outside the repository's HEAD.
#[tokio::test]
#[ignore = "requires rendered user worksheet and real model"]
async fn rendered_pdf_worksheet_end_to_end() {
    let Ok(image) = std::env::var("CORDIAL_PDF_PAGE_IMAGE") else {
        eprintln!("worksheet render not supplied; skipping optional test");
        return;
    };
    let text_path = std::env::var("CORDIAL_PDF_PAGE_TEXT").expect("rendered PDF text required");
    let Some(mut live) = Live::boot().await else {
        panic!("model unavailable")
    };
    // A bounded evaluation should produce diagnostics even if the model loops.
    // Production Task mode retains its larger independent round budget.
    live.max_tool_rounds_override = Some(12);
    let (session, answer) = live.turn(Effort::Task, false,
        "Complete the attached worksheet. Show reasoning for each answer. Use Python for calculations and clean up any temporary scripts.",
        vec![Attachment { kind: AttachmentKind::Image, name: "worksheet.pdf, page 1 (image)".into(), path: image },
             Attachment { kind: AttachmentKind::Text, name: "worksheet.pdf, page 1 (text)".into(), path: text_path }]).await;
    let calls: Vec<_> = session
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|c| c.name.as_str())
        .collect();
    fn contains_script(dir: &Path) -> std::io::Result<bool> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() && contains_script(&entry.path())? {
                return Ok(true);
            }
            if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    let leftover_scripts =
        contains_script(&live.workspace).unwrap() || contains_script(live._scratch.path()).unwrap();
    eprintln!("EVAL rendered worksheet answer={answer:?}, plan={:?}, tools={calls:?}, leftover_scripts={leftover_scripts}", session.plan);
    let numbers: Vec<&str> = regex::Regex::new(r"[+-]?[0-9]+(?:\.[0-9]+)?")
        .unwrap()
        .find_iter(&answer)
        .map(|m| m.as_str())
        .collect();
    // Questions 1 and 3 explicitly require two and three decimal places.
    assert!(
        numbers.contains(&"75.40") && numbers.contains(&"98.534"),
        "answers did not match the required values and precision: {numbers:?}"
    );
    assert!(
        numbers
            .iter()
            .filter_map(|n| n.parse::<f64>().ok())
            .any(|actual| (actual - 47.16990566028302).abs() < 0.005),
        "the independently calculated box diagonal was missing: {numbers:?}"
    );
    let lower = answer.to_ascii_lowercase();
    assert!(
        regex::Regex::new(r"\b(yes|will fit|can fit|fits inside|fits in|does fit)\b")
            .unwrap()
            .is_match(&lower),
        "the rod-fit conclusion was missing"
    );
    assert!(
        !regex::Regex::new(
            r"\b(does not fit|doesn't fit|will not fit|won't fit|cannot fit|can't fit)\b"
        )
        .unwrap()
        .is_match(&lower),
        "the answer contradicts the rod-fit conclusion"
    );
    assert!(
        !session.plan.is_empty()
            && session
                .plan
                .iter()
                .all(|s| s.status == cordial_core::session::PlanStatus::Done),
        "worksheet checklist incomplete"
    );
    assert!(
        calls
            .iter()
            .filter(|name| name.contains("python"))
            .any(|name| {
                let prefix = format!("tool {name} ok=true:");
                live.last_diagnostics
                    .iter()
                    .any(|line| line.starts_with(&prefix))
            }),
        "Python was not successfully executed"
    );
    assert!(
        !leftover_scripts,
        "a temporary Python script was left behind"
    );
}

/// Optional exact-file evaluation. The PDF is supplied by the test operator,
/// not embedded in the repository permanently. This exercises the ordinary
/// attachment path, without injecting pre-extracted text or answers.
#[tokio::test]
#[ignore = "requires an explicitly supplied worksheet and real model"]
async fn user_pdf_worksheet_end_to_end() {
    let Ok(path) = std::env::var("CORDIAL_WORKSHEET_PDF") else {
        eprintln!("worksheet fixture not supplied; skipping optional test");
        return;
    };
    let Some(mut live) = Live::boot().await else {
        panic!("model unavailable")
    };
    let (session, answer) = live.turn(Effort::Task, false,
        "Please complete the attached worksheet, show your reasoning for each answer, and use Python for calculations. Clean up any temporary scripts when finished.",
        vec![Attachment { kind: AttachmentKind::Text, name: "worksheet.pdf".into(), path }]).await;
    let calls: Vec<_> = session
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|c| c.name.as_str())
        .collect();
    eprintln!(
        "EVAL exact worksheet answer={answer:?}, plan={:?}, tools={calls:?}",
        session.plan
    );
    assert!(
        answer.contains("75.40") && answer.contains("47.17") && answer.contains("98.534"),
        "worksheet answers did not match independent calculations"
    );
    assert!(
        !session.plan.is_empty()
            && session
                .plan
                .iter()
                .all(|s| s.status == cordial_core::session::PlanStatus::Done),
        "worksheet task did not complete a checklist"
    );
    assert!(
        calls.iter().any(|name| name.contains("python")),
        "worksheet task did not run Python"
    );
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    let lower = haystack.to_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn assert_no_filler(answer: &str) {
    let lower = answer.to_lowercase();
    for bad in [
        "as an ai language model",
        "i hope this helps",
        "great question",
        "let me know if you",
        "in conclusion",
    ] {
        assert!(!lower.contains(bad), "answer used filler {bad:?}: {answer}");
    }
}

fn assert_helpful(answer: &str, max_words: usize) {
    assert!(!answer.trim().is_empty(), "the model produced no answer");
    assert!(
        words(answer) <= max_words,
        "answer was {} words (limit {max_words}):\n{answer}",
        words(answer)
    );
    assert_no_filler(answer);
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn answers_a_plain_factual_question_briefly() {
    let Some(mut live) = Live::boot().await else {
        eprintln!("no model available: live assertions were not run");
        return;
    };
    let (_, answer) = live
        .turn(
            Effort::Medium,
            false,
            "What is the capital of Australia?",
            Vec::new(),
        )
        .await;
    assert_helpful(&answer, 80);
    assert!(
        contains_any(&answer, &["canberra"]),
        "expected Canberra in the answer, got: {answer}"
    );
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn reasons_through_a_short_arithmetic_question() {
    let Some(mut live) = Live::boot().await else {
        eprintln!("no model available: live assertions were not run");
        return;
    };
    let (_, answer) = live
        .turn(
            Effort::Medium,
            false,
            "A train leaves at 09:40 and the trip takes 1 hour 25 minutes. What time does it arrive?",
            Vec::new(),
        )
        .await;
    assert_helpful(&answer, 90);
    assert!(
        contains_any(&answer, &["11:05", "11.05", "11 05"]),
        "expected 11:05, got: {answer}"
    );
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn uses_the_file_tool_and_quotes_the_workspace() {
    let Some(mut live) = Live::boot().await else {
        eprintln!("no model available: live assertions were not run");
        return;
    };
    let (session, answer) = live
        .turn(
            Effort::Medium,
            false,
            "Read notes.txt in my workspace and tell me when the project ships.",
            Vec::new(),
        )
        .await;
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.role == Role::Tool && message.content.contains("Meridian")),
        "the model never read the file"
    );
    assert_helpful(&answer, 90);
    assert!(
        contains_any(&answer, &["14", "fourteenth", "november"]),
        "expected the ship date, got: {answer}"
    );
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn does_not_invent_facts_when_the_web_is_off() {
    let Some(mut live) = Live::boot().await else {
        eprintln!("no model available: live assertions were not run");
        return;
    };
    let (_, answer) = live
        .turn(
            Effort::Low,
            false,
            "What is the share price of Apple right now?",
            Vec::new(),
        )
        .await;
    assert_helpful(&answer, 70);
    assert!(
        contains_any(
            &answer,
            &[
                "cannot",
                "can't",
                "unable",
                "no access",
                "don't know",
                "do not know",
                "offline",
                "check"
            ]
        ),
        "expected honesty about not being able to check, got: {answer}"
    );
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn low_effort_answers_in_one_pass() {
    let Some(mut live) = Live::boot().await else {
        eprintln!("no model available: live assertions were not run");
        return;
    };
    let (_, answer) = live
        .turn(
            Effort::Low,
            false,
            "Name the three primary colours.",
            Vec::new(),
        )
        .await;
    assert_helpful(&answer, 40);
    let hits = ["red", "blue", "yellow"]
        .iter()
        .filter(|colour| answer.to_lowercase().contains(*colour))
        .count();
    assert_eq!(hits, 3, "expected red, blue and yellow, got: {answer}");
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn sees_the_colour_of_an_attached_image() {
    let Some(mut live) = Live::boot().await else {
        eprintln!("no model available: live assertions were not run");
        return;
    };

    // A 64x64 solid red PNG, written by hand so the test needs no image crates.
    let png = write_solid_png(64, 64, [220, 40, 40]);
    let image_path = live.workspace.join("swatch.png");
    std::fs::write(&image_path, png).unwrap();

    let (_, answer) = live
        .turn(
            Effort::Medium,
            false,
            "What colour is this image? Answer in one word.",
            vec![Attachment {
                kind: AttachmentKind::Image,
                name: "swatch.png".into(),
                path: image_path.display().to_string(),
            }],
        )
        .await;
    assert!(!answer.trim().is_empty(), "no answer for the image");
    assert!(
        contains_any(&answer, &["red", "crimson", "scarlet"]),
        "expected the model to see red, got: {answer}"
    );
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn latest_release_notes_attempts_a_real_search() {
    let Some(mut live) = Live::boot().await else {
        panic!("model unavailable")
    };
    let (session, answer) = live
        .turn(
            Effort::Medium,
            true,
            "Summarise the latest release notes for llama.cpp",
            vec![],
        )
        .await;
    assert!(
        session
            .messages
            .iter()
            .flat_map(|m| &m.tool_calls)
            .any(|c| c.name == "web_search"),
        "expected a search, got {answer:?}"
    );
    assert!(
        !answer.trim_start().starts_with("{\"query\""),
        "raw arguments were shown: {answer}"
    );
    assert!(!answer.trim().is_empty(), "no answer after search");
}

#[tokio::test]
#[ignore = "downloads the model and runs inference"]
async fn cancellation_does_not_break_the_next_turn() {
    let Some(mut live) = Live::boot().await else {
        panic!("model unavailable")
    };
    let cancel = live.backend.cancel_flag().expect("cancellation flag");
    cancel.store(false, Ordering::Relaxed);
    let stop_flag = Arc::clone(&cancel);
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stop_flag.store(true, Ordering::Relaxed);
    });
    let mut session = Session::new();
    let mut policy = PermissionPolicy::new(PermissionMode::Auto);
    let gate = ApprovalGate::new();
    let sink = EventSink::silent();
    let mut config = AgentConfig::from_settings(&cordial_core::settings::Settings::default());
    config.web_enabled = false;
    let mut agent = Agent {
        backend: &mut live.backend,
        tools: &live.runtime,
        policy: &mut policy,
        approvals: &gate,
        events: &sink,
        config,
        store: None,
    };
    let result = agent
        .run(
            &mut session,
            "Explain how a telescope works in detail.".into(),
            vec![],
        )
        .await
        .unwrap();
    stopper.await.unwrap();
    assert_eq!(result.stop, cordial_core::events::StopReason::Cancelled);
    drop(agent);
    cancel.store(false, Ordering::Relaxed);
    let (_, answer) = live
        .turn(
            Effort::Low,
            false,
            "What is the capital of Australia?",
            vec![],
        )
        .await;
    assert!(
        answer.to_lowercase().contains("canberra"),
        "next turn failed: {answer:?}"
    );
}

/// Minimal PNG encoder: solid RGB, no filtering, deflate stored blocks.
fn write_solid_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            *entry = value;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc = table[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }

    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(body.len() + 12);
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc_input = Vec::with_capacity(body.len() + 4);
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(body);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        out
    }

    // Raw scanlines: filter byte 0 then RGB triples.
    let mut raw = Vec::with_capacity(((width * 3 + 1) * height) as usize);
    for _ in 0..height {
        raw.push(0u8);
        for _ in 0..width {
            raw.extend_from_slice(&rgb);
        }
    }

    // zlib stream with stored (uncompressed) deflate blocks.
    let mut zlib = vec![0x78, 0x01];
    let mut offset = 0usize;
    while offset < raw.len() {
        let take = (raw.len() - offset).min(65_535);
        let last = offset + take == raw.len();
        zlib.push(if last { 1 } else { 0 });
        zlib.extend_from_slice(&(take as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(take as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[offset..offset + take]);
        offset += take;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    let adler = (b << 16) | a;
    zlib.extend_from_slice(&adler.to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour, no interlace

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &zlib));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

#[test]
fn png_encoder_produces_a_valid_header() {
    let png = write_solid_png(2, 2, [1, 2, 3]);
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    assert_eq!(&png[12..16], b"IHDR");
    assert_eq!(&png[16..20], &2u32.to_be_bytes());
    let path = std::env::temp_dir().join("cordial-swatch.png");
    std::fs::write(&path, &png).unwrap();
    assert!(Path::new(&path).is_file());
    std::fs::remove_file(&path).ok();
}
