# Security and privacy

Cordial runs a language model on the user's machine and lets it act. That
combination deserves a written threat model rather than good intentions.

## What the agent can do

| Tool | Reach | Permission |
| --- | --- | --- |
| `web_search` | Sends a query string to DuckDuckGo | Auto: allowed |
| `web_fetch` | GETs a URL, returns text | Auto: allowed |
| `browser_open` | Launches the installed Chrome/Chromium/Edge headless | Auto: allowed |
| `browser_click`, `browser_type`, `browser_view` | Interacts with the current page | Auto: allowed |
| `read_file`, `list_files` | Reads inside the workspace folder | Auto: allowed |
| `write_file`, `docx_edit`, `docx_create` | Writes inside the workspace folder | Auto: asks once per tool, then remembers the grant |
| `run_command` | Built-in read-only operations in workspace, no shell | Auto: allowed |
| `docx_read` | Reads DOCX paragraphs in workspace | Auto: allowed |
| `update_plan` | Updates the conversation checklist | Auto: allowed |
| unknown tool | — | Treated as a write, asks first |

`PermissionMode::Manual` asks before every call. `PermissionMode::Auto` asks only
for writes, and remembers a grant for the rest of the session.

## Boundaries that are enforced, not suggested

- **Filesystem.** `tools/files.rs` refuses absolute paths, refuses `..`
  traversal and rejects existing symlink components. These checks are path-based,
  not an OS sandbox: another local process changing paths concurrently can race
  them. Use a trusted workspace and do not treat this as isolation from a
  malicious local user. Reads are capped at 1.5 MB and
  rejected when the first 8 KB look binary.
- **Network.** Direct tool URLs must use `http` or `https`, and HTTP redirects
  are limited to 8 hops. This is not network isolation: URLs, redirects, DNS,
  and browser page requests can reach localhost or private networks. Do not use
  web or browser tools with untrusted content where internal services are
  sensitive. Browser pages can make additional requests and follow redirects.
- **Browser.** The browser runs headless with its own throwaway profile in the
  app-data scratch folder, `--mute-audio --no-first-run`, and is killed when the
  session ends. It is the user's own browser binary: Cordial does not ship one.
- **Renderer.** All model output is rendered through DOMPurify with a tag
  allowlist and no data attributes (`ui/src/markdown.ts`). Model output is treated
  as untrusted, because a fetched page can steer it.
- **Tauri surface.** The capability file grants only window dragging, the file
  picker, and opening paths; there is no filesystem or shell plugin enabled.
  The send-message command also accepts bounded locally rendered PDF pages,
  written only into generated scratch paths. Scratch confinement uses path-based
  checks and is not isolation against a hostile local process. CSP restricts
  scripts to `'self'`, images to `self`/`asset:`/`data:`.

## Experimental Python sandbox

An opt-in WASI Python prototype is available to the live test harness and via
an experimental desktop build feature, but is **not enabled in default builds**.
It uses a pinned CPython module, no preopened host directories, no inherited
host environment or standard input, bounded output, memory and instruction
fuel. The module is integrity-checked and scripts are passed in memory rather
than written to disk. These controls depend on the WASI runtime and operating
system; they do not guarantee absolute safety. Packaging, cancellation,
resource limits and isolation still require further cross-platform testing.

## Prompt injection

A web page the agent reads can contain instructions ("ignore your previous
instructions…"). Cordial cannot make a 2B model immune to this, so the design
limits the blast radius instead:

1. Tool results are labelled (`<tool_response>`) and untrusted by construction.
2. Anything that writes needs explicit approval, in both permission modes.
3. File tools are restricted to the chosen workspace, subject to the filesystem
   race limitation above. Users can select a broader folder, including their home.
4. The system prompt tells the model that page content is data, not instructions.

A reviewer should treat "the model reads a hostile page and asks to write a file"
as the interesting case: it produces an approval card, and the approval card shows
the exact path and preview.

## Data on disk

| Path | Contents |
| --- | --- |
| `<data>/Cordial/settings.json` | preferences, model paths |
| `<data>/Cordial/sessions/*.json` | full conversations, including tool output |
| `<data>/Cordial/workspace/` | files the agent may read and write |
| `<data>/Cordial/scratch/` | screenshots and throwaway browser profiles |
| `<data>/Cordial/models/` | GGUF weights, checksummed on download |

Nothing is encrypted at rest: the threat model is the local user's account, not
full-disk compromise. `Delete model files` in Settings removes the weights;
deleting the chat from the sidebar removes that session file.

## What is *not* sent anywhere

No telemetry, no analytics, no crash reports, no update pings, no remote logging.
llama.cpp runs in-process; there is no inference server listening on a port.

## Known limitations

- The model can still be wrong. The system prompt pushes it to cite sources and
  to say when it cannot check something; it cannot guarantee truth.
- Search uses DuckDuckGo's HTML endpoints, which are unofficial and may rate
  limit. The tool reports failures to the model instead of failing the turn.
- User-selected PDF attachments are rendered locally by PDF.js in the webview.
  They are limited to 10 MB and three pages, with at most 4,000 extracted
  characters per page. Each page image and text are stored unencrypted under
  `scratch/pdf-attachments/` so saved sessions can refer to them; storage is
  capped at 100 MB. No OCR, form filling, complete layout fidelity, or model
  accuracy is guaranteed. Large documents are refused rather than partially
  attached. Parsing an untrusted PDF still carries risks. Web-fetch PDFs and
  workspace PDF reads are not supported.
- The browser driver speaks CDP directly and targets Chromium-family browsers
  only. Safari and Firefox are not supported.

## Task mode limitations

Task mode is experimental. The checklist reflects the model’s own reports and is
not proof of correctness. Work stops when the app exits, when its budget runs
out, or when the model fails; checkpointing does not automatically resume it.
DOCX support handles a bounded subset of WordprocessingML, not all document
features. Review output documents and back up originals. Real-model held-out
complex-task testing is still required before claiming reliable completion.
