#!/usr/bin/env python3
"""Test workflow safety checks against mutated temporary copies."""
import contextlib
import io
from pathlib import Path
import shutil
import tempfile
import unittest

import check_workflows


class WorkflowCheckerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.original = check_workflows.WORKFLOWS
        self.addCleanup(setattr, check_workflows, "WORKFLOWS", self.original)
        self.workflows = Path(self.temp.name) / "workflows"
        shutil.copytree(self.original, self.workflows)
        check_workflows.WORKFLOWS = self.workflows

    def run_checker(self):
        with contextlib.redirect_stdout(io.StringIO()):
            check_workflows.main()

    def test_current_workflows_pass(self):
        self.run_checker()

    def test_format_cannot_regenerate_dependencies(self):
        path = self.workflows / "format.yml"
        data = path.read_text()
        path.write_text(data.replace("cargo metadata --locked", "cargo generate-lockfile\n          cargo metadata --locked", 1))
        with self.assertRaisesRegex(AssertionError, "never update dependencies implicitly"):
            self.run_checker()

    def test_cargo_tests_must_use_locked_dependencies(self):
        path = self.workflows / "ci.yml"
        data = path.read_text()
        path.write_text(data.replace("cargo test --locked", "cargo test", 1))
        with self.assertRaisesRegex(AssertionError, "Cargo tests must use the checked-in lockfile"):
            self.run_checker()


if __name__ == "__main__":
    unittest.main()
