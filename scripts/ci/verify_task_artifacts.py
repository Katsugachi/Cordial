#!/usr/bin/env python3
"""Independent verifier for the generated live-task artifacts.

Uses Python's CSV-independent text handling and standard-library ZIP/XML
reader, not Cordial's DOCX reader. The fixture is deliberately small; passing
it is not evidence that arbitrary long or research tasks are reliable.
"""
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET
import zipfile

WORD = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def has_number(text: str, value: int) -> bool:
    return re.search(rf"(?<![\d.]){value}(?!\d)(?!\.\d)", text) is not None


def verify(root: Path, a: int, b: int, c: int) -> list[str]:
    errors: list[str] = []
    try:
        lines = (root / "totals.txt").read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as exc:
        lines = []
        errors.append(f"cannot read report: {exc}")
    for label, expected in [("A", a * 2), ("B", b * 3), ("C", c * 4)]:
        if not any(re.search(rf"\b(?:item\s+)?{label}\b", line, re.I) and has_number(line, expected) for line in lines):
            errors.append(f"report does not associate item {label} with {expected}")
    total = a * 2 + b * 3 + c * 4
    if not any(re.search(r"\b(?:grand\s+)?total\b", line, re.I) and has_number(line, total) for line in lines):
        errors.append(f"report does not associate total with {total}")

    valid = False
    for path in root.glob("*.docx"):
        if path.name == "worksheet.docx":
            continue
        try:
            with zipfile.ZipFile(path) as archive:
                info = archive.getinfo("word/document.xml")
                if info.file_size > 10_000_000:
                    continue
                document = ET.fromstring(archive.read(info))
            text = "\n".join("".join(node.itertext()) for node in document.iter(WORD + "p"))
            # Require each answer in its own numbered question section; simply
            # finding both numbers elsewhere in a document is not sufficient.
            sections = re.split(r"\bquestion\s+([12])\b", text, flags=re.I)
            by_question = {sections[i]: sections[i + 1] for i in range(1, len(sections) - 1, 2)}
            if has_number(by_question.get("1", ""), a + b) and has_number(by_question.get("2", ""), c - a):
                valid = True
                break
        except (OSError, KeyError, zipfile.BadZipFile, ET.ParseError):
            continue
    if not valid:
        errors.append("no readable output DOCX contains both expected answers")
    return errors


def main() -> int:
    if len(sys.argv) != 5:
        print("usage: verify_task_artifacts.py WORKSPACE A B C", file=sys.stderr)
        return 2
    errors = verify(Path(sys.argv[1]), *(int(x) for x in sys.argv[2:]))
    for error in errors:
        print("EVAL independent: " + error)
    if errors:
        return 1
    print("EVAL independent: CSV associations and DOCX XML answers passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
