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
| `write_file` | Writes inside the workspace folder | Always asks first |
| unknown tool | — | Treated as a write, asks first |

`PermissionMode::Manual` asks before every call. `PermissionMode::Auto` asks only
for writes, and remembers a grant for the rest of the session.

## Boundaries that are enforced, not suggested

- **Filesystem.** `tools/files.rs` refuses absolute paths, refuses `..`
  traversal after lexical resolution, and re-checks the canonicalised parent, so a
  symlink cannot walk out of the workspace. Reads are capped at 1.5 MB and
  rejected when the first 8 KB look binary.
- **Network.** Only `http` and `https` URLs are fetched. Redirects are limited to
  8 hops. There is no request to `file://`, `localhost` via the tools, or any
  protocol handler the app did not implement.
- **Browser.** The browser runs headless with its own throwaway profile in the
  app-data scratch folder, `--mute-audio --no-first-run`, and is killed when the
  session ends. It is the user's own browser binary: Cordial does not ship one.
- **Renderer.** All model output is rendered through DOMPurify with a tag
  allowlist and no data attributes (`ui/src/markdown.ts`). Model output is treated
  as untrusted, because a fetched page can steer it.
- **Tauri surface.** The capability file grants only window dragging, the file
  picker, and opening paths; there is no filesystem or shell plugin enabled.
  CSP restricts scripts to `'self'`, images to `self`/`asset:`/`data:`.

## Prompt injection

A web page the agent reads can contain instructions ("ignore your previous
instructions…"). Cordial cannot make a 2B model immune to this, so the design
limits the blast radius instead:

1. Tool results are labelled (`<tool_response>`) and untrusted by construction.
2. Anything that writes needs explicit approval, in both permission modes.
3. The workspace is a sandbox folder, not the home directory.
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
- PDFs are not parsed. The fetch tool says so rather than returning binary noise.
- The browser driver speaks CDP directly and targets Chromium-family browsers
  only. Safari and Firefox are not supported.
