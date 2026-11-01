# Review guide

What to read, in what order, and what to push back on. This is written for a
senior engineer picking the project up cold.

## Read in this order

| # | File | Why |
| --- | --- | --- |
| 1 | `crates/cordial-core/src/agent.rs` | The whole product in one loop: generate, parse, permit, execute, repeat. |
| 2 | `crates/cordial-core/src/prompt.rs` | LFM3's ChatML, reproduced exactly, with golden tests. |
| 3 | `crates/cordial-core/src/backend.rs` | The inference worker: threads, KV reuse, cancellation, token streaming. |
| 4 | `crates/cordial-llama/csrc/cordial_llama.cpp` | The only unsafe edge; ~40 functions with no llama.cpp structs in Rust. |
| 5 | `crates/cordial-core/src/tools/` | Search, fetch, CDP browser, sandboxed files, HTML→text. |
| 6 | `ui/src/store.ts` + `ui/src/app.ts` | Pure reducer plus composition root; no framework, no virtual DOM. |
| 7 | `docs/PROMPT.md`, `docs/SECURITY.md` | Why the prompt says what it says; what the agent may and may not do. |

## Claims this code makes, and the tests behind them

| Claim | Evidence |
| --- | --- |
| The prompt matches LFM3's template | `prompt.rs` unit tests assert exact ChatML strings, including the thinking rules and `<tool_response>` wrapping |
| Tool calls survive a 2B model's typos | `protocol.rs` tests: missing close tags, quoted names, unterminated blocks, multi-line parameters |
| Approvals cannot deadlock or race | `ApprovalGate` registers the wait before announcing the request; `agent_flow.rs` covers allow, deny and auto-grant |
| Context overflow is handled, not fatal | `agent_flow.rs::context_overflow_trims_old_turns_and_retries` |
| A tool budget still produces an answer | `agent_flow.rs::tool_budget_triggers_a_final_answer` |
| The file sandbox cannot be escaped | `files.rs` tests: `..`, absolute paths, symlink parents rejected |
| Model output cannot inject script | `render.test.ts`: DOMPurify tag allowlist, `onerror` stripped |
| The window works end to end | `app.test.ts` mounts the app against a demo bridge and drives send, stream, approve, settings |
| The model is actually helpful | `live_model.rs` against real weights, with the transcript published in `docs/live-report.md` |

Run everything: `make test && make test-ui`, then `make live` when changing the
prompt.

## Design decisions worth arguing about

**No inference framework, direct llama.cpp.** `llama-cpp-2`, `mistralrs` or
`candle` would remove the build script, but each lags upstream llama.cpp — and
LFM3-VL support is days old. Pinning a release and writing ~40 lines of shim
means Cordial gets `mtmd` and the LFM3 vocabulary the day they land.

**A C++ shim instead of `bindgen`.** Structs like `llama_context_params` gain
fields every few weeks. Generated bindings silently mis-read the layout when that
happens; the shim cannot drift, because C++ reads the real headers.

**`std::sync::mpsc` worker thread instead of async.** llama.cpp decode is
blocking, single-threaded CPU work. A dedicated thread plus channels keeps it off
the runtime and gives cancellation a clean seam (an `AtomicBool` read by
llama.cpp's abort callback).

**No frontend framework.** The window has one state machine and one list. A
150-line reducer with no dependency is easier to review than a React tree, and it
keeps the bundle at ~38 kB gzipped.

**Effort instead of a token slider.** Three named budgets are easier to reason
about than a raw number, and they map to the thing users care about (heat, time,
depth). Low effort also disables reasoning, which is the single biggest token
saving.

## Known limitations (not bugs, but be aware)

1. **Prompt injection is mitigated, not solved.** A 2B model can be talked into
   strange things by a hostile page. The sandbox, approvals and `<tool_response>`
   framing bound the damage; they do not prevent it. See `SECURITY.md`.
2. **Search depends on DuckDuckGo's HTML endpoints.** No API key, no cost, no
   stability guarantee. Failures are reported to the model, which is told to say
   it could not check.
3. **PDFs are not parsed.** The tool says so rather than returning binary noise.
4. **The browser is Chromium-only.** It uses the user's installed Chrome, Chromium,
   Edge or Brave over CDP; Safari and Firefox are not supported.
5. **CPU-only on Windows x64 and Linux.** Metal on macOS, OpenCL on Windows ARM64.
   CUDA and Vulkan are one flag away (`--features`), but are not shipped because
   they multiply the release matrix.
6. **Images are dropped from prompts older than two user turns.** This keeps long
   browsing sessions inside the context window; it also means the model cannot
   look back at a screenshot from five turns ago.
7. **`cargo fmt` runs in CI, not locally.** Contributions are formatted by
   `.github/workflows/format.yml`; a local `rustfmt` of a different version would
   fight it.

## What I would do next

1. **Fine-tune the tool-call format** on Cordial's own transcripts (LFM3 supports
   TRL SFT). A 2B model that always emits clean `<tool_call>` blocks would remove
   most of `protocol.rs`'s forgiveness.
2. **Stream structured tool calls** instead of waiting for the closing tag, so a
   slow first token does not delay the card appearing.
3. **KV cache persistence across launches** (`llama_state_seq_save_file`), so a
   reopened conversation does not re-evaluate its history.
4. **A real fix for prompt injection**: pin a per-session nonce into the system
   prompt and require it in tool arguments that mutate state.

## Reviewing the UI

Open `npm --prefix ui run dev` — without Tauri the app runs on the demo bridge,
so the whole interface (transcript, tool cards, approvals, settings, download
screen) can be exercised in a browser. The header says "Demo preview" so it can
never be mistaken for a running model.

Design follows the Tuck style: paper `#f4f3eb`, ink `#21352e`, flat accent fills,
hairline rules, 4px spacing rhythm, two typefaces, no shadows or gradients. The
checklist in the brief maps to `ui/src/style.css`; every colour there is a token.
