"""Check the opt-in Python bundle's resource mappings and pinned guest."""
import hashlib
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
APP = ROOT / "crates" / "cordial-app"


class PythonPackagingTest(unittest.TestCase):
    def test_runtime_and_licenses_are_present_and_pinned(self):
        config = json.loads((APP / "tauri.python.conf.json").read_text())
        resources = config["bundle"]["resources"]
        by_destination = {destination: (APP / source).resolve() for source, destination in resources.items()}
        self.assertEqual(set(by_destination), {
            "python/python.wasm", "python/licenses/python-wasi-apache-2.0.txt",
            "python/licenses/python-3.12.txt",
            "python/licenses/wasmtime-apache-with-llvm-exception.txt",
        })
        for path in by_destination.values():
            self.assertTrue(path.is_file(), str(path))
        digest = hashlib.sha256(by_destination["python/python.wasm"].read_bytes()).hexdigest()
        self.assertEqual(digest, "e5dc5a398b07b54ea8fdb503bf68fb583d533f10ec3f930963e02b9505f7a763")


if __name__ == "__main__":
    unittest.main()
