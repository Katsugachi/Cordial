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
use std::sync::atomic::AtomicBool;
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

        let manager = ModelManager::new(&model_dir).ok()?;
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
        let (backend, ready) = LlamaBackend::spawn(options, sink.clone(), cancel).ok()?;
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
        .ok()?;

        Some(Self {
            backend: Backend::Llama(backend),
            runtime,
            workspace: workspace.path().to_path_buf(),
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
        let drain = tokio::spawn(async move {
            let mut text = String::new();
            let mut diagnostics: Vec<String> = Vec::new();
            while let Some(event) = receiver.recv().await {
                match event {
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
                    } => diagnostics.push(format!("tool {name} ok={ok}: {summary}")),
                    _ => {}
                }
            }
            (text, diagnostics)
        });

        let config = AgentConfig {
            effort,
            permission_mode: PermissionMode::Auto,
            web_enabled,
            thinking: effort != Effort::Low,
            max_output_tokens: 1_536,
            max_tool_rounds: effort.max_tool_rounds(),
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
        };
        let result = agent.run(&mut session, text.to_string(), attachments).await;
        drop(agent);
        drop(sink);
        let (streamed, diagnostics) = drain.await.unwrap_or_default();
        for line in &diagnostics {
            eprintln!("  · {line}");
        }
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
