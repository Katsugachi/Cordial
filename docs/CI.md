# Continuous integration

Cordial uses `instabox-medium` for its general-purpose Linux x64 jobs instead of
spending GitHub-hosted runner minutes. Each job receives a fresh Ubuntu 24.04
x64 machine with four vCPUs and 8 GiB of RAM. The machine is deleted after the
job; there is no permanently running repository runner.

## Enable the runner

In the Instabox dashboard, open **CI**, choose **Connect GitHub**, and grant the
Instabox GitHub App access to `Katsugachi/Cordial`. The free plan supports
`instabox-medium`. No Instabox API key, SSH credentials, or GitHub Actions secret
is needed for this integration. See the [Instabox CI documentation](https://instabox.sh/docs/ci).

If GitHub shows a job as queued, check the Instabox CI page for its machine status.
Free-plan jobs can wait for another job to finish.

## Job placement

| Workflow | Instabox jobs |
| --- | --- |
| CI | Rust checks, desktop shell, UI, inference stack, optional live-model suite |
| Format | Rust formatting and lockfile maintenance |
| Live model | Scheduled or manually dispatched behaviour suite |
| Release | UI/core validation, model smoke test, publication |

The release build matrix remains platform-specific:

- macOS arm64/x64 and Windows x64/arm64 need their native GitHub-hosted runners.
- Linux arm64 needs an ARM64 runner; `instabox-medium` is x64.
- Linux x64 installers remain built on Ubuntu 22.04 to preserve the existing
  glibc compatibility baseline. Moving that build to Ubuntu 24.04 would raise
  the minimum system library version.

Thus Instabox removes the recurring Linux CI minutes, but does not eliminate the
native release-build minutes.

## Trust boundary

Instabox machines are self-hosted runners from GitHub's perspective. CI jobs run
only for repository pushes, manually dispatched runs, and pull requests whose
head branch belongs to this repository. Pull requests from forks are skipped,
not given access to the Instabox workspace. A maintainer should review a fork's
changes before pushing a trusted test branch.

The format and live-model workflows accept only push, schedule, or dispatch
events. Release publication accepts only a version-tag push or an explicit
trusted dispatch. Workflow tokens are limited to read access except for the
jobs that maintain formatting, publish a live transcript, or publish a release.

## Resources and caches

Cargo and CMake are bounded to four parallel build jobs on the medium machine.
The existing GitHub Actions caches still hold Cargo dependencies and model
weights; a fresh VM does not mean those caches are disabled. Model files and
build products stay in runner temporary/storage paths and are not committed.
The formatting workflow checks but does not regenerate `Cargo.lock`. Cargo test
and clippy commands use `--locked`, and desktop/release bundle builds fail if
Cargo changes the lockfile. Dependency updates need an intentional, reviewed
lockfile change and fresh CI; bot formatting commits are not evaluated by CI
automatically.

The workflow placement can be checked without running a build:

```bash
python3 scripts/ci/check_workflows.py
```

This checks YAML structure, the trusted-PR guards, explicit token permissions,
and the native release matrix. It requires PyYAML (`python3 -m pip install pyyaml`
in a local virtual environment if it is not already installed).
