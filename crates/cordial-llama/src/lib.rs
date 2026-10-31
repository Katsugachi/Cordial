//! Safe bindings to the pinned llama.cpp build that ships inside Cordial.
//!
//! The surface is intentionally small: load a model, create one context,
//! tokenize, decode, sample, and evaluate multimodal prompts. Anything that
//! needs a llama.cpp struct lives in `csrc/cordial_llama.cpp`, so this crate
//! only ever passes integers, byte strings and opaque pointers across FFI.

mod sys;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once, OnceLock};

use thiserror::Error;

/// ggml backend selected at build time: `metal`, `opencl`, `vulkan`, `cuda` or `cpu`.
pub const BACKEND: &str = env!("CORDIAL_BACKEND");
/// Pinned llama.cpp release, shown in the app's About panel.
pub const LLAMA_CPP_TAG: &str = env!("CORDIAL_LLAMA_TAG");

const ERR_LEN: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Other,
}

#[derive(Debug, Error)]
pub enum LlamaError {
    #[error("could not load model: {0}")]
    Load(String),
    #[error("could not create context: {0}")]
    Context(String),
    #[error("could not load the vision projector: {0}")]
    Vision(String),
    #[error("tokenization failed: {0}")]
    Tokenize(String),
    #[error("decode failed with code {0}")]
    Decode(i32),
    #[error("sampling failed")]
    Sample,
    #[error("multimodal evaluation failed: {0}")]
    Eval(String),
    #[error("path contains a NUL byte: {0}")]
    Path(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<std::ffi::NulError> for LlamaError {
    fn from(err: std::ffi::NulError) -> Self {
        LlamaError::Path(err.to_string())
    }
}

/// Initialise llama.cpp exactly once per process.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: called once, before any model or context is created.
        unsafe { sys::cordial_backend_init() };
    });
}

/// The llama.cpp version string compiled into the app.
pub fn llama_version() -> String {
    // SAFETY: the pointer is a static string owned by llama.cpp.
    unsafe { CStr::from_ptr(sys::cordial_version()) }
        .to_string_lossy()
        .into_owned()
}

static LOGGER: OnceLock<fn(LogLevel, &str)> = OnceLock::new();

/// Forward llama.cpp/ggml/mtmd logging into the host application.
pub fn set_logger(logger: fn(LogLevel, &str)) {
    if LOGGER.set(logger).is_ok() {
        // SAFETY: the trampoline only reads its arguments and calls `LOGGER`.
        unsafe { sys::cordial_log_set(Some(log_trampoline), std::ptr::null_mut()) };
    }
}

extern "C" fn log_trampoline(level: c_int, text: *const c_char, _ud: *mut c_void) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let level = match level {
        1 => LogLevel::Debug,
        2 => LogLevel::Info,
        3 => LogLevel::Warn,
        4 => LogLevel::Error,
        _ => LogLevel::Other,
    };
    let message = if text.is_null() {
        String::new()
    } else {
        // SAFETY: llama.cpp hands over a NUL-terminated, valid-for-the-call string.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    };
    logger(level, message.trim_end());
}

fn err_buffer() -> [c_char; ERR_LEN] {
    [0; ERR_LEN]
}

fn read_err(buf: &[c_char; ERR_LEN], fallback: &str) -> String {
    // SAFETY: the shim always NUL-terminates when it reports an error.
    let text = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn path_cstring(path: &Path) -> Result<CString, LlamaError> {
    let raw = path.to_string_lossy();
    Ok(CString::new(raw.as_ref())?)
}

// --------------------------------------------------------------------- model

/// A loaded GGUF model. Cheap to share behind an [`Arc`]; the weights are
/// read-only after loading. A context is required to actually run inference.
pub struct Model {
    ptr: NonNull<sys::CordialModel>,
}

// SAFETY: llama.cpp allows one model to be shared by multiple contexts as long
// as they are not used concurrently from multiple threads; Cordial owns a
// single context on a single worker thread.
unsafe impl Send for Model {}
unsafe impl Sync for Model {}

impl Model {
    /// Loads a GGUF file. `n_gpu_layers` = -1 offloads everything the backend can hold.
    pub fn load(path: &Path, n_gpu_layers: i32) -> Result<Arc<Self>, LlamaError> {
        init();
        let c_path = path_cstring(path)?;
        let mut err = err_buffer();
        // SAFETY: valid path, valid error buffer.
        let ptr = unsafe {
            sys::cordial_model_load(c_path.as_ptr(), n_gpu_layers, err.as_mut_ptr(), ERR_LEN)
        };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| LlamaError::Load(read_err(&err, "unknown llama.cpp error")))?;
        Ok(Arc::new(Self { ptr }))
    }

    /// Human readable model description, e.g. "LFM3-VL 2B Q4_K - Medium".
    pub fn description(&self) -> String {
        let mut buf = vec![0u8; 256];
        // SAFETY: buffer and length are valid for the call.
        let written = unsafe {
            sys::cordial_model_desc(
                self.ptr.as_ptr(),
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
            )
        };
        if written < 0 {
            return String::new();
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }

    /// Native context length advertised by the GGUF metadata.
    pub fn n_ctx_train(&self) -> u32 {
        // SAFETY: pointer is valid while `self` lives.
        let value = unsafe { sys::cordial_model_n_ctx_train(self.ptr.as_ptr()) };
        value.max(1) as u32
    }

    pub fn n_embd(&self) -> u32 {
        // SAFETY: as above.
        unsafe { sys::cordial_model_n_embd(self.ptr.as_ptr()) }.max(1) as u32
    }

    pub fn n_params(&self) -> u64 {
        // SAFETY: as above.
        unsafe { sys::cordial_model_n_params(self.ptr.as_ptr()) }
    }

    pub fn file_size(&self) -> u64 {
        // SAFETY: as above.
        unsafe { sys::cordial_model_size(self.ptr.as_ptr()) }
    }

    pub fn vocab_size(&self) -> i32 {
        // SAFETY: as above.
        unsafe { sys::cordial_vocab_n_tokens(self.ptr.as_ptr()) }
    }

    /// Reads a GGUF metadata string such as `general.architecture`.
    pub fn meta(&self, key: &str) -> Option<String> {
        let key = CString::new(key).ok()?;
        let mut buf = vec![0u8; 512];
        // SAFETY: valid key, buffer and length.
        let written = unsafe {
            sys::cordial_model_meta(
                self.ptr.as_ptr(),
                key.as_ptr(),
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
            )
        };
        if written < 0 {
            return None;
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    /// Tokenizes `text`. `parse_special` treats `<|im_start|>`-style markers as
    /// their special tokens, which is what prompt assembly relies on.
    pub fn tokenize(
        &self,
        text: &str,
        add_special: bool,
        parse_special: bool,
    ) -> Result<Vec<i32>, LlamaError> {
        let c_text = CString::new(text)?;
        let mut guess = text.len().saturating_add(16) as usize;
        loop {
            let mut out = vec![0i32; guess];
            // SAFETY: valid text, writable buffer of `guess` entries.
            let written = unsafe {
                sys::cordial_tokenize(
                    self.ptr.as_ptr(),
                    c_text.as_ptr(),
                    add_special as i32,
                    parse_special as i32,
                    out.as_mut_ptr(),
                    guess as i32,
                )
            };
            if written >= 0 {
                out.truncate(written as usize);
                return Ok(out);
            }
            // llama.cpp returns -needed when the buffer is too small.
            let needed = (-written) as usize;
            if needed <= guess {
                return Err(LlamaError::Tokenize(format!(
                    "llama.cpp reported {needed} tokens are needed but {guess} slots were free"
                )));
            }
            guess = needed + 8;
        }
    }

    /// Decodes a single token back into text. Tokens may split UTF-8 sequences,
    /// so the result is lossy on purpose.
    pub fn token_to_piece(&self, token: i32) -> String {
        let mut cap = 32usize;
        loop {
            let mut buf = vec![0u8; cap];
            // SAFETY: writable buffer of `cap` bytes.
            let written = unsafe {
                sys::cordial_token_to_piece(
                    self.ptr.as_ptr(),
                    token,
                    buf.as_mut_ptr() as *mut c_char,
                    cap as i32,
                )
            };
            if written >= 0 {
                buf.truncate(written as usize);
                return String::from_utf8_lossy(&buf).into_owned();
            }
            let needed = (-written) as usize;
            if needed <= cap {
                return String::new();
            }
            cap = needed + 8;
        }
    }

    /// True for end-of-generation tokens (eot/eos/turn end).
    pub fn is_eog(&self, token: i32) -> bool {
        // SAFETY: pointer valid while `self` lives.
        unsafe { sys::cordial_token_is_eog(self.ptr.as_ptr(), token) != 0 }
    }

    pub fn token_text(&self, token: i32) -> Option<String> {
        // SAFETY: returns a pointer owned by the vocabulary.
        let ptr = unsafe { sys::cordial_token_text(self.ptr.as_ptr(), token) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: NUL-terminated static vocabulary string.
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        // SAFETY: we own the handle and free it once.
        unsafe { sys::cordial_model_free(self.ptr.as_ptr()) };
    }
}

// ------------------------------------------------------------------- context

/// Context sizing and threading for one conversation.
#[derive(Debug, Clone, Copy)]
pub struct ContextOptions {
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub threads: i32,
    pub kv_unified: bool,
}

impl ContextOptions {
    pub fn new(n_ctx: u32, threads: i32) -> Self {
        Self {
            n_ctx,
            n_batch: 2048,
            n_ubatch: 512,
            threads: threads.max(1),
            kv_unified: true,
        }
    }
}

/// One decoding context (KV cache) for a single conversation.
pub struct Context {
    ptr: NonNull<sys::CordialCtx>,
    model: Arc<Model>,
    n_batch: u32,
    cancel: Option<Arc<AtomicBool>>,
}

impl Context {
    pub fn new(
        model: &Arc<Model>,
        options: ContextOptions,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Self, LlamaError> {
        init();
        let mut err = err_buffer();
        let abort_ud = cancel
            .as_ref()
            .map(|flag| Arc::as_ptr(flag) as *mut c_void)
            .unwrap_or(std::ptr::null_mut());
        // SAFETY: the model outlives the context (held in `self`), and the
        // cancel flag is kept alive in `self` for as long as llama.cpp holds
        // the pointer.
        let ptr = unsafe {
            sys::cordial_ctx_new(
                model.ptr.as_ptr(),
                options.n_ctx,
                options.n_batch,
                options.n_ubatch,
                options.threads,
                options.kv_unified as i32,
                if cancel.is_some() {
                    Some(cancel_trampoline)
                } else {
                    None
                },
                abort_ud,
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| LlamaError::Context(read_err(&err, "unknown llama.cpp error")))?;
        Ok(Self {
            ptr,
            model: Arc::clone(model),
            n_batch: options.n_batch.max(1),
            cancel,
        })
    }

    pub fn model(&self) -> &Arc<Model> {
        &self.model
    }

    pub fn n_ctx(&self) -> u32 {
        // SAFETY: pointer valid while `self` lives.
        unsafe { sys::cordial_ctx_n_ctx(self.ptr.as_ptr()) }
    }

    /// Highest cached position, or -1 when the cache is empty.
    pub fn pos_max(&self) -> i32 {
        // SAFETY: as above.
        unsafe { sys::cordial_ctx_pos_max(self.ptr.as_ptr()) }
    }

    /// Drops everything cached from `n_keep` onwards, keeping the shared prefix.
    pub fn trim(&mut self, n_keep: u32) {
        // SAFETY: as above.
        unsafe { sys::cordial_ctx_trim(self.ptr.as_ptr(), n_keep as i32) };
    }

    pub fn clear(&mut self) {
        // SAFETY: as above.
        unsafe { sys::cordial_ctx_clear(self.ptr.as_ptr()) };
    }

    /// True when the caller asked to stop and llama.cpp observed it.
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Decodes `tokens` at `pos0`, chunking to the context's batch size.
    /// Logits are produced for the final token only.
    pub fn decode(&mut self, tokens: &[i32], pos0: i32) -> Result<(), LlamaError> {
        if tokens.is_empty() {
            return Ok(());
        }
        let batch = self.n_batch as usize;
        let mut offset = 0usize;
        let mut pos = pos0;
        while offset < tokens.len() {
            let end = (offset + batch).min(tokens.len());
            let chunk = &tokens[offset..end];
            let last = end == tokens.len();
            // SAFETY: `chunk` is a valid slice for `chunk.len()` tokens.
            let rc = unsafe {
                sys::cordial_decode_text(
                    self.ptr.as_ptr(),
                    chunk.as_ptr(),
                    chunk.len() as i32,
                    pos,
                    last as i32,
                )
            };
            if rc != 0 {
                return Err(LlamaError::Decode(rc));
            }
            pos += chunk.len() as i32;
            offset = end;
            if self.is_cancelled() {
                return Err(LlamaError::Decode(-2));
            }
        }
        Ok(())
    }

    /// Samples the next token from the logits of the most recent decode.
    pub fn sample(&mut self, sampler: &mut Sampler) -> Result<i32, LlamaError> {
        // SAFETY: both handles are live for the duration of the call.
        let token = unsafe { sys::cordial_sampler_sample(sampler.ptr.as_ptr(), self.ptr.as_ptr()) };
        if token < 0 {
            return Err(LlamaError::Sample);
        }
        Ok(token)
    }

    /// Evaluates a prompt that contains one media marker per attached image.
    /// Returns the position after the prompt.
    pub fn eval_multimodal(
        &mut self,
        mtmd: &Mtmd,
        prompt: &str,
        media: &[PathBuf],
        n_past: i32,
    ) -> Result<i32, LlamaError> {
        let c_prompt = CString::new(prompt)?;
        let c_media: Vec<CString> = media
            .iter()
            .map(|p| CString::new(p.to_string_lossy().as_ref()))
            .collect::<Result<_, _>>()?;
        let media_ptrs: Vec<*const c_char> = c_media.iter().map(|c| c.as_ptr()).collect();
        let mut err = err_buffer();
        // SAFETY: all pointers outlive the call; the shim frees its own copies.
        let rc = unsafe {
            sys::cordial_eval_prompt(
                mtmd.ptr.as_ptr(),
                self.ptr.as_ptr(),
                c_prompt.as_ptr(),
                media_ptrs.as_ptr(),
                media_ptrs.len() as i32,
                n_past,
                if n_past >= 0 { 1 } else { 0 },
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        if rc < 0 {
            return Err(LlamaError::Eval(read_err(&err, "unknown mtmd error")));
        }
        Ok(rc)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: we own the handle and free it once, before the cancel flag
        // (still held in `self`) is dropped.
        unsafe { sys::cordial_ctx_free(self.ptr.as_ptr()) };
    }
}

extern "C" fn cancel_trampoline(ud: *mut c_void) -> i32 {
    if ud.is_null() {
        return 0;
    }
    // SAFETY: `ud` is the `Arc<AtomicBool>` pointer handed to llama.cpp in
    // `Context::new`; it outlives the context.
    let flag = unsafe { &*(ud as *const AtomicBool) };
    flag.load(Ordering::Relaxed) as i32
}

// ------------------------------------------------------------------- sampler

/// Sampling parameters. Defaults follow Liquid AI's recommendation for LFM3:
/// temperature 0.1, top_k 50, top_p 1.0, min_p 0.0, repetition penalty 1.1.
#[derive(Debug, Clone, Copy)]
pub struct SamplingParams {
    pub temperature: f32,
    pub top_k: i32,
    pub top_p: f32,
    pub min_p: f32,
    pub repeat_penalty: f32,
    pub repeat_last_n: i32,
    pub seed: u32,
}

impl Default for SamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.1,
            top_k: 50,
            top_p: 1.0,
            min_p: 0.0,
            repeat_penalty: 1.1,
            repeat_last_n: 64,
            seed: 0x00c0_2d1a,
        }
    }
}

pub struct Sampler {
    ptr: NonNull<sys::CordialSampler>,
}

// SAFETY: a sampler chain is independent state, moved onto the worker thread.
unsafe impl Send for Sampler {}

impl Sampler {
    pub fn new(model: &Model, params: SamplingParams) -> Result<Self, LlamaError> {
        // SAFETY: plain integer/float arguments.
        let ptr = unsafe {
            sys::cordial_sampler_new(
                model.vocab_size(),
                params.temperature,
                params.top_k,
                params.top_p,
                params.min_p,
                params.repeat_penalty,
                params.repeat_last_n,
                params.seed,
            )
        };
        NonNull::new(ptr)
            .map(|ptr| Self { ptr })
            .ok_or(LlamaError::Sample)
    }

    pub fn reset(&mut self) {
        // SAFETY: handle is live.
        unsafe { sys::cordial_sampler_reset(self.ptr.as_ptr()) };
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // SAFETY: we own the handle and free it once.
        unsafe { sys::cordial_sampler_free(self.ptr.as_ptr()) };
    }
}

// ---------------------------------------------------------------------- mtmd

/// The vision projector (mmproj) paired with a model.
pub struct Mtmd {
    ptr: NonNull<sys::CordialMtmd>,
    _model: Arc<Model>,
}

unsafe impl Send for Mtmd {}

impl Mtmd {
    pub fn load(model: &Arc<Model>, mmproj: &Path, threads: i32) -> Result<Self, LlamaError> {
        init();
        let c_path = path_cstring(mmproj)?;
        let mut err = err_buffer();
        // SAFETY: the model outlives the projector (kept in `self`).
        let ptr = unsafe {
            sys::cordial_mtmd_load(
                model.ptr.as_ptr(),
                c_path.as_ptr(),
                threads.max(1),
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| LlamaError::Vision(read_err(&err, "unknown mtmd error")))?;
        Ok(Self {
            ptr,
            _model: Arc::clone(model),
        })
    }

    pub fn has_vision(&self) -> bool {
        // SAFETY: handle is live.
        unsafe { sys::cordial_mtmd_has_vision(self.ptr.as_ptr()) != 0 }
    }

    /// The placeholder string the prompt must use for each image.
    pub fn marker(&self) -> String {
        // SAFETY: returns a static string owned by mtmd.
        let ptr = unsafe { sys::cordial_mtmd_marker(self.ptr.as_ptr()) };
        if ptr.is_null() {
            return "<__media__>".to_string();
        }
        // SAFETY: NUL-terminated static string.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Mtmd {
    fn drop(&mut self) {
        // SAFETY: we own the handle and free it once.
        unsafe { sys::cordial_mtmd_free(self.ptr.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_defaults_match_model_card() {
        let p = SamplingParams::default();
        assert_eq!(p.temperature, 0.1);
        assert_eq!(p.top_k, 50);
        assert_eq!(p.top_p, 1.0);
        assert_eq!(p.min_p, 0.0);
        assert_eq!(p.repeat_penalty, 1.1);
    }

    #[test]
    fn backend_is_known() {
        assert!(["metal", "opencl", "vulkan", "cuda", "cpu"].contains(&BACKEND));
    }

    #[test]
    fn llama_version_is_reported() {
        init();
        assert!(!llama_version().is_empty());
    }
}
