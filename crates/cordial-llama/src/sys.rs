//! Raw C ABI exposed by `csrc/cordial_llama.cpp`.
//!
//! Nothing in this module is safe to call directly; `lib.rs` wraps it.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

#[repr(C)]
pub struct CordialModel {
    _private: [u8; 0],
}

#[repr(C)]
pub struct CordialCtx {
    _private: [u8; 0],
}

#[repr(C)]
pub struct CordialSampler {
    _private: [u8; 0],
}

#[repr(C)]
pub struct CordialMtmd {
    _private: [u8; 0],
}

pub type LogCallback = extern "C" fn(level: c_int, text: *const c_char, ud: *mut c_void);
pub type AbortCallback = extern "C" fn(ud: *mut c_void) -> i32;

extern "C" {
    pub fn cordial_backend_init();
    pub fn cordial_version() -> *const c_char;
    pub fn cordial_log_set(cb: Option<LogCallback>, ud: *mut c_void);

    pub fn cordial_model_load(
        path: *const c_char,
        n_gpu_layers: i32,
        err: *mut c_char,
        errlen: usize,
    ) -> *mut CordialModel;
    pub fn cordial_model_free(m: *mut CordialModel);
    pub fn cordial_model_n_ctx_train(m: *const CordialModel) -> i32;
    pub fn cordial_model_n_embd(m: *const CordialModel) -> i32;
    pub fn cordial_model_n_params(m: *const CordialModel) -> u64;
    pub fn cordial_model_size(m: *const CordialModel) -> u64;
    pub fn cordial_model_desc(m: *const CordialModel, buf: *mut c_char, len: usize) -> i32;
    pub fn cordial_model_meta(
        m: *const CordialModel,
        key: *const c_char,
        buf: *mut c_char,
        len: usize,
    ) -> i32;
    pub fn cordial_vocab_n_tokens(m: *const CordialModel) -> i32;

    pub fn cordial_ctx_new(
        m: *mut CordialModel,
        n_ctx: u32,
        n_batch: u32,
        n_ubatch: u32,
        n_threads: i32,
        kv_unified: i32,
        abort_cb: Option<AbortCallback>,
        abort_ud: *mut c_void,
        err: *mut c_char,
        errlen: usize,
    ) -> *mut CordialCtx;
    pub fn cordial_ctx_free(ctx: *mut CordialCtx);
    pub fn cordial_ctx_n_ctx(ctx: *const CordialCtx) -> u32;
    pub fn cordial_ctx_pos_max(ctx: *const CordialCtx) -> i32;
    pub fn cordial_ctx_trim(ctx: *mut CordialCtx, n_keep: i32);
    pub fn cordial_ctx_clear(ctx: *mut CordialCtx);

    pub fn cordial_tokenize(
        m: *const CordialModel,
        text: *const c_char,
        add_special: i32,
        parse_special: i32,
        out: *mut i32,
        cap: i32,
    ) -> i32;
    pub fn cordial_token_to_piece(
        m: *const CordialModel,
        token: i32,
        buf: *mut c_char,
        cap: i32,
    ) -> i32;
    pub fn cordial_token_is_eog(m: *const CordialModel, token: i32) -> i32;
    pub fn cordial_token_text(m: *const CordialModel, token: i32) -> *const c_char;

    pub fn cordial_decode_text(
        ctx: *mut CordialCtx,
        tokens: *const i32,
        n: i32,
        pos0: i32,
        logits_last_only: i32,
    ) -> i32;

    pub fn cordial_sampler_new(
        n_vocab: i32,
        temp: f32,
        top_k: i32,
        top_p: f32,
        min_p: f32,
        repeat_penalty: f32,
        penalty_last_n: i32,
        seed: u32,
    ) -> *mut CordialSampler;
    pub fn cordial_sampler_free(s: *mut CordialSampler);
    pub fn cordial_sampler_reset(s: *mut CordialSampler);
    pub fn cordial_sampler_sample(s: *mut CordialSampler, ctx: *mut CordialCtx) -> i32;

    pub fn cordial_mtmd_load(
        m: *const CordialModel,
        mmproj: *const c_char,
        n_threads: i32,
        err: *mut c_char,
        errlen: usize,
    ) -> *mut CordialMtmd;
    pub fn cordial_mtmd_free(mm: *mut CordialMtmd);
    pub fn cordial_mtmd_has_vision(mm: *const CordialMtmd) -> i32;
    pub fn cordial_mtmd_marker(mm: *const CordialMtmd) -> *const c_char;
    pub fn cordial_eval_prompt(
        mm: *mut CordialMtmd,
        ctx: *mut CordialCtx,
        text: *const c_char,
        media_paths: *const *const c_char,
        n_media: i32,
        n_past: i32,
        logits_last: i32,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
}
