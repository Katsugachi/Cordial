# Preview 11 release gate

Preview 11 is conditional on the requested evaluations passing; a successful
build or a correct individual answer is not sufficient. Do not publish or tag it
while a required check fails or is skipped. The release workflow checks that the
exact commit has a successful CI run with the full-page PDF and research steps;
it rejects the crop experiment. It also rejects the current release build until
Python is configured in the bundles and a per-target sandbox test is present.
These are preliminary checks, not a replacement for the remaining checks below. Formatting-bot commits change the
commit SHA, so the exact release commit must itself have a qualifying CI run.

Before publication:

- Test the exact supplied PDF through the ordinary full-page attachment path,
  including the packaged Python calculator, answer explanations, checklist
  progression and cleanup. The experimental region-crop harness is a diagnostic,
  not the desktop attachment path.
- Pass the architecture and English research worksheets, including independent
  arithmetic checks, cited sources and completed checklists. Repeat with varied
  seeds and genuinely different unseen questions; inspect explanations and source
  relevance as well as automated results. Numeric and keyword checks can miss
  semantic errors.
- Pass the generated artifact, command-safety, permissions, UI, desktop and
  sandbox tests on the intended release commit. Verify the sandbox and bundled
  runtime on each supported platform and execute the installed packages, not just
  inspect their files. The current release configuration does not enable the
  experimental Python bundle.
- Inspect the release candidates, checksums and platform-specific launch and
  signing behavior before publication. macOS must not require paid signing.

No finite test suite proves absolute safety or correctness. Document residual
risks rather than representing any result as a guarantee.
