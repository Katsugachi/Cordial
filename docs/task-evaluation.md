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
work blocked. That run predated the PDF attachment renderer; the renderer now
prepares bounded page images and text, but still needs end-to-end model
validation. The follow-up rendered-page evaluation on Instabox (run 41815625401)
reached a three-question checklist but exhausted a 12-round test budget without
solving the worksheet or running Python. The ordinary desktop, UI, Rust and
inference checks passed. A prior run was superseded after more than 40 minutes
without a final result. With the experimental Python tool available in the
live harness, [run 41908510800](https://github.com/Katsugachi/Cordial/actions/runs/41908510800)
again exhausted its budget: the model repeatedly updated a pending checklist
and did not use Python. In [run 41914447409](https://github.com/Katsugachi/Cordial/actions/runs/41914447409)
the model identified the diagram dimensions and executed Python in the isolated
runtime, but its final answers were wrong: it double-counted the cone base,
reversed the rod-fit conclusion, and gave the wrong cube volume. Its checklist
also remained unfinished. In [run 41919253067](https://github.com/Katsugachi/Cordial/actions/runs/41919253067),
the model again used Python, but used an incorrect cone formula and treated the
cube's body diagonal as an edge; it correctly stated that the rod fits despite
having run a reversed Python inequality. The checklist remained incomplete.
In [run 42373667799](https://github.com/Katsugachi/Cordial/actions/runs/42373667799),
Python ran but the final answer again had an incorrect cone formula, reversed
rod-fit conclusion, incorrect cube volume and an incomplete checklist. In
[run 42376009812](https://github.com/Katsugachi/Cordial/actions/runs/42376009812),
the cone and cube answers were again incorrect, the rod-fit statements
contradicted each other, and the checklist remained incomplete. The same errors
recurred in [run 42380760745](https://github.com/Katsugachi/Cordial/actions/runs/42380760745)
and [run 42383103954](https://github.com/Katsugachi/Cordial/actions/runs/42383103954).
With a different recorded seed, [run 42436304967](https://github.com/Katsugachi/Cordial/actions/runs/42436304967)
also failed: the model used an incorrect cone formula, an incorrect base-diagonal
comparison and an edge in place of the cube diagonal, with an unfinished plan.
The image-token capacity probe passed before these worksheet attempts. A
`[pdf-test] [pdf-zoom]` experiment instead supplies three overlapping generic
page regions, with a larger test context and the same seed as run 42447192434.
It is an evaluation-only variant, not a change to desktop PDF attachments;
a successful sample would not establish general visual or mathematical accuracy.
In [run 42463152563](https://github.com/Katsugachi/Cordial/actions/runs/42463152563),
the model supplied all three independently checked answers and explanations and
used Python without leaving scripts. Its checklist remained 0/3, so the full
end-to-end test failed. The earlier full-page run with the same seed gave all
three answers incorrectly. The experiment also changed context size and prompt,
so it does not isolate the effect of cropping.
The isolated runtime passed its Linux security tests,
but it is not enabled in default desktop builds. PDF diagrams require visual rendering,
not text extraction alone.
The fixture was removed from the current branch after the test, but remains in
Git history, with the user's consent. This failed test does not validate other
subjects or research tasks. A separate `research_worksheets_architecture_and_english`
live smoke test exercises web search, opening and citing a source, a generated
geometry calculation, and literary analysis. It is triggered with `[research]`
and independently checks selected answers; it is not a comprehensive held-out
benchmark. The PDF and parallel research CI jobs derive a recorded sampling
seed from the run ID, so repeated runs cover different model samples rather than
replaying the same fixed seed. A passing sample would not establish reliability. In [run 41795514600](https://github.com/Katsugachi/Cordial/actions/runs/41795514600)
the model produced no final answer for either worksheet. The English attempt
repeated web fetches until its context filled. In the later [run 41798196782](https://github.com/Katsugachi/Cordial/actions/runs/41798196782),
the architecture answer contained the checked facts and arithmetic but no
source URL or checklist; the English task stopped after repeated HTTP 403s.
Both runs failed the end-to-end research tests. In [run 41919253067](https://github.com/Katsugachi/Cordial/actions/runs/41919253067),
the expanded architecture task ended with an incomplete answer and no checklist;
the English task opened sources but returned no final answer with its checklist
unfinished. In [run 42373667799](https://github.com/Katsugachi/Cordial/actions/runs/42373667799),
the model used sources and Python for the architecture worksheet but calculated
flooring from the pack size rather than the floor area; its checklist was absent.
The English task opened sources but stopped with a decode failure after context
compaction. In [run 42376009812](https://github.com/Katsugachi/Cordial/actions/runs/42376009812),
the architecture attempt stopped after repeated fetches; English reached its
finite test budget with an incomplete checklist, misidentified personification,
and did not compare the figures of speech. In [run 42380760745](https://github.com/Katsugachi/Cordial/actions/runs/42380760745),
the English attempt recovered from screenshot-related context pressure but still
misidentified personification and treated figurative images as missing pictures;
the architecture attempt exhausted its context. In run 42383103954, reasoning
compaction allowed the English task to advance its checklist, but those English
errors persisted; the architecture flooring calculation was again wrong and its
checklist absent. In run 42436304967, the English task correctly identified
personification but did not compare the literary images or cite its source; the
architecture attempt again stopped after repeated fetches. Both checklists were
unfinished. In run 42443033762 (seed 3788328107), the revised per-source guard
let architecture continue but it exhausted its budget on other fetches before
summarizing; the expected area was 399, perimeter 80, 176 packs and $8448.
English again mistook figurative images for missing pictures, omitted its source
URL and left the checklist unfinished. The PDF run again used Python but gave
245.04 cm², said the rod would not fit, and gave 512 cm³; its checklist was
1/3. In run 42445169265 (seed 3790463610), architecture cited its source and
marked its checklist complete, but gave 2 packs/$96 instead of 155 packs/$7440
for area 351 and perimeter 80. English exhausted its budget and invented
attributions for the examples. The PDF answer correctly said the rod fits but
mangled the calculation, gave the wrong cone area and treated the cube diagonal
as an edge; its checklist was 1/3. These results remain failures even though
some tools succeeded. In run 42447192434 (seed 3792486779), architecture
exhausted its budget without a checklist or summary; the expected area was 250,
perimeter 70, 110 packs and $5280. English exhausted its budget and again treated
figurative images as missing pictures. The PDF answer again used an incorrect
cone formula, compared the rod against the base diagonal, and treated the cube's
diagonal as an edge; its checklist remained 1/3. In run 42457810071 (seed
3803104416), architecture gave the independently checked area 522, perimeter 94,
230 packs and $11040, but its intermediate waste area was wrong, it omitted a
source URL and its checklist was unfinished. English exhausted its budget. The
PDF answer said the rod fits but used the wrong cone formula, treated the cube
diagonal as an edge and left all steps unfinished. In run 42460929795 (seed
3806224140), architecture cited a source and calculated area 378 and perimeter
82, but gave 12 packs/$576 instead of 167 packs/$8016 and left its plan
unfinished. English marked its plan complete despite identifying the figures
of speech incorrectly, treating figurative images as missing pictures and not
citing a source URL. The PDF attempt correctly calculated the rod fit but gave
94.25 cm² for the cone and 512 cm³ for the cube; its checklist was 1/3. In
run 42474756716 (seed 3820051061), architecture gave the independently checked
area 297, perimeter 76, 131 packs and $6288 and completed its plan. English had
no plan, misidentified the figurative images, and gave the wrong publication
year, so the combined research evaluation failed. The same run repeated the visual
region experiment at seed 3792486779 after adding temporary draft context for
checklist reconciliation. The model still left the checklist 0/3 and changed the
cone answer from the earlier correct 75.40 cm² to 103.67 cm², while retaining
the correct rod and cube results. The draft-context change was removed after
this regression. A single comparison does not establish its cause. A later
[run 42478210068](https://github.com/Katsugachi/Cordial/actions/runs/42478210068)
tried an explicit plan-call example instead, again with the same crop seed. The
checklist stayed 0/3 and the final cone answer was 103.32 cm²; the other two
answers were correct and no scripts were left behind. This prompt experiment
was also removed. In the same run, the architecture answer and checklist passed
its independent arithmetic and citation checks, while the English task exhausted
its tool budget without a substantive answer (seed 3823504413). After removing
the plan-call example, [run 42482161466](https://github.com/Katsugachi/Cordial/actions/runs/42482161466)
again produced all three correct PDF answers at seed 3792486779 and left no
scripts, but the checklist remained 0/3, so the evaluation still failed. The
ordinary Rust, UI, desktop, sandbox and inference checks passed. At research seed
3827455811, architecture's numerical results matched the independent checker
but it had no checklist and made an invalid comparison between area and
perimeter; English exhausted its tool budget. Both real-model jobs failed.
In [run 42486608847](https://github.com/Katsugachi/Cordial/actions/runs/42486608847),
search calls returned both results and empty results after adding a
Wikipedia-only fallback, but the diagnostics did not identify which provider
answered. Both research worksheets exhausted their tool budgets without
substantive answers (seed 3831903192). The ordinary Rust, UI, desktop, sandbox
and inference jobs passed. This single run does not establish whether the
fallback caused or improved the failures. In
[run 42491867495](https://github.com/Katsugachi/Cordial/actions/runs/42491867495),
the status explicitly identified Wikipedia-only search results. The research
evaluation still failed: English exhausted its tool budget, and architecture
marked its checklist done while giving 2 packs/$96 instead of 238 packs/$11,424
(seed 3837161840). The ordinary Rust, UI, desktop, sandbox and inference checks
passed. Search availability does not make model reasoning or progress reports
reliable. The evaluation now matches whole numeric values (including grouped
currency) rather than numeric substrings and requires successful Python output
to contain the independently calculated values for all three PDF questions.
This conservative check can reject a correct calculation presented only as an
expression or boolean; it does not establish semantic correctness of an answer.
The research check also requires the relevant factual terms to appear in
successfully fetched, cited pages. Their presence alone does not prove that the
pages support the claims or that the interpretation is correct. The English
checker also requires the two literary labels near the corresponding subjects;
this can reject a correct but lengthy explanation and cannot recognize every
contradiction. Research fixtures
now derive their dimensions and English example variant from the logged run seed
(or log a generated seed when no seed is supplied), so failures can be replayed.
The alternative examples test the same concepts and do not constitute a broad
unseen-task benchmark.
In [run 42522006604](https://github.com/Katsugachi/Cordial/actions/runs/42522006604),
the ordinary jobs passed. The crop experiment again returned all three correct
PDF answers but failed with an unfinished checklist; both research worksheets
exhausted the tool budget without substantive answers (seed 3867300949). The
`[pdf-random]` marker can vary the crop experiment's seed instead of reusing
the fixed comparison seed; crops remain evaluation-only. The optional
`[pdf-segments]` diagnostic additionally asks the model to solve each region in
a separate fresh conversation. It is not an autonomous checklist or desktop
end-to-end test and cannot qualify a release. In
[run 42542730259](https://github.com/Katsugachi/Cordial/actions/runs/42542730259),
seed 3888024604, the combined-region task gave wrong cone and cube answers and
left its checklist 0/3. In separate turns the first region's cone answer was
correct but claimed a Python calculation without calling the tool; the second
had the correct rod conclusion and rounded the diagonal to 47.2 cm; the third
misread the cube and gave 512 cm³. The diagnostic failed and no scripts remained.
The ordinary jobs passed; separating images alone is not a demonstrated fix. In
[run 42804151954](https://github.com/Katsugachi/Cordial/actions/runs/42804151954),
seed 4149446299, the combined-region PDF again gave incorrect answers and an
unfinished plan. In the separate-region diagnostic, the cone and rod answers
were correct, but the cube answer was 512 cm³; all three turns executed Python.
Both research tasks exhausted their budgets. Rust, UI, desktop, sandbox and
inference jobs passed at the evaluated commit. At varied seed
3878959921, [run 42533665576](https://github.com/Katsugachi/Cordial/actions/runs/42533665576)
returned a wrong cone area and rod conclusion, the correct cube volume, and a
2/3 checklist whose completed steps included the wrong answers. The Python
outputs were 245.04, 25 and 98.534. The PDF evaluation failed; a correct answer
in earlier fixed-seed crop runs does not establish reliability. Both research
worksheets in that run also exhausted their tool budgets without substantive
answers; the ordinary jobs passed.
[Run 42527009463](https://github.com/Katsugachi/Cordial/actions/runs/42527009463)
tested the ordinary full-page PDF path and a three-generation research pause.
The PDF gave 94.25 cm², rejected the rod despite calculating 47.17 cm, and
used 512 cm³; its checklist was 0/3. Python output contained those same values,
so the calculator executed the model's incorrect assumptions rather than fixing
them. Architecture's arithmetic was correct but its checklist was unfinished;
English exhausted the tool budget (seed 3872303808). Both real-model jobs failed,
while ordinary jobs passed. The longer research-pause experiment was removed:
it did not demonstrate improvement and would constrain legitimate research.
Different seeds prevent a controlled causal comparison. In the next full-page
[run 42530172915](https://github.com/Katsugachi/Cordial/actions/runs/42530172915),
the ordinary jobs passed but both real-model jobs failed. At seed 3875467260,
the PDF gave the correct cone and rod answers but treated the cube's diagonal as
a side and returned 512 cm³; its checklist was 0/3. Successful Python output
contained 75.40, 47.17 and 512, not the correct cube volume. Both research
worksheets exhausted their tool budgets without substantive answers. In
[run 42539386735](https://github.com/Katsugachi/Cordial/actions/runs/42539386735),
the full-page PDF gave 245.04 cm², an incorrect rod conclusion and 512 cm³,
with checklist 1/3. The new English example exhausted its tool budget; the
architecture arithmetic and checklist were correct but the answer omitted a
source URL (model seed 3884681080). Ordinary jobs passed.

### Task and Horizon worksheet evaluation with production effort settings

[Run 43016355747](https://github.com/Katsugachi/Cordial/actions/runs/43016355747)
used seed 66682797, the full-page supplied PDF and independently keyed research
worksheets. Both Task and Horizon evaluations failed. Task repeatedly ran Python
on the PDF and filled the context before answering. Horizon used the incorrect
cone formula (245.04 cm²) before filling the context. No scripts remained.
For architecture, the correct answers were area 300 m², perimeter 74 m,
132 packs and $6,336. Task answered 1 pack and $52.80; Horizon answered 120 packs
and $5,760. Horizon's loop guard stopped it, and its English attempt also stalled;
Task's English attempt filled the context. All ordinary CI jobs passed.

This run raised the output reserve to the production Task/Horizon setting of
4,096 tokens but retained the old 8,192-token smoke-test context, leaving only
4,096 tokens for prompt and history. This artificially constrains long tasks,
so subsequent worksheet diagnostics use a 32,768-token context (still below
the default 131,072). This change does not invalidate the observed wrong
answers or establish that a larger context improves them. Horizon is a
separate diagnostic, not release evidence; its external wall-clock timeout
protects CI from an unbounded run.

[Run 43024113716](https://github.com/Katsugachi/Cordial/actions/runs/43024113716)
used a 32,768-token context (seed 74440766). Its PDF tests did not reach the
model task: the image-capacity probe still assumed an 8K context and failed
its own assertion. This is an evaluation harness error, not a model result;
the probe has been corrected to use the actual configured context. For the
research worksheet, Task gave correct architecture arithmetic (209 m², 60 m,
92 packs, $4,416), but citations included sources it had not fetched and the
English task exhausted the round budget after repetitive searches. Horizon's
research diagnostic timed out after 60 minutes before producing a final result.
Both research steps hit their CI time limits. The ordinary CI jobs passed.

[Run 43049533432](https://github.com/Katsugachi/Cordial/actions/runs/43049533432)
fixed the probe and evaluated the full-page worksheet at 32,768 context tokens,
seed 99860482. Both Task and Horizon failed. Task answered cone 245.04 cm²,
rod fits with diagonal 47.17 cm, and cube 512 cm³, with only one of three
plan steps marked done. Horizon answered cone 216.46 cm², box diagonal
50.25 cm, and cube 512 cm³, with no steps done. The correct values are
approximately 75.40 cm², 47.17 cm (rod fits), and 98.534 cm³. Horizon's Python
results (245.04, 64.16 and 512) also contradicted parts of its final answer.
Both used Python without leaving script files. Rust, UI, desktop, inference and
sandbox jobs passed; research was not run on this commit. These are model
failures, not failures of the earlier context probe.

A source-provenance safeguard asks Task/Horizon to review unconfirmed links
once and warns on final answers containing links not confirmed opened in the
current turn. Only successful page-fetch results and browser page snapshots
count; search snippets, failed requests and prior turns do not. This is a
conservative heuristic, not source verification: URLs can refer to redirects,
may be incorrectly parsed, and opened pages may not support the associated claim.

[Run 43308693038](https://github.com/Katsugachi/Cordial/actions/runs/43308693038)
tested source-provenance safeguards and the full-page PDF with 32,768-token
context, seed 359020088. Ordinary jobs passed, but both live evaluations failed.
The PDF answer gave 245.04 cm² for the cone, said a 46 cm rod did not fit
despite a 47.17 cm diagonal, and treated the cube's 8 cm diagonal as a side,
giving 512 cm³; the plan was 1/3. Python was called and no scripts remained.
For architecture the reported area 336 m², perimeter 74 m, 148 packs and
$7,104 matched the key, but the answer jumped from 134.4 packs to 148 without
explaining the waste factor and compared square area and linear perimeter as
if their numerical magnitudes were meaningful; no checklist was created. The
English task searched repeatedly and exhausted its budget with no substantive
answer. This run does not demonstrate that the citation review improves
research correctness: different seeds and task outcomes prevent a controlled
comparison, and an opened page is not proof that it supports a claim.
