// Cordial's inference boundary.
//
// Everything in this file is a deliberate, minimal C ABI over llama.cpp +
// libmtmd. Rust never sees a llama.cpp struct: model/context/sampler params,
// callbacks and the batch layout all stay inside C++ where the headers are the
// source of truth. That keeps the unsafe surface small and auditable, and it
// means an upstream llama.cpp bump cannot silently shift a Rust struct.
//
// Conventions:
//   - Pointers returned are owned by the caller until the matching *_free.
//   - Negative return values are errors; text errors go through `err`.
//   - No function here allocates a Rust-side type.

#ifndef CORDIAL_LLAMA_H
#define CORDIAL_LLAMA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*cordial_log_cb)(int level, const char * text, void * ud);
// Returns true to abort an in-flight decode.
typedef int32_t (*cordial_abort_cb)(void * ud);

// ---------------------------------------------------------------- lifecycle

void         cordial_backend_init(void);
const char * cordial_version(void);
void         cordial_log_set(cordial_log_cb cb, void * ud);

// ------------------------------------------------------------------- model

typedef struct cordial_model cordial_model;

cordial_model * cordial_model_load(const char * path, int32_t n_gpu_layers,
                                   char * err, size_t errlen);
void            cordial_model_free(cordial_model * m);
int32_t         cordial_model_n_ctx_train(const cordial_model * m);
int32_t         cordial_model_n_embd(const cordial_model * m);
uint64_t        cordial_model_n_params(const cordial_model * m);
uint64_t        cordial_model_size(const cordial_model * m);
int32_t         cordial_model_desc(const cordial_model * m, char * buf, size_t len);
int32_t         cordial_model_meta(const cordial_model * m, const char * key, char * buf, size_t len);
int32_t         cordial_vocab_n_tokens(const cordial_model * m);

// ----------------------------------------------------------------- context

typedef struct cordial_ctx cordial_ctx;

cordial_ctx * cordial_ctx_new(cordial_model * m, uint32_t n_ctx, uint32_t n_batch,
                              uint32_t n_ubatch, int32_t n_threads, int32_t kv_unified,
                              cordial_abort_cb abort_cb, void * abort_ud,
                              char * err, size_t errlen);
void     cordial_ctx_free(cordial_ctx * ctx);
uint32_t cordial_ctx_n_ctx(const cordial_ctx * ctx);
int32_t  cordial_ctx_pos_max(const cordial_ctx * ctx); // -1 when nothing is cached
void     cordial_ctx_trim(cordial_ctx * ctx, int32_t n_keep); // drop KV from n_keep onwards
void     cordial_ctx_clear(cordial_ctx * ctx);

// --------------------------------------------------------------- tokenizer

// Returns the token count, or a negative value (-n) when `cap` is too small.
int32_t cordial_tokenize(const cordial_model * m, const char * text, int32_t add_special,
                         int32_t parse_special, int32_t * out, int32_t cap);
// Returns bytes written, or a negative value when the buffer is too small.
int32_t     cordial_token_to_piece(const cordial_model * m, int32_t token, char * buf, int32_t cap);
int32_t     cordial_token_is_eog(const cordial_model * m, int32_t token);
const char * cordial_token_text(const cordial_model * m, int32_t token);

// ------------------------------------------------------------------ decode

// Decodes `tokens` starting at position `pos0`. Returns 0 on success.
int32_t cordial_decode_text(cordial_ctx * ctx, const int32_t * tokens, int32_t n,
                            int32_t pos0, int32_t logits_last_only);

// --------------------------------------------------------------- sampling

typedef struct cordial_sampler cordial_sampler;

cordial_sampler * cordial_sampler_new(int32_t n_vocab, float temp, int32_t top_k, float top_p,
                                      float min_p, float repeat_penalty, int32_t penalty_last_n,
                                      uint32_t seed);
void    cordial_sampler_free(cordial_sampler * s);
void    cordial_sampler_reset(cordial_sampler * s);
int32_t cordial_sampler_sample(cordial_sampler * s, cordial_ctx * ctx);

// ------------------------------------------------------------------- mtmd

typedef struct cordial_mtmd cordial_mtmd;

cordial_mtmd * cordial_mtmd_load(const cordial_model * m, const char * mmproj, int32_t n_threads,
                                 char * err, size_t errlen);
void           cordial_mtmd_free(cordial_mtmd * mm);
int32_t        cordial_mtmd_has_vision(const cordial_mtmd * mm);
const char *   cordial_mtmd_marker(const cordial_mtmd * mm);

// Tokenizes `text` (which must contain one marker per media file) plus the
// media files, then evaluates every chunk against `ctx`. Returns the new
// position after evaluation, or -1 on failure with `err` filled in.
int32_t cordial_eval_prompt(cordial_mtmd * mm, cordial_ctx * ctx, const char * text,
                            const char * const * media_paths, int32_t n_media, int32_t n_past,
                            int32_t logits_last, char * err, size_t errlen);

#ifdef __cplusplus
}
#endif

#endif // CORDIAL_LLAMA_H
