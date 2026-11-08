#!/usr/bin/env python3
import copy
import unittest

from preview_11_gate import ORDINARY, PDF, RESEARCH, passes, release_python_configured

SHA = "a" * 40


def job(name, steps=()):
    return {"name": name, "status": "completed", "conclusion": "success",
            "steps": [{"name": name, "conclusion": result} for name, result in steps]}


def fixture():
    run = {"head_sha": SHA, "status": "completed", "conclusion": "success"}
    jobs = [job(name) for name in ORDINARY]
    jobs.append(job(PDF, [("Prepare supplied worksheet with the desktop PDF renderer", "success"),
                          ("Prepare overlapping page regions for the visual experiment", "skipped"),
                          ("Evaluate supplied worksheet with the real model", "success")]))
    jobs.append(job(RESEARCH, [("Evaluate research worksheets", "success")]))
    return run, jobs


class PreviewGateTests(unittest.TestCase):
    def test_python_release_configuration_is_required(self):
        configured = """jobs:
  build:
    steps:
      - name: Test bounded Python sandbox on release target
        env:
          CORDIAL_TEST_PYTHON_WASM: /runtime/python.wasm
        run: cargo test -p cordial-core --features sandbox-python python_sandbox::tests
      - name: Build
        run: tauri build --features experimental-python --config tauri.python.conf.json
"""
        self.assertTrue(release_python_configured(configured))
        self.assertFalse(release_python_configured(configured.replace("--features experimental-python", "")))
        self.assertFalse(release_python_configured(configured.replace("--config tauri.python.conf.json", "")))
        self.assertFalse(release_python_configured(configured.replace("- name: Test bounded Python sandbox on release target", "# - name: Test bounded Python sandbox on release target")))
        self.assertFalse(release_python_configured(configured.replace("  env:", "  if: false\n  env:")))
        self.assertFalse(release_python_configured(configured.replace("python_sandbox::tests", "permissions::tests")))
        self.assertFalse(release_python_configured(configured.replace("CORDIAL_TEST_PYTHON_WASM", "OTHER_PATH")))

    def test_required_checks_pass(self):
        run, jobs = fixture()
        self.assertTrue(passes(run, jobs, SHA))

    def test_wrong_commit_and_failed_or_unfinished_run_fail(self):
        run, jobs = fixture()
        self.assertFalse(passes(run, jobs, "b" * 40))
        for key, value in (("status", "in_progress"), ("conclusion", "failure")):
            changed = {**run, key: value}
            self.assertFalse(passes(changed, jobs, SHA))

    def test_skipped_failed_missing_or_duplicate_jobs_fail(self):
        run, jobs = fixture()
        for index in range(len(jobs)):
            for conclusion in ("skipped", "failure", None):
                changed = copy.deepcopy(jobs)
                changed[index]["conclusion"] = conclusion
                self.assertFalse(passes(run, changed, SHA))
            self.assertFalse(passes(run, jobs[:index] + jobs[index + 1:], SHA))
        self.assertFalse(passes(run, jobs + [jobs[-1]], SHA))

    def test_crop_experiment_and_skipped_evaluations_fail(self):
        run, jobs = fixture()
        for job_index, step_index, result in ((-2, 0, "skipped"), (-2, 1, "success"),
                                               (-2, 2, "skipped"), (-1, 0, "failure")):
            changed = copy.deepcopy(jobs)
            changed[job_index]["steps"][step_index]["conclusion"] = result
            self.assertFalse(passes(run, changed, SHA))
        changed = copy.deepcopy(jobs)
        changed[-2]["steps"].pop()
        self.assertFalse(passes(run, changed, SHA))


if __name__ == "__main__":
    unittest.main()
