"""Regression tests for bounded GitHub annotation diagnostics."""
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("task_eval_diagnostics.py")


class DiagnosticsTest(unittest.TestCase):
    def test_python_calls_do_not_hide_the_final_answer(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "test.log"
            log.write_text("EVAL rendered worksheet answer=wrong\n" +
                           "".join(f"EVAL python code={i}, result=ok\n" for i in range(8)))
            output = subprocess.check_output(["python3", str(SCRIPT), str(log)], text=True)
            self.assertIn("EVAL rendered worksheet answer=wrong", output)
            self.assertIn("EVAL python code=7", output)
            self.assertLessEqual(len(output.splitlines()), 9)

    def test_many_progress_events_do_not_hide_final_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "test.log"
            log.write_text("".join(f"EVAL progress tool step{i}\n" for i in range(20)) +
                           "EVAL rendered worksheet answer=missing\n" +
                           "thread 'case' panicked at test.rs:1\n" +
                           "test result: FAILED\n")
            output = subprocess.check_output(["python3", str(SCRIPT), str(log)], text=True)
            lines = output.splitlines()
            self.assertLessEqual(len(lines), 9)
            self.assertIn("EVAL rendered worksheet answer=missing", output)
            self.assertIn("panicked at", output)
            self.assertIn("step19", output)
            self.assertNotIn("step0 |", output)


if __name__ == "__main__":
    unittest.main()
