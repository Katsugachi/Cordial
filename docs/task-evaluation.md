# Task mode evaluation

Task mode is experimental and is not yet validated for autonomous complex or
long-running work. The generated live test in `crates/cordial-core/tests/live_model.rs`
creates a small CSV calculation and a two-question Word worksheet at runtime.
A separate Python verifier checks item totals and reads the output Word XML
independently of Cordial's DOCX tools. The test also requires a created,
progressing and completed checklist. It uses a disposable workspace and
approves writes in the test harness; this is not the app's permission policy.

These are smoke tests, not a sufficiently complex, fully unseen, research-based
benchmark. They do not establish correctness for arbitrary worksheets, preserved
Word formatting, long tasks, task recovery after application exit, or hostile
local filesystem races.

In [run 41729131338](https://github.com/Katsugachi/Cordial/actions/runs/41729131338),
the independent verifier accepted both output artifacts, but the live test
failed because neither task completed a checklist: the CSV plan remained
unfinished and the Word task had no plan. Earlier runs also failed checklist
progression. The ordinary Rust, UI, desktop and inference-stack jobs passed in
that run. Do not treat successful arithmetic artifacts as proof that Task mode
completed the requested autonomous workflow.

The exact user-supplied PDF was tested in [run 41794020560](https://github.com/Katsugachi/Cordial/actions/runs/41794020560).
The agent did not read the worksheet, run Python, or solve it. It reported the
work blocked. PDF attachments and sandboxed Python execution are not currently
supported; PDF diagrams require visual rendering, not text extraction alone.
The fixture was removed from the current branch after the test, but remains in
Git history, with the user's consent. This failed test does not validate other
subjects or research tasks. A separate `research_worksheets_architecture_and_english`
live smoke test exercises web search, opening and citing a source, a generated
geometry calculation, and literary analysis. It is triggered with `[research]`
and independently checks selected answers; it is not a comprehensive held-out
benchmark.
