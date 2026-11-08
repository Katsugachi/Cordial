#!/usr/bin/env python3
"""Check runner placement and the public-repository trust boundary.

This is a static migration check, not a substitute for running CI. PyYAML is
preinstalled on the Ubuntu CI images; locally install it in a virtualenv.
"""

from pathlib import Path
import sys

try:
    import yaml
except ImportError:
    raise SystemExit("PyYAML is required: python3 -m pip install pyyaml")

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"
MEDIUM = "instabox-medium"
TRUSTED = (
    "github.event_name != 'pull_request' || "
    "github.event.pull_request.head.repo.full_name == github.repository"
)
EXPECTED = {
    "ci.yml": {
        "rust": MEDIUM,
        "python-sandbox": MEDIUM,
        "app": MEDIUM,
        "ui": MEDIUM,
        "inference-stack": MEDIUM,
        "live-model": MEDIUM,
        "research-evaluation": MEDIUM,
    },
    "format.yml": {"rustfmt": MEDIUM},
    "live.yml": {"behaviour": MEDIUM},
    "release.yml": {
        "preview-11-evaluation": MEDIUM,
        "build": "${{ matrix.os }}",
        "validation": MEDIUM,
        "smoke": MEDIUM,
        "publish-release": MEDIUM,
    },
}
NATIVE_TARGETS = {
    "macOS arm64": "macos-15",
    "macOS x64": "macos-15-intel",
    "Windows x64": "windows-2022",
    "Windows arm64 (OpenCL)": "windows-11-arm",
    "Linux x64": "ubuntu-22.04",
    "Linux arm64": "ubuntu-24.04-arm",
}


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def events(workflow: dict) -> set[str]:
    # PyYAML implements YAML 1.1, where the unquoted key `on` is a boolean.
    trigger = workflow.get("on", workflow.get(True))
    check(isinstance(trigger, dict), "workflow triggers must be a mapping")
    return set(trigger)


def main() -> None:
    check(sorted(p.name for p in WORKFLOWS.glob("*.yml")) == sorted(EXPECTED),
          "update the runner inventory when adding or removing a workflow")
    for name, expected in EXPECTED.items():
        workflow = yaml.safe_load((WORKFLOWS / name).read_text())
        jobs = workflow["jobs"]
        check(set(jobs) == set(expected), f"{name}: unexpected job inventory")
        check(workflow.get("permissions", {}).get("contents") in {"read", "write"},
              f"{name}: explicitly bound the workflow token")
        for job_name, runner in expected.items():
            job = jobs[job_name]
            check(job["runs-on"] == runner, f"{name}/{job_name}: wrong runner")
            check(isinstance(job.get("timeout-minutes"), int),
                  f"{name}/{job_name}: missing timeout")

        if name == "ci.yml":
            check(events(workflow) == {"push", "pull_request", "workflow_dispatch"},
                  "CI: unexpected trigger with access to Instabox")
            check(workflow["permissions"]["contents"] == "read",
                  "CI: default token must be read-only")
            for job_name, job in jobs.items():
                condition = " ".join(job.get("if", "").split())
                check(TRUSTED in condition,
                      f"CI/{job_name}: fork PRs must not reach Instabox")
            check(jobs["live-model"]["permissions"]["contents"] == "write",
                  "CI/live-model: transcript publication needs its scoped write grant")
        elif name == "format.yml":
            check(events(workflow) == {"push", "workflow_dispatch"},
                  "Format: only trusted push/dispatch events are allowed")
            check(workflow["permissions"]["contents"] == "write",
                  "Format: unexpected token permission")
        elif name == "live.yml":
            check(events(workflow) == {"schedule", "workflow_dispatch"},
                  "Live model: only schedule/dispatch events are allowed")
            check(workflow["permissions"]["contents"] == "write",
                  "Live model: transcript publication needs write access")
        else:
            check(events(workflow) == {"push", "workflow_dispatch"},
                  "Release: only tag push/dispatch events are allowed")
            check(workflow["permissions"]["contents"] == "read",
                  "Release: default token must be read-only")
            check(jobs["publish-release"]["permissions"]["contents"] == "write",
                  "Release: publication needs its scoped write grant")
            gate = jobs["preview-11-evaluation"]
            check(gate["permissions"] == {"contents": "read", "actions": "read"},
                  "Release: preview 11 gate needs read-only Actions access")
            check("preview-11-evaluation" in jobs["build"]["needs"],
                  "Release: builds must depend on the preview 11 gate")
            check(any("scripts/ci/preview_11_gate.py" in step.get("run", "")
                      and "preview-11" in step.get("if", "")
                      for step in gate["steps"]),
                  "Release: preview 11 must check matching evaluations")
            targets = jobs["build"]["strategy"]["matrix"]["include"]
            check({t["label"]: t["os"] for t in targets} == NATIVE_TARGETS,
                  "Release: native architecture or Linux ABI baseline changed")
            check("inputs.publish" in jobs["publish-release"]["if"] and
                  "refs/tags/v0.1.0-preview-" in jobs["publish-release"]["if"],
                  "Release: explicit publication gate changed")
        print(f"PASS {name}: runner placement, permissions, triggers")
    print(f"All {len(EXPECTED)} workflows passed.")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, KeyError, TypeError, yaml.YAMLError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
