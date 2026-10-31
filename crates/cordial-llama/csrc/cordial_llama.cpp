#include "cordial_llama.h"

#include "llama.h"
#include "mtmd.h"
#include "mtmd-helper.h"

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

struct cordial_model {
    llama_model * model = nullptr;
};

struct cordial_ctx {
    llama_context * ctx = nullptr;
};

struct cordial_sampler {
    llama_sampler * smpl = nullptr;
};

struct cordial_mtmd {
    mtmd_context * ctx = nullptr;
};

namespace {

void set_err(char * err, size_t errlen, const char * msg) {
    if (err != nullptr && errlen > 0) {
        std::snprintf(err, errlen, "%s", msg != nullptr ? msg : "unknown error");
    }
}

cordial_log_cb g_log_cb  = nullptr;
void *         g_log_ud  = nullptr;

void shim_log(enum ggml_log_level level, const char * text, void * /*user_data*/) {
    if (g_log_cb != nullptr) {
        g_log_cb(static_cast<int>(level), text != nullptr ? text : "", g_log_ud);
    }
}

// Copies a llama.cpp metadata string into a caller buffer.
int32_t copy_meta(const llama_model * model, const char * key, char * buf, size_t len) {
    if (model == nullptr || buf == nullptr || len == 0) {
        return -1;
    }
    return llama_model_meta_val_str(model, key, buf, len);
}

} // namespace

// ---------------------------------------------------------------- lifecycle

void cordial_backend_init(void) {
    llama_backend_init();
}

const char * cordial_version(void) {
    return llama_version();
}

void cordial_log_set(cordial_log_cb cb, void * ud) {
    g_log_cb = cb;
    g_log_ud = ud;
    llama_log_set(shim_log, nullptr);
    mtmd_log_set(shim_log, nullptr);
}

// ------------------------------------------------------------------- model

cordial_model * cordial_model_load(const char * path, int32_t n_gpu_layers, char * err, size_t errlen) {
    if (path == nullptr) {
        set_err(err, errlen, "model path is empty");
        return nullptr;
    }
    llama_model_params params = llama_model_default_params();
    params.n_gpu_layers = n_gpu_layers;

    llama_model * model = llama_model_load_from_file(path, params);
    if (model == nullptr) {
        set_err(err, errlen, "llama.cpp could not load the model file");
        return nullptr;
    }
    auto * out = new cordial_model();
    out->model = model;
    return out;
}

void cordial_model_free(cordial_model * m) {
    if (m == nullptr) {
        return;
    }
    if (m->model != nullptr) {
        llama_model_free(m->model);
    }
    delete m;
}

int32_t cordial_model_n_ctx_train(const cordial_model * m) {
    return m != nullptr ? llama_model_n_ctx_train(m->model) : 0;
}

int32_t cordial_model_n_embd(const cordial_model * m) {
    return m != nullptr ? llama_model_n_embd(m->model) : 0;
}

uint64_t cordial_model_n_params(const cordial_model * m) {
    return m != nullptr ? llama_model_n_params(m->model) : 0;
}

uint64_t cordial_model_size(const cordial_model * m) {
    return m != nullptr ? llama_model_size(m->model) : 0;
}

int32_t cordial_model_desc(const cordial_model * m, char * buf, size_t len) {
    if (m == nullptr || buf == nullptr || len == 0) {
        return -1;
    }
    return llama_model_desc(m->model, buf, len);
}

int32_t cordial_model_meta(const cordial_model * m, const char * key, char * buf, size_t len) {
    return copy_meta(m != nullptr ? m->model : nullptr, key, buf, len);
}

int32_t cordial_vocab_n_tokens(const cordial_model * m) {
    if (m == nullptr) {
        return 0;
    }
    const llama_vocab * vocab = llama_model_get_vocab(m->model);
    return llama_vocab_n_tokens(vocab);
}

// ----------------------------------------------------------------- context

cordial_ctx * cordial_ctx_new(cordial_model * m, uint32_t n_ctx, uint32_t n_batch, uint32_t n_ubatch,
                              int32_t n_threads, int32_t kv_unified, cordial_abort_cb abort_cb,
                              void * abort_ud, char * err, size_t errlen) {
    if (m == nullptr) {
        set_err(err, errlen, "model is not loaded");
        return nullptr;
    }
    llama_context_params params = llama_context_default_params();
    params.n_ctx           = n_ctx;
    params.n_batch         = n_batch;
    params.n_ubatch        = n_ubatch;
    params.n_seq_max       = 1;
    params.n_threads       = n_threads;
    params.n_threads_batch = n_threads;
    params.kv_unified      = kv_unified != 0;
    if (abort_cb != nullptr) {
        params.abort_callback      = reinterpret_cast<ggml_abort_callback>(abort_cb);
        params.abort_callback_data = abort_ud;
    }

    llama_context * ctx = llama_init_from_model(m->model, params);
    if (ctx == nullptr) {
        set_err(err, errlen, "llama.cpp could not create a context (out of memory?)");
        return nullptr;
    }
    auto * out = new cordial_ctx();
    out->ctx = ctx;
    return out;
}

void cordial_ctx_free(cordial_ctx * ctx) {
    if (ctx == nullptr) {
        return;
    }
    if (ctx->ctx != nullptr) {
        llama_free(ctx->ctx);
    }
    delete ctx;
}

uint32_t cordial_ctx_n_ctx(const cordial_ctx * ctx) {
    return ctx != nullptr ? llama_n_ctx(ctx->ctx) : 0;
}

int32_t cordial_ctx_pos_max(const cordial_ctx * ctx) {
    if (ctx == nullptr) {
        return -1;
    }
    llama_memory_t mem = llama_get_memory(const_cast<llama_context *>(ctx->ctx));
    return static_cast<int32_t>(llama_memory_seq_pos_max(mem, 0));
}

void cordial_ctx_trim(cordial_ctx * ctx, int32_t n_keep) {
    if (ctx == nullptr) {
        return;
    }
    llama_memory_t mem = llama_get_memory(ctx->ctx);
    llama_memory_seq_rm(mem, 0, n_keep, -1);
}

void cordial_ctx_clear(cordial_ctx * ctx) {
    if (ctx == nullptr) {
        return;
    }
    llama_memory_t mem = llama_get_memory(ctx->ctx);
    llama_memory_clear(mem, true);
}

// --------------------------------------------------------------- tokenizer

int32_t cordial_tokenize(const cordial_model * m, const char * text, int32_t add_special,
                         int32_t parse_special, int32_t * out, int32_t cap) {
    if (m == nullptr || text == nullptr) {
        return -1;
    }
    const llama_vocab * vocab = llama_model_get_vocab(m->model);
    const int32_t text_len = static_cast<int32_t>(std::strlen(text));
    return llama_tokenize(vocab, text, text_len, out, cap, add_special != 0, parse_special != 0);
}

int32_t cordial_token_to_piece(const cordial_model * m, int32_t token, char * buf, int32_t cap) {
    if (m == nullptr || buf == nullptr || cap <= 0) {
        return -1;
    }
    const llama_vocab * vocab = llama_model_get_vocab(m->model);
    return llama_token_to_piece(vocab, token, buf, cap, 0, true);
}

int32_t cordial_token_is_eog(const cordial_model * m, int32_t token) {
    if (m == nullptr) {
        return 0;
    }
    const llama_vocab * vocab = llama_model_get_vocab(m->model);
    return llama_vocab_is_eog(vocab, token) ? 1 : 0;
}

const char * cordial_token_text(const cordial_model * m, int32_t token) {
    if (m == nullptr) {
        return nullptr;
    }
    const llama_vocab * vocab = llama_model_get_vocab(m->model);
    return llama_vocab_get_text(vocab, token);
}

// ------------------------------------------------------------------ decode

int32_t cordial_decode_text(cordial_ctx * ctx, const int32_t * tokens, int32_t n, int32_t pos0,
                            int32_t logits_last_only) {
    if (ctx == nullptr || tokens == nullptr || n <= 0) {
        return -1;
    }
    llama_batch batch = llama_batch_init(n, 0, 1);
    if (batch.token == nullptr) {
        return -1;
    }
    for (int32_t i = 0; i < n; ++i) {
        batch.token[i]     = tokens[i];
        batch.pos[i]       = pos0 + i;
        batch.n_seq_id[i]  = 1;
        batch.seq_id[i][0] = 0;
        batch.logits[i]    = (logits_last_only == 0 || i == n - 1) ? 1 : 0;
    }
    batch.n_tokens = n;

    const int32_t rc = llama_decode(ctx->ctx, batch);
    llama_batch_free(batch);
    return rc;
}

float * cordial_logits(const cordial_ctx * ctx) {
    return ctx != nullptr ? llama_get_logits(const_cast<llama_context *>(ctx->ctx)) : nullptr;
}

// --------------------------------------------------------------- sampling

cordial_sampler * cordial_sampler_new(int32_t n_vocab, float temp, int32_t top_k, float top_p,
                                      float min_p, float repeat_penalty, int32_t penalty_last_n,
                                      uint32_t seed) {
    auto chain_params = llama_sampler_chain_default_params();
    chain_params.no_perf = true;

    llama_sampler * chain = llama_sampler_chain_init(chain_params);
    if (chain == nullptr) {
        return nullptr;
    }
    if (penalty_last_n > 0 && repeat_penalty > 0.0f) {
        llama_sampler_chain_add(chain, llama_sampler_init_penalties(n_vocab, penalty_last_n,
                                                                    repeat_penalty, 0.0f, 0.0f));
    }
    if (top_k > 0) {
        llama_sampler_chain_add(chain, llama_sampler_init_top_k(top_k));
    }
    if (top_p < 1.0f && top_p > 0.0f) {
        llama_sampler_chain_add(chain, llama_sampler_init_top_p(top_p, 1));
    }
    if (min_p > 0.0f) {
        llama_sampler_chain_add(chain, llama_sampler_init_min_p(min_p, 1));
    }
    if (temp > 0.0f) {
        llama_sampler_chain_add(chain, llama_sampler_init_temp(temp));
        llama_sampler_chain_add(chain, llama_sampler_init_dist(seed));
    } else {
        llama_sampler_chain_add(chain, llama_sampler_init_greedy());
    }

    auto * out = new cordial_sampler();
    out->smpl = chain;
    return out;
}

void cordial_sampler_free(cordial_sampler * s) {
    if (s == nullptr) {
        return;
    }
    if (s->smpl != nullptr) {
        llama_sampler_free(s->smpl);
    }
    delete s;
}

void cordial_sampler_reset(cordial_sampler * s) {
    if (s != nullptr && s->smpl != nullptr) {
        llama_sampler_reset(s->smpl);
    }
}

int32_t cordial_sampler_sample(cordial_sampler * s, cordial_ctx * ctx) {
    if (s == nullptr || ctx == nullptr) {
        return -1;
    }
    return llama_sampler_sample(s->smpl, ctx->ctx, -1);
}

// ------------------------------------------------------------------- mtmd

cordial_mtmd * cordial_mtmd_load(const cordial_model * m, const char * mmproj, int32_t n_threads,
                                 char * err, size_t errlen) {
    if (m == nullptr || mmproj == nullptr) {
        set_err(err, errlen, "vision projector path is empty");
        return nullptr;
    }
    mtmd_context_params params = mtmd_context_params_default();
    params.use_gpu      = true;
    params.n_threads    = n_threads;
    params.print_timings = false;
    params.warmup       = false;

    mtmd_context * ctx = mtmd_init_from_file(mmproj, m->model, params);
    if (ctx == nullptr) {
        set_err(err, errlen, "llama.cpp could not load the vision projector");
        return nullptr;
    }
    auto * out = new cordial_mtmd();
    out->ctx = ctx;
    return out;
}

void cordial_mtmd_free(cordial_mtmd * mm) {
    if (mm == nullptr) {
        return;
    }
    if (mm->ctx != nullptr) {
        mtmd_free(mm->ctx);
    }
    delete mm;
}

int32_t cordial_mtmd_has_vision(const cordial_mtmd * mm) {
    return (mm != nullptr && mtmd_support_vision(mm->ctx)) ? 1 : 0;
}

const char * cordial_mtmd_marker(const cordial_mtmd * mm) {
    return mm != nullptr ? mtmd_get_marker(mm->ctx) : nullptr;
}

int32_t cordial_eval_prompt(cordial_mtmd * mm, cordial_ctx * ctx, const char * text,
                            const char * const * media_paths, int32_t n_media, int32_t n_past,
                            int32_t logits_last, char * err, size_t errlen) {
    if (mm == nullptr || ctx == nullptr || text == nullptr) {
        set_err(err, errlen, "vision prompt called without a loaded projector");
        return -1;
    }

    std::vector<mtmd_bitmap *>                 bitmaps;
    std::vector<mtmd_helper_bitmap_wrapper>    wrappers;
    bitmaps.reserve(static_cast<size_t>(n_media));

    const mtmd_helper_init_opt opt = mtmd_helper_init_opt_default();
    for (int32_t i = 0; i < n_media; ++i) {
        if (media_paths == nullptr || media_paths[i] == nullptr) {
            continue;
        }
        mtmd_helper_bitmap_wrapper w =
            mtmd_helper_bitmap_init_from_file(mm->ctx, media_paths[i], false, opt);
        if (w.bitmap == nullptr) {
            for (auto * b : bitmaps) {
                mtmd_bitmap_free(b);
            }
            set_err(err, errlen, "could not decode an attached image");
            return -1;
        }
        wrappers.push_back(w);
        bitmaps.push_back(w.bitmap);
    }

    mtmd_input_chunks * chunks = mtmd_input_chunks_init();
    if (chunks == nullptr) {
        for (auto * b : bitmaps) {
            mtmd_bitmap_free(b);
        }
        set_err(err, errlen, "could not allocate multimodal chunks");
        return -1;
    }

    mtmd_input_text input;
    input.text          = text;
    input.text_len      = std::strlen(text);
    input.add_special   = false;
    input.parse_special = true;

    int32_t rc = mtmd_tokenize(mm->ctx, chunks, &input, bitmaps.data(), bitmaps.size());
    if (rc != 0) {
        mtmd_input_chunks_free(chunks);
        for (auto * b : bitmaps) {
            mtmd_bitmap_free(b);
        }
        set_err(err, errlen, "could not tokenize the multimodal prompt");
        return -1;
    }

    llama_pos new_n_past = n_past;
    rc = mtmd_helper_eval_chunks(mm->ctx, ctx->ctx, chunks, n_past, 0,
                                 static_cast<int32_t>(llama_n_batch(ctx->ctx)),
                                 logits_last != 0, &new_n_past);

    mtmd_input_chunks_free(chunks);
    for (auto * b : bitmaps) {
        mtmd_bitmap_free(b);
    }

    if (rc != 0) {
        set_err(err, errlen, "multimodal evaluation failed");
        return -1;
    }
    return static_cast<int32_t>(new_n_past);
}
