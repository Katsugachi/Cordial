<p align="center"><img src="assets/brand/icon-preview.png" width="96" alt="" /></p>

# Cordial

A private agent that runs on your machine. It answers questions, checks facts on
the web, drives a browser, reads your files, and looks at images, powered by
[LFM3-VL-2B](https://huggingface.co/LiquidAI/LFM3-VL-2B) through
[llama.cpp](https://github.com/ggml-org/llama.cpp)

Cordial is a desktop app: a thin Rust/Tauri shell around a local agentic harness.
On first launch it downloads the Q4_K_M weights (1.1 GB) plus the vision
projector (203 MB), and from then on the network is used only when the agent
searches or browses for you.  <br><br>
The X post from LiquidAI said it was almost as capable as GPT 5.6 Luna and i guess if it's stable enough to be in Cordial that's good.

## What it does

- **Answers with sources.** The system prompt pushes the model to search before it
  states a fact, to open the best result, and to cite links inline.
- **Uses tools honestly.** `web_search`, `web_fetch`, and a real browser driven
  over the Chrome DevTools Protocol (`browser_open`, `browser_click`,
  `browser_type`, `browser_view`). The browser returns both page text and a
  screenshot, so a 2B vision model can *see* what it is doing.
- **Handles what you give it.** Images go to the vision encoder; text files are
  folded into the prompt; the model can read, list and write files inside a
  sandboxed workspace folder.
- **Asks before it acts.** Two permission modes — Auto (reads and network run,
  writes ask once) and Manual (everything asks) — with approvals rendered inline.
- **Keeps answers short.** Effort levels (Low / Medium / High) cap tool rounds,
  output tokens and page reads. Low effort also switches reasoning off. The
  default prompt asks for the answer first, no preamble, and at most three links.
- **Runs offline.** No telemetry, no analytics, no accounts. Nothing leaves the
  machine unless a web tool is called.


## Install

Download the installer for your platform from the releases page:

| Platform | Bundle | Backend |
| --- | --- | --- |
| macOS 11+ | `.dmg` (arm64, x64) | Metal |
| Windows 10/11 x64 | `.msi` / `.exe` | CPU |
| Windows 11 arm64 | `.msi` | **OpenCL**, CPU fallback |
| Linux x64 / arm64 | `.deb` / `.AppImage` | CPU |

On first launch Cordial downloads the model into your app-data folder:

| OS | Location |
| --- | --- |
| macOS | `~/Library/Application Support/Cordial/models` |
| Windows | `%APPDATA%\Cordial\models` |
| Linux | `~/.local/share/Cordial/models` |

Downloads resume if interrupted and are verified against the SHA-256 Hugging
Face publishes for the file (pinned in `crates/cordial-core/src/model_manager.rs`,
not read from a response header).

### Removing model downloads

Use **Remove downloaded model** in the sidebar before uninstalling. It deletes
only Cordial-managed model files and leaves chats and workspace files intact.
The Windows NSIS uninstaller in newer releases also removes these model files,
except during updates. MSI, macOS app removal, Linux packages and AppImages do
not automatically remove per-user model downloads. For those, use the sidebar
first or delete the `models` directory listed above after uninstalling.
Previously installed versions do not have the new NSIS uninstall hook.

## Build from source

Requirements: Rust 1.83+, Node 22+, CMake 3.16+, a C++17 compiler, and on Linux
the Tauri packages (`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`,
`libjavascriptcoregtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`).

```bash
git clone https://github.com/Katsugachi/Cordial && cd Cordial
npm --prefix ui ci
npm --prefix ui run build
cargo run -p cordial-app --release
```

The first `cargo build` downloads llama.cpp `b12020`, verifies its SHA-256, and
builds static `llama`, `ggml` and `mtmd` libraries — about five minutes on two
cores. Set `CORDIAL_LLAMA_SRC=/path/to/llama.cpp` to reuse an existing checkout,
and `CORDIAL_PORTABLE_CPU=1` to build without `-march=native` (what the release
workflow does).

### UI in a browser

The window runs without Tauri against a demo bridge:

```bash
npm --prefix ui run dev     # http://localhost:1420
```

## Using it

- **Ask.** Enter sends, Shift + Enter adds a line. Answers stream; reasoning
  appears in a collapsed drawer.
- **Attach.** The paperclip accepts images (sent to the vision encoder) and text
  files (read into the prompt).
- **Approve.** In Manual mode each tool call shows a card with Allow and Deny.
  Denials are reported back to the model so it can adapt instead of retrying.
- **Memory.** Add or remove personal context in the sidebar. It is stored locally
  and included in new turns across chats. Cordial does not automatically learn
  or extract memories; entries are plain text, not encrypted.
- **Skills.** Add reusable instructions in the sidebar. They apply to new turns
  and do not bypass tool permissions. You can import a SKILL.md or text file
  into the single instruction field (maximum 4,000 characters). This does not
  support skill folders, bundled resources, scripts, or automatic skill discovery.
- **Stop.** The stop control cancels token generation through llama.cpp's abort
  callback, not just the UI.
- **Effort.** Low (≤3 tool rounds, 512 output tokens, no reasoning), Medium
  (≤8, 1024), High (≤16, 2048, deeper page reads).

## Tests

```bash
cargo test --workspace                 # harness + prompt + tools (no model needed)
npm --prefix ui run test               # UI reducer, renderer, window integration
cargo test -p cordial-core --test live_model -- --ignored --nocapture
```

The harness tests drive the real agent loop with a scripted model: tool-call round
trips, approvals, denials, effort exhaustion, context-overflow trimming and
cancellation all run deterministically in milliseconds.

The ignored `live_model` suite downloads the real weights and asserts on
behaviour: correct facts, arithmetic, reading a workspace file, honesty when the
web is off, brevity, absence of filler, and recognising the colour of an attached
image. It is the suite to run after changing the system prompt or sampler.

Each live run publishes what the model actually said to
[`docs/live-report.md`](docs/live-report.md), so prompt quality can be reviewed
without a local GPU or a 1.3 GB download.
Cordial is MIT licensed ([`LICENSE`](LICENSE)). LFM3-VL-2B weights are released
by Liquid AI under the [LFM Open License v1.0](https://huggingface.co/LiquidAI/LICENSE);
llama.cpp is MIT. Cordial downloads weights at runtime and does not redistribute
them.
