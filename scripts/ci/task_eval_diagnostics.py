#!/usr/bin/env python3
"""Expose generated task failures when Actions log downloads are unavailable."""
from pathlib import Path
import sys

path = Path(sys.argv[1])
if not path.exists():
    print("::warning::Generated task evaluation did not produce a log")
    raise SystemExit(0)
lines = path.read_text(errors="replace").splitlines()
selected = [line for line in lines if line.startswith("EVAL ")]
selected += lines[-25:]
for line in selected:
    # GitHub workflow commands must escape control characters and percent signs.
    line = line[:3000].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    print(f"::notice title=Generated task evaluation::{line}")
