# Contributing

Thanks for taking a look. Cordial is small on purpose; these notes cover the
conventions that keep it that way.

## Getting set up

```bash
git clone https://github.com/Katsugachi/Cordial && cd Cordial
npm --prefix ui ci
cargo test --workspace --exclude cordial-app   # no model needed, ~1 min
npm --prefix ui run test
cargo run -p cordial-app                       # builds llama.cpp on first run
```

Formatting is applied automatically by CI (`.github/workflows/format.yml`) and the
lockfile is refreshed there too, so you do not need a matching local `rustfmt`.

## Rules of the road

1. **The prompt is code.** Changes to `crates/cordial-core/src/prompt.rs` need a
   golden-string test. It mirrors the checkpoint's `chat_template.jinja`; if the
   template changes, the tests are the place to notice.
2. **No new heavyweight dependencies without a note in the PR.** The release
   binary statically links llama.cpp, so every crate is paid for by every user.
3. **Tools declare risk.** A new tool needs a `Risk` in `tools/mod.rs`, a
   description short enough for a 2B model, and a failed outcome rather than a
   panic when things go wrong.
4. **The UI reducer stays pure.** `ui/src/store.ts` must not touch the DOM; all
   rendering lives in `ui/src/views`. This is what makes the window testable.
5. **Design follows the Tuck style.** Palette tokens from `ui/src/style.css` only:
   flat fills, hairline rules, no shadows, no gradients, no emoji.
6. **Untrusted output is sanitised.** Anything the model produces goes through
   `renderMarkdown` (DOMPurify, tag allowlist) before it reaches the DOM.

## Changing the model or llama.cpp

- Weights, quantisation and projector live in `crates/cordial-core/src/model_manager.rs`.
- The llama.cpp pin is `LLAMA_TAG` + `LLAMA_SHA256` in
  `crates/cordial-llama/build.rs`. Bump both, then run `make llama` and the live
  suite.
- After changing the system prompt, run:

  ```bash
  CORDIAL_MODEL_DIR=/path/to/models cargo test -p cordial-core --test live_model -- --ignored --nocapture
  ```

  The suite prints every answer, so prompt regressions are visible, not just
  failing assertions.

## Pull requests

- Keep the diff focused; one concern per PR.
- `cargo test`, `npm --prefix ui run test` and the app compile check must pass.
- If you change behaviour a user can see, say what it looks like now.
