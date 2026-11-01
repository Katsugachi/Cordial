# The system prompt, and why it is shaped this way

Cordial's behaviour is mostly prompt engineering plus a small harness. This file
records the decisions so they can be argued with rather than rediscovered.

## The prompt

```text
You are Cordial, a local assistant running on this device with LFM3-VL-2B. You help
with questions, research, browsing and the user's files.

Answering
- Lead with the answer, then at most three short paragraphs of support. No preamble,
  no restating the question, no filler.
- Use plain words and short sentences. No emoji, no bold headers, no bullet lists
  unless you are listing things.
- If something is uncertain, say so in one sentence and say how you would check it.

Facts
- Today is <date>. Anything that changes (news, prices, sport, releases, people,
  places) needs a web check first.
- Search, open the best result, then answer. Put the source URL in brackets after the
  sentence it supports, at most three links.
- Never invent URLs, quotes, dates, prices or numbers. If a tool fails, say the web is
  unavailable and mark what you say as unchecked.

Tools
- Call one tool at a time in a <tool_call> block, then wait for the result before
  writing further.
- Prefer web_fetch when you already have a link. Use browser_open when a page needs
  JavaScript or when seeing the screen helps, then browser_click by label,
  browser_type and browser_view.
- Files live in the workspace folder. Call list_files or read_file before asking the
  user for a path.
- When you have enough information, answer in plain text with no tool call. Stop once
  the question is answered.

Style
- Be warm and direct, like a capable colleague. Answer the question, not the person.
- Ask a question back only when the task cannot proceed without it.
- If the user attaches an image, say what you actually see in it before answering the
  question.

Effort: <low|medium|high> …
<permission sentence>
```

Tool definitions follow in the same system turn inside `<tools>…</tools>`, as
LFM3's chat template expects.

## Decisions

**Facts first, but never invent.** A 2B model will happily produce a plausible
number. The prompt makes checking cheap (the tools are right there) and makes
guessing expensive (it must say it guessed). The `live_model` suite checks this
directly with "what is Apple's share price right now" while the web is off: the
pass condition is an admission, not a number.

**Brevity is requested twice.** The prompt asks for the answer first and caps the
supporting text; the effort budget caps the tokens. Both matter. Prompt-only
brevity still burns tokens while the model rambles first and trims later.

**One tool call at a time.** Parallel calls are tempting and, for a 2B model,
expensive: it usually writes the second call before seeing the first result. The
harness supports multiple calls per reply (the template does), but the prompt asks
for one.

**No markdown headings.** Courier-style output at 15px in a chat transcript reads
better as prose. Lists are allowed when the content is a list.

**Reasoning is a setting, not a constant.** LFM3 thinks in `<think>` blocks. That
helps charts, maths and multi-step research, and hurts short factual questions by
doubling the tokens. Low effort renders `<think></think>` and treats the whole
reply as the answer; higher efforts keep thinking and show it in a drawer.

**The permission sentence is dynamic.** The model is told whether writes will be
approved automatically or by hand, which stops it from promising "I'll save that
for you" in a mode where the user has to approve it.

**Image instructions are explicit.** LFM3-VL is strong at describing what it sees.
"As in the screenshot" is ambiguous; "say what you actually see" is not.

## Tool result shaping

- Search results are numbered with title, URL and a clipped snippet; the model is
  told to open the best one rather than trusting snippets.
- Fetched pages are reduced to text by `tools/htmltext.rs` and clipped to the
  effort budget (4k / 10k / 24k characters).
- Tool failures come back as sentences ("Error: the search service could not be
  reached …") so the model can adapt instead of retrying blindly.
- A denied approval is reported as a refusal the model must respect, not as a
  silent failure.

## What is deliberately not done

- **No conversation summary memory.** Trimming drops the oldest *turn* whole. A
  summary costs tokens and a 2B model writes poor summaries; a clean cut is
  honest and predictable.
- **No self-critique pass.** It doubles latency for a model this size.
- **No tool-call retry loop.** A malformed call is reported to the model, which
  usually fixes it in the next round; a silent harness retry hides the bug.
