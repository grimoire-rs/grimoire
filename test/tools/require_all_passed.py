# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Refuse a JUnit report unless every case passed and every named module ran.

    python3 tools/require_all_passed.py --junit results/old-grim/junit.xml \\
        tests/test_a.py tests/test_b.py::test_one

A skip counts as a refusal: a gate whose input is missing skips, and that skip
must not read as a pass. Exits 1 on a failed, errored, skipped or xfailed case,
on a named module with no case in the report, or on an unreadable report.
"""

from __future__ import annotations

import argparse
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from collections.abc import Sequence
from pathlib import Path

TEST_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(TEST_DIR))
from tools.accept_floor import case_id, outcome


def module_of(selection: str) -> str:
    """The JUnit classname prefix of a pytest path or node id."""
    path = selection.split("::", 1)[0].removesuffix(".py")
    return path.replace("\\", "/").strip("/").replace("/", ".")


def problems(cases: Sequence[ET.Element], selections: Sequence[str]) -> list[str]:
    found = [
        f"{case_id(case)} {result}"
        for case in cases
        if (result := outcome(case)) != "passed"
    ]
    for selection in selections:
        module = module_of(selection)
        if not any(
            (name := case.get("classname", "")) == module
            or name.startswith(f"{module}.")
            for case in cases
        ):
            found.append(f"{selection} ran no case")
    return found


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--junit", type=Path, required=True, help="pytest's report")
    parser.add_argument(
        "selections", nargs="+", help="the paths or node ids pytest was given"
    )
    args = parser.parse_args(argv)
    try:
        cases = list(ET.parse(args.junit).getroot().iter("testcase"))
    except (OSError, ET.ParseError) as err:
        print(
            f"require all passed: unreadable report {args.junit}: {err}",
            file=sys.stderr,
        )
        return 1
    tally = Counter(outcome(case) for case in cases)
    print(
        f"require all passed: {len(cases)} cases, {tally['passed']} passed, "
        f"{tally['skipped']} skipped, {tally['xfailed']} xfailed, "
        f"{tally['failed']} failed or errored"
    )
    found = problems(cases, args.selections)
    for problem in found:
        print(f"require all passed: {problem}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
