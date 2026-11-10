#!/usr/bin/env python3
"""Fail closed unless the exact release commit passed required live CI jobs.

This is a necessary release condition, not proof of safety or correctness.
Packaging, platform and human review requirements remain separate.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ORDINARY = (
    "Rust (clippy, tests)",
    "Desktop shell (compile check)",
    "UI (typecheck, tests, build)",
    "Inference stack (llama.cpp + model reachability)",
    "Experimental Python sandbox",
)
PDF = "Behaviour suite (real LFM3-VL-2B)"
RESEARCH = "Architecture and English research evaluation"


def successful_job(jobs, name):
    matching = [job for job in jobs if job.get("name") == name]
    if len(matching) != 1:
        return None
    job = matching[0]
    return job if job.get("status") == "completed" and job.get("conclusion") == "success" else None


def step_is(job, name, conclusion):
    matching = [step for step in job.get("steps", []) if step.get("name") == name]
    return len(matching) == 1 and matching[0].get("conclusion") == conclusion


def passes(run, jobs, sha):
    if run.get("head_sha") != sha or run.get("status") != "completed" or run.get("conclusion") != "success":
        return False
    if not all(successful_job(jobs, name) for name in ORDINARY):
        return False
    pdf = successful_job(jobs, PDF)
    research = successful_job(jobs, RESEARCH)
    return bool(pdf and research
        and step_is(pdf, "Prepare supplied worksheet with the desktop PDF renderer", "success")
        and step_is(pdf, "Prepare overlapping page regions for the visual experiment", "skipped")
        and step_is(pdf, "Evaluate supplied worksheet with the real model", "success")
        and step_is(research, "Evaluate research worksheets", "success"))


def release_python_configured(workflow):
    # A preliminary fail-closed check, not a substitute for inspecting the
    # installed bundles and running cross-platform sandbox tests.
    raw_lines = workflow.splitlines()
    start = next((i for i, line in enumerate(raw_lines) if line == "  build:"), None)
    if start is None:
        return False
    end = next((i for i in range(start + 1, len(raw_lines))
                if re.fullmatch(r"  [A-Za-z0-9_-]+:", raw_lines[i])), len(raw_lines))
    lines = [line.strip() for line in raw_lines[start:end] if not line.lstrip().startswith("#")]
    build = any("tauri build" in line and "--features experimental-python" in line
                and "--config tauri.python.conf.json" in line for line in lines)
    matches = [i for i, line in enumerate(lines)
               if line == "- name: Test bounded Python sandbox on release target"]
    if len(matches) != 1:
        return False
    following = lines[matches[0] + 1:]
    end = next((i for i, line in enumerate(following) if line.startswith(("- name:", "- uses:"))), len(following))
    step = following[:end]
    # Fail closed if a conditional or continue-on-error could skip the test.
    if any(line.startswith(("if:", "continue-on-error:")) for line in step):
        return False
    test = any("cargo test --locked -p cordial-core --features sandbox-python python_sandbox::tests" in line
               and not any(operator in line for operator in ("||", ";", "|", "#")) for line in step)
    runtime = any("CORDIAL_TEST_PYTHON_WASM:" in line and "python.wasm" in line for line in step)
    return build and test and runtime


def api(path, *args):
    command = ["gh", "api", "--method", "GET", path, *args]
    return json.loads(subprocess.run(command, check=True, text=True, capture_output=True).stdout)


def main():
    sha = os.environ["GITHUB_SHA"]
    repo = os.environ["GITHUB_REPOSITORY"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("invalid release commit or repository")
    workflow = Path(".github/workflows/release.yml").read_text()
    if not release_python_configured(workflow):
        raise SystemExit("Preview 11 release builds do not yet enable and test the packaged Python runtime on each target")
    runs = api(f"repos/{repo}/actions/workflows/ci.yml/runs", "-f", f"head_sha={sha}", "-f", "per_page=100").get("workflow_runs", [])
    for run in runs:
        if run.get("head_sha") != sha or run.get("conclusion") != "success" or run.get("status") != "completed":
            continue
        run_id = run.get("id")
        if not isinstance(run_id, int) or run_id <= 0:
            continue
        jobs = api(f"repos/{repo}/actions/runs/{run_id}/jobs", "-f", "per_page=100").get("jobs", [])
        if passes(run, jobs, sha):
            print(f"Required CI evaluation jobs passed for {sha} in run {run_id}. Manual release review is still required.")
            return
    raise SystemExit(f"No successful full-page PDF and research CI evaluation for exact release commit {sha}")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(f"Preview 11 evaluation gate failed: {error}", file=sys.stderr)
        sys.exit(1)
