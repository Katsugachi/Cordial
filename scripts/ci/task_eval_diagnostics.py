#!/usr/bin/env python3
"""Expose generated task failures when Actions log downloads are unavailable."""
from pathlib import Path
import sys

path = Path(sys.argv[1])
if not path.exists():
    print("::warning::Generated task evaluation did not produce a log")
    raise SystemExit(0)
lines = path.read_text(errors="replace").splitlines()
# GitHub only accepts ten annotations per step. Prioritise the final result
# and failure, not a long stream of intermediate plan updates.
final = [line for line in lines if line.startswith("EVAL ") and not line.startswith(("EVAL progress", "EVAL python"))]
python = [line for line in lines if line.startswith("EVAL python")]
failures = [line for line in lines if "panicked at" in line or "test result:" in line or "error: test failed" in line or "SIGKILL" in line]
progress = [line for line in lines if line.startswith("EVAL progress")]
selected = final[-2:] + python[-2:] + failures[-3:]
if progress:
    selected.append("Progress: " + " | ".join(progress[-5:]))
selected += lines[-3:]
for line in selected[:9]:
    # GitHub workflow commands must escape control characters and percent signs.
    line = line[:3000].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::notice title=Generated task evaluation::{line}")
