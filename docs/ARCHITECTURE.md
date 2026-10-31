# Architecture

Cordial is three crates and one web view. This document explains the seams, so a
reviewer can judge the design rather than reverse-engineer it.

```
┌──────────────────────────── ui/ (TypeScript) ────────────────────────────┐
│ app.ts        composition root: renders the shell, dispatches actions     │
│ store.ts      pure reducer: AgentEvent + AppState -> AppState (tested)    │
│ bridge.ts     the only Tauri binding; a demo bridge replaces it in a      │
│               browser so the interface can be reviewed without a build    │
└───────────────────────────────▲──────────────────────────────────────────┘
                                │ invoke commands / cordial://event stream
┌─────────────────────────── cordial-app ──────────────────────────────────┐
│ commands.rs   bootstrap, send_message, approve_tool, cancel_turn,         │
│               sessions, settings, model lifecycle                         │
│ state.rs      Core (engine, tools, policy, store) behind one mutex;        │
│               approvals and cancellation stay outside it so they answer    │
│               while the model generates                                    │
└───────────────────────────────▲──────────────────────────────────────────┘
                                │ Agent::run(...)
┌─────────────────────────── cordial-core ─────────────────────────────────┐
│ agent.rs      the loop: render → generate → parse → permission → tools    │
│ prompt.rs     LFM3 ChatML rendering (golden-tested against chat_template) │
│ protocol.rs   parses <think>/<tool_call> output, forgiving of a 2B model  │
│ backend.rs    inference worker thread + scripted backend for tests        │
│ tools/        web search/fetch, CDP browser, sandboxed files, HTML→text    │
│ permissions   Auto/Manual policy with session grants                      │
│ session.rs    message model and JSON store                                │
│ model_manager first-run download, resume, checksum                        │
└───────────────────────────────▲──────────────────────────────────────────┘
                                │ C ABI (~40 functions)
┌─────────────────────────── cordial-llama ────────────────────────────────┐
│ build.rs      fetches llama.cpp b12020 (SHA-256 pinned), builds static    │
│               llama/ggml/mtmd with Metal, OpenCL, Vulkan or CPU, then     │
│               compiles the shim                                            │
│ csrc/*.cpp    the only place llama.cpp structs exist                      │
│ src/lib.rs    safe wrappers: Model, Context, Sampler, Mtmd                │
└──────────────────────────────────────────────────────────────────────────┘
```

## Why a C++ shim instead of bindgen

`llama_model_params`, `llama_context_params` and `mtmd_context_params` change
between llama.cpp releases: fields are added, booleans are packed at the end of
the struct, and callbacks appear and disappear. A generated binding silently
mis-reads those layouts when a field moves, which shows up as a crash during
model load with no useful message. The shim takes the structs as C++ (where the
headers are the source of truth), fills in defaults with
`llama_model_default_params()` and friends, and exposes only integers, strings and
opaque pointers. Upstream bumps cannot shift a Rust struct because there is none.

## The turn loop

```
user message
  └─ render prompt (system + tools + messages, images as <__media__> markers)
     └─ worker: tokenize → reuse KV prefix → decode → sample
        ├─ no <tool_call>  → assistant message, done
        └─ <tool_call>…    → for each call:
                             permission? (Auto/Manual → maybe approval card)
                             execute → <tool_response> message
                           loop until the effort budget is spent
```

Three guards live in the loop rather than in callers:

- **Context overflow.** `llama_decode` rejects a prompt that does not fit. The
  loop then drops the oldest complete turn and retries (up to four times), and
  says so in the transcript.
- **Empty answers.** If the model spends the whole budget inside `<think>`, the
  loop retries once with reasoning disabled instead of showing a blank bubble.
- **Budget exhaustion.** After the last tool round the model is asked once more
  with the tool list removed and an instruction to summarise, so the user always
  gets prose.

## KV reuse

`Backend::generate` keeps the exact token sequence that was decoded. On the next
turn the prompt shares a prefix with it (system prompt, tool schemas, prior
turns), so the worker trims the cache to the common prefix and decodes only the
new suffix. Without this, every follow-up question in a 128k context would cost a
full prompt evaluation. Image turns are the exception: `mtmd` owns those
embeddings, so a turn with images clears the cache and starts fresh.

## Multimodal

Images reach the model as mtmd chunks. The prompt contains one `<__media__>`
marker per image, and `Context::eval_multimodal` hands the text plus image paths
to `mtmd_tokenize` and `mtmd_helper_eval_chunks`. Screenshots from the browser
are attached to the tool result and therefore to the *next* model turn — that is
how "open a page, look at it, click the right thing" works with a 2B model.

Only the two most recent user turns keep their images (`keep_image_turns`), so a
long browsing session does not re-encode old screenshots forever.

## Effort and heat

The device-heating concern is addressed in three places:

1. **Token budgets.** `Effort::max_output_tokens` caps generation per round
   (512 / 1024 / 2048), and `max_tool_rounds` caps rounds (3 / 8 / 16).
2. **Prompt discipline.** The system prompt asks for the answer first, forbids
   preamble and padding, and caps links at three.
3. **Low effort skips reasoning.** `thinking = false` renders
   `<think></think>` and the parser treats the whole reply as the answer, which
   removes the largest source of wasted tokens on short questions.

Threads default to `cores − 1` so the machine stays responsive; the setting is
exposed for users who want to trade battery for speed.

## Testing strategy

| Layer | How it is tested |
| --- | --- |
| prompt rendering | golden strings from `chat_template.jinja` |
| output parsing | think/tool-call edge cases, including truncation |
| tools | HTML→text, search parsing, path sandboxing, PNG/binary rejection |
| agent loop | scripted backend: approvals, denials, budget, overflow, cancel |
| UI | reducer, renderer, and a mounted window against the demo bridge |
| model behaviour | `live_model` suite against the real weights (ignored by default) |

The scripted backend exists because the interesting failures in an agent are
control-flow failures (an approval that never resolves, a loop that never ends),
and those should not require a 1.3 GB download to reproduce.
