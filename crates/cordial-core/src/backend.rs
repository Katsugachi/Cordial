//! Inference backends.
//!
//! [`LlamaBackend`] owns the model on a dedicated thread: llama.cpp contexts are
//! not `Sync`, and inference must never block the async runtime that is also
//! serving the UI. Requests go in over a channel, tokens come back as events.
//!
//! [`ScriptedBackend`] replays canned replies so the whole harness — prompt
//! assembly, tool calls, permissions, trimming — is testable without weights.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cordial_llama::{Context, ContextOptions, LlamaError, Model, Mtmd, Sampler, SamplingParams};

use crate::error::CoreError;
use crate::events::{AgentEvent, EventSink};
use crate::prompt::THINK_CLOSE;

/// Why generation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenStop {
    /// The model emitted an end-of-turn token.
    EndOfTurn,
    /// The token budget ran out.
    MaxTokens,
    /// The user cancelled.
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub turn_id: String,
    pub prompt: String,
    pub media: Vec<PathBuf>,
    pub max_tokens: u32,
    pub thinking: bool,
    pub sampling: SamplingParams,
}

#[derive(Debug, Clone)]
pub struct GenerateOutput {
    pub text: String,
    pub stop: GenStop,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub tokens_per_second: f32,
}

/// Tokens streamed out while generating.
#[derive(Debug, Clone)]
pub enum Delta {
    Thinking(String),
    Text(String),
}

enum WorkerCommand {
    Generate {
        request: GenerateRequest,
        deltas: tokio::sync::mpsc::UnboundedSender<Delta>,
        reply: tokio::sync::oneshot::Sender<Result<GenerateOutput, CoreError>>,
    },
    Shutdown,
}

/// What the worker reports once the model is loaded.
#[derive(Debug, Clone)]
pub struct EngineInfo {
    pub description: String,
    pub context_tokens: u32,
    pub vision: bool,
    pub backend: String,
}

/// How the model was launched.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    pub model_path: PathBuf,
    pub mmproj_path: Option<PathBuf>,
    pub context_tokens: u32,
    pub threads: i32,
    pub sampling: SamplingParams,
}

/// Handle to the inference worker thread.
pub struct LlamaBackend {
    sender: std::sync::mpsc::Sender<WorkerCommand>,
    cancel: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LlamaBackend {
    /// Starts the worker. Model loading happens on the worker thread; the
    /// returned receiver resolves once the model is ready (or has failed).
    pub fn spawn(
        options: EngineOptions,
        events: EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<
        (
            Self,
            tokio::sync::oneshot::Receiver<Result<EngineInfo, CoreError>>,
        ),
        CoreError,
    > {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let cancel_for_worker = Arc::clone(&cancel);

        let worker = std::thread::Builder::new()
            .name("cordial-inference".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                worker_main(options, events, cancel_for_worker, command_rx, ready_tx);
            })
            .map_err(|err| {
                CoreError::Inference(format!("could not start inference thread: {err}"))
            })?;

        Ok((
            Self {
                sender: command_tx,
                cancel,
                worker: Some(worker),
            },
            ready_rx,
        ))
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    pub async fn generate(
        &mut self,
        request: GenerateRequest,
        events: &EventSink,
    ) -> Result<GenerateOutput, CoreError> {
        let turn_id = request.turn_id.clone();
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel();
        let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();

        self.sender
            .send(WorkerCommand::Generate {
                request,
                deltas: delta_tx,
                reply: reply_tx,
            })
            .map_err(|_| CoreError::Inference("the inference worker has stopped".to_string()))?;

        let mut deltas_closed = false;
        loop {
            tokio::select! {
                delta = delta_rx.recv(), if !deltas_closed => {
                    match delta {
                        Some(Delta::Thinking(text)) => events.emit(AgentEvent::ThinkingDelta {
                            turn_id: turn_id.clone(),
                            text,
                        }),
                        Some(Delta::Text(text)) => events.emit(AgentEvent::TextDelta {
                            turn_id: turn_id.clone(),
                            text,
                        }),
                        None => deltas_closed = true,
                    }
                }
                result = &mut reply_rx => {
                    return match result {
                        Ok(inner) => inner,
                        Err(_) => Err(CoreError::Inference(
                            "the inference worker stopped unexpectedly".to_string(),
                        )),
                    };
                }
            }
        }
    }

    /// Stops the worker thread.
    pub fn shutdown(&self) {
        let _ = self.sender.send(WorkerCommand::Shutdown);
    }
}

impl Drop for LlamaBackend {
    fn drop(&mut self) {
        // Close the inference worker before returning so mapped model files can
        // be removed reliably, especially on Windows.
        let _ = self.sender.send(WorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct WorkerState {
    model: Arc<Model>,
    context: Context,
    sampler: Option<Sampler>,
    sampling: SamplingParams,
    mtmd: Option<Mtmd>,
    marker: String,
    /// Tokens known to be decoded into the context, when `cache_exact`.
    cached_tokens: Vec<i32>,
    cache_exact: bool,
    events: EventSink,
}

fn worker_main(
    options: EngineOptions,
    events: EventSink,
    cancel: Arc<AtomicBool>,
    commands: std::sync::mpsc::Receiver<WorkerCommand>,
    ready: tokio::sync::oneshot::Sender<Result<EngineInfo, CoreError>>,
) {
    events.emit(AgentEvent::ModelLoading {
        path: options.model_path.display().to_string(),
    });

    let gpu_layers = if cordial_llama::BACKEND == "cpu" {
        0
    } else {
        -1
    };
    let loaded = (|| -> Result<WorkerState, CoreError> {
        let model = Model::load(&options.model_path, gpu_layers)?;
        let threads = if options.threads > 0 {
            options.threads
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get() as i32)
                .unwrap_or(4)
        };
        let context = Context::new(
            &model,
            ContextOptions::new(options.context_tokens, threads),
            Some(Arc::clone(&cancel)),
        )?;
        let sampler = Sampler::new(&model, options.sampling)?;
        let (mtmd, marker) = match &options.mmproj_path {
            Some(path) if path.exists() => {
                let projector = Mtmd::load(&model, path, threads)?;
                let marker = projector.marker();
                (Some(projector), marker)
            }
            _ => (None, crate::prompt::DEFAULT_MEDIA_MARKER.to_string()),
        };
        Ok(WorkerState {
            model,
            context,
            sampler: Some(sampler),
            sampling: options.sampling,
            mtmd,
            marker,
            cached_tokens: Vec::new(),
            cache_exact: true,
            events: events.clone(),
        })
    })();

    match loaded {
        Ok(state) => {
            let info = EngineInfo {
                description: state.model.description(),
                context_tokens: state.context.n_ctx(),
                vision: state.mtmd.as_ref().map(|m| m.has_vision()).unwrap_or(false),
                backend: cordial_llama::BACKEND.to_string(),
            };
            let _ = ready.send(Ok(info.clone()));
            let mut state = state;
            state.events.emit(AgentEvent::ModelReady {
                description: info.description,
                context_tokens: info.context_tokens,
                backend: info.backend,
                vision: info.vision,
            });

            while let Ok(command) = commands.recv() {
                match command {
                    WorkerCommand::Generate {
                        request,
                        deltas,
                        reply,
                    } => {
                        let result = run_generation(&mut state, request, &deltas, &cancel);
                        let _ = reply.send(result);
                    }
                    WorkerCommand::Shutdown => break,
                }
            }
        }
        Err(err) => {
            let _ = ready.send(Err(CoreError::Inference(err.to_string())));
            events.emit(AgentEvent::Error {
                turn_id: None,
                message: err.to_string(),
            });
        }
    }
}

fn run_generation(
    state: &mut WorkerState,
    request: GenerateRequest,
    deltas: &tokio::sync::mpsc::UnboundedSender<Delta>,
    cancel: &AtomicBool,
) -> Result<GenerateOutput, CoreError> {
    let result = run_generation_inner(state, request, deltas, cancel);
    // An aborted decode can leave recurrent model memory partially updated.
    // Never reuse it, even if the token cache appears to match the next prompt.
    if result.is_err()
        || cancel.load(Ordering::Relaxed)
        || matches!(&result, Ok(output) if output.stop == GenStop::Cancelled)
    {
        state.context.clear();
        state.cached_tokens.clear();
        state.cache_exact = false;
    }
    if cancel.load(Ordering::Relaxed) && result.is_err() {
        return Ok(GenerateOutput {
            text: String::new(),
            stop: GenStop::Cancelled,
            prompt_tokens: 0,
            output_tokens: 0,
            tokens_per_second: 0.0,
        });
    }
    result
}

fn decode_error(err: LlamaError, used: usize, available: usize) -> CoreError {
    // llama_decode returns 1 when it cannot allocate a KV-cache slot. Other
    // codes are not evidence of context overflow and must remain failures.
    if matches!(&err, LlamaError::Decode(1)) {
        CoreError::ContextOverflow { used, available }
    } else {
        CoreError::Inference(err.to_string())
    }
}

fn run_generation_inner(
    state: &mut WorkerState,
    request: GenerateRequest,
    deltas: &tokio::sync::mpsc::UnboundedSender<Delta>,
    cancel: &AtomicBool,
) -> Result<GenerateOutput, CoreError> {
    // 1. Tokenize the prompt. LFM3 needs `parse_special` so ChatML markers
    //    become real control tokens; BOS is written into the text already.
    let tokens = state.model.tokenize(&request.prompt, false, true)?;

    let n_ctx = state.context.n_ctx() as usize;
    let needed = tokens.len() + request.max_tokens as usize;
    if tokens.len() >= n_ctx || needed > n_ctx {
        let available = n_ctx.saturating_sub(request.max_tokens as usize);
        return Err(CoreError::ContextOverflow {
            used: tokens.len(),
            available,
        });
    }

    // 2. Reuse the shared prefix when the cache is trustworthy.
    let started = std::time::Instant::now();
    let prompt_tokens = tokens.len() as u32;

    if !request.media.is_empty() {
        let Some(mtmd) = state.mtmd.as_ref() else {
            return Err(CoreError::ModelNotReady(
                "vision projector is not loaded".to_string(),
            ));
        };
        state.context.clear();
        state.cache_exact = false;
        state.cached_tokens.clear();
        let prompt = request
            .prompt
            .replace(crate::prompt::DEFAULT_MEDIA_MARKER, &state.marker);
        state
            .context
            .eval_multimodal(mtmd, &prompt, &request.media, 0)
            .map_err(|err| CoreError::Inference(err.to_string()))?;
        // Images expand to many tokens that are absent from text tokenization.
        // Check the real context position before generating, rather than
        // discovering the missing space halfway through a streamed answer.
        let used = (state.context.pos_max() + 1).max(0) as usize;
        if used.saturating_add(request.max_tokens as usize) > n_ctx {
            return Err(CoreError::ContextOverflow {
                used,
                available: n_ctx.saturating_sub(request.max_tokens as usize),
            });
        }
    } else {
        if !state.cache_exact {
            state.context.clear();
            state.cached_tokens.clear();
        }
        let common = if state.cache_exact {
            common_prefix(&state.cached_tokens, &tokens)
        } else {
            0
        };

        // How much of the cache survives. A prompt that simply extends what is
        // already decoded needs no surgery; a prompt that diverged (an old turn
        // was trimmed, or the conversation changed) does.
        let mut start = common;
        if common < state.cached_tokens.len() {
            if !state.context.trim(common as u32) {
                // Hybrid models (LFM3) cannot rewind their recurrent layers.
                // Starting the sequence again is the only correct option.
                state.context.clear();
                start = 0;
            }
        }
        state.cached_tokens.truncate(start);

        // The last token must always be evaluated so there are logits to sample.
        if start >= tokens.len() {
            state.context.clear();
            start = 0;
            state.cached_tokens.clear();
        }

        if start < tokens.len() {
            state
                .context
                .decode(&tokens[start..], start as i32)
                .map_err(|err| {
                    decode_error(
                        err,
                        tokens.len(),
                        n_ctx.saturating_sub(request.max_tokens as usize),
                    )
                })?;
        }
        state.cache_exact = true;
        state.cached_tokens = tokens.clone();
    }

    // 3. Sample until the model stops.
    if state.sampler.is_none()
        || state.sampling.seed != request.sampling.seed
        || state.sampling.temperature != request.sampling.temperature
        || state.sampling.top_k != request.sampling.top_k
    {
        state.sampler = Some(Sampler::new(&state.model, request.sampling)?);
        state.sampling = request.sampling;
    }
    let sampler = state.sampler.as_mut().expect("sampler created above");
    sampler.reset();

    let mut text = String::new();
    let mut in_thinking = request.thinking;
    let mut sent_bytes = 0usize;
    // Ask the context where it is instead of assuming: the memory is the only
    // authority on the last stored position (and M-RoPE rejects replays).
    let mut pos = state.context.pos_max() + 1;
    let mut stop = GenStop::MaxTokens;
    let mut output_tokens = 0u32;

    for _ in 0..request.max_tokens {
        if cancel.load(Ordering::Relaxed) {
            stop = GenStop::Cancelled;
            break;
        }
        let token = match state.context.sample(sampler) {
            Ok(token) => token,
            Err(err) => return Err(err.into()),
        };
        if state.model.is_eog(token) {
            stop = GenStop::EndOfTurn;
            break;
        }
        let piece = state.model.token_to_piece(token);
        if piece.is_empty() {
            // Still decode it: the model expects to see its own token.
            if let Err(err) = state.context.decode(&[token], pos) {
                if cancel.load(Ordering::Relaxed) {
                    stop = GenStop::Cancelled;
                    break;
                }
                return Err(decode_error(err, pos.max(0) as usize + 1, n_ctx));
            }
            pos = state.context.pos_max() + 1;
            state.cached_tokens.push(token);
            output_tokens += 1;
            continue;
        }

        text.push_str(&piece);
        output_tokens += 1;

        // Stream the new part, classified so the UI can collapse reasoning live.
        if in_thinking {
            if let Some(index) = text.find(THINK_CLOSE) {
                let split = index + THINK_CLOSE.len();
                if split > sent_bytes {
                    let _ = deltas.send(Delta::Thinking(text[sent_bytes..split].to_string()));
                }
                let rest = &text[split..];
                if !rest.is_empty() {
                    let _ = deltas.send(Delta::Text(rest.to_string()));
                }
                sent_bytes = text.len();
                in_thinking = false;
            } else {
                let _ = deltas.send(Delta::Thinking(text[sent_bytes..].to_string()));
                sent_bytes = text.len();
            }
        } else {
            let _ = deltas.send(Delta::Text(text[sent_bytes..].to_string()));
            sent_bytes = text.len();
        }

        if let Err(err) = state.context.decode(&[token], pos) {
            // llama.cpp aborts an in-flight decode when the user stops; that is
            // a cancellation, not a failure.
            if cancel.load(Ordering::Relaxed) {
                stop = GenStop::Cancelled;
                break;
            }
            return Err(decode_error(err, pos.max(0) as usize + 1, n_ctx));
        }
        pos = state.context.pos_max() + 1;
        state.cached_tokens.push(token);

        // Some LFM3 builds emit the ChatML terminator as text.
        if text.ends_with("<|im_end|>") {
            stop = GenStop::EndOfTurn;
            break;
        }
    }

    let elapsed = started.elapsed().as_secs_f32().max(0.001);
    Ok(GenerateOutput {
        text,
        stop,
        prompt_tokens,
        output_tokens,
        tokens_per_second: output_tokens as f32 / elapsed,
    })
}

fn common_prefix(a: &[i32], b: &[i32]) -> usize {
    let mut index = 0;
    while index < a.len() && index < b.len() && a[index] == b[index] {
        index += 1;
    }
    index
}

/// A failure a scripted backend can inject, used by the harness tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptFailure {
    /// Pretend the prompt did not fit the context window.
    Overflow,
    /// Pretend inference failed.
    Failed,
}

/// Replays canned replies. Used by tests, the CI behavioural suite and the
/// `--fake-model` developer mode.
pub struct ScriptedBackend {
    replies: VecDeque<String>,
    pub requests: Vec<GenerateRequest>,
    fallback: String,
    failures: VecDeque<ScriptFailure>,
    stops: VecDeque<GenStop>,
    cancel: Option<Arc<AtomicBool>>,
}

impl ScriptedBackend {
    pub fn new<I, S>(replies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            replies: replies.into_iter().map(Into::into).collect(),
            requests: Vec::new(),
            fallback: "I do not have a scripted reply for that.".to_string(),
            failures: VecDeque::new(),
            stops: VecDeque::new(),
            cancel: None,
        }
    }

    pub fn with_fallback(mut self, reply: impl Into<String>) -> Self {
        self.fallback = reply.into();
        self
    }

    /// Supplies stop reasons for scripted replies; unspecified replies end normally.
    pub fn with_stops(mut self, stops: impl IntoIterator<Item = GenStop>) -> Self {
        self.stops = stops.into_iter().collect();
        self
    }

    /// Shares a cancellation flag with the harness.
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Queues a failure to return before the next scripted reply.
    pub fn fail_once(&mut self, failure: ScriptFailure) {
        self.failures.push_back(failure);
    }

    pub async fn generate(
        &mut self,
        request: GenerateRequest,
        events: &EventSink,
    ) -> Result<GenerateOutput, CoreError> {
        if let Some(failure) = self.failures.pop_front() {
            return Err(match failure {
                ScriptFailure::Overflow => CoreError::ContextOverflow {
                    used: 999_999,
                    available: 1_000,
                },
                ScriptFailure::Failed => CoreError::Inference("scripted failure".to_string()),
            });
        }
        let reply = self
            .replies
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone());
        let prompt_tokens = request.prompt.chars().count() as u32 / 4;
        self.requests.push(request.clone());

        let (thinking, answer) = match reply.find(THINK_CLOSE) {
            Some(index) => (
                Some(reply[..index].to_string()),
                reply[index + THINK_CLOSE.len()..].to_string(),
            ),
            None => (None, reply.clone()),
        };
        if let Some(thinking) = thinking {
            events.emit(AgentEvent::ThinkingDelta {
                turn_id: request.turn_id.clone(),
                text: thinking,
            });
        }
        if !answer.is_empty() {
            // Stream in a few chunks to exercise the UI path.
            let chars: Vec<char> = answer.chars().collect();
            for chunk in chars.chunks(24) {
                events.emit(AgentEvent::TextDelta {
                    turn_id: request.turn_id.clone(),
                    text: chunk.iter().collect(),
                });
            }
        }

        Ok(GenerateOutput {
            text: reply,
            stop: self.stops.pop_front().unwrap_or(GenStop::EndOfTurn),
            prompt_tokens,
            output_tokens: 32,
            tokens_per_second: 42.0,
        })
    }
}

/// Either the real model or a scripted one.
pub enum Backend {
    Llama(LlamaBackend),
    Scripted(ScriptedBackend),
}

impl Backend {
    pub async fn generate(
        &mut self,
        request: GenerateRequest,
        events: &EventSink,
    ) -> Result<GenerateOutput, CoreError> {
        match self {
            Backend::Llama(backend) => backend.generate(request, events).await,
            Backend::Scripted(backend) => backend.generate(request, events).await,
        }
    }

    pub fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        match self {
            Backend::Llama(backend) => Some(backend.cancel_flag()),
            Backend::Scripted(backend) => backend.cancel.clone(),
        }
    }
}

/// Convenience for loading a model file pair from settings.
pub fn engine_options(
    model_path: &Path,
    mmproj_path: Option<&Path>,
    context_tokens: u32,
    threads: i32,
    sampling: SamplingParams,
) -> EngineOptions {
    EngineOptions {
        model_path: model_path.to_path_buf(),
        mmproj_path: mmproj_path.map(Path::to_path_buf),
        context_tokens,
        threads,
        sampling,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSink;

    fn request(text: &str) -> GenerateRequest {
        GenerateRequest {
            turn_id: "t1".into(),
            prompt: text.into(),
            media: Vec::new(),
            max_tokens: 64,
            thinking: true,
            sampling: SamplingParams::default(),
        }
    }

    #[test]
    fn only_kv_slot_exhaustion_is_classified_as_context_overflow() {
        assert!(matches!(
            decode_error(LlamaError::Decode(1), 100, 99),
            CoreError::ContextOverflow {
                used: 100,
                available: 99
            }
        ));
        assert!(matches!(
            decode_error(LlamaError::Decode(-1), 100, 99),
            CoreError::Inference(_)
        ));
        assert!(matches!(
            decode_error(LlamaError::Sample, 100, 99),
            CoreError::Inference(_)
        ));
    }

    #[test]
    fn common_prefix_counts_matching_tokens() {
        assert_eq!(common_prefix(&[1, 2, 3], &[1, 2, 9]), 2);
        assert_eq!(common_prefix(&[], &[1]), 0);
        assert_eq!(common_prefix(&[1, 2], &[1, 2]), 2);
    }

    #[tokio::test]
    async fn scripted_backend_streams_reasoning_then_answer() {
        let mut backend = ScriptedBackend::new(vec!["plan</think>\nHello there."]);
        let (sink, mut receiver) = EventSink::channel();
        let output = backend.generate(request("hi"), &sink).await.unwrap();
        assert_eq!(output.stop, GenStop::EndOfTurn);
        assert!(output.text.contains("Hello there."));

        let mut thinking = String::new();
        let mut answer = String::new();
        while let Ok(event) = receiver.try_recv() {
            match event {
                AgentEvent::ThinkingDelta { text, .. } => thinking.push_str(&text),
                AgentEvent::TextDelta { text, .. } => answer.push_str(&text),
                _ => {}
            }
        }
        assert_eq!(thinking, "plan");
        assert!(answer.contains("Hello there."));
    }

    #[tokio::test]
    async fn scripted_backend_falls_back_when_script_runs_out() {
        let mut backend = ScriptedBackend::new(Vec::<String>::new()).with_fallback("fallback text");
        let output = backend
            .generate(request("hi"), &EventSink::silent())
            .await
            .unwrap();
        assert_eq!(output.text, "fallback text");
    }
}
