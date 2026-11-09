# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Judge the whole acceptance suite over its shard reports and merge them.

    python3 tools/accept_floor.py --label ubuntu-latest --shard-count 4

Reads ``results/junit-<label>-<i>-of-<n>.xml`` for every ``i`` and writes the merged
``results/junit.xml``, even on a refusal, so a refused run still publishes what it read.
Exits 1 on a failed case, a missing shard, a duplicate ``classname.name``, fewer cases
than ``test/SUITE_FLOOR``, more skips or xfails than ``test/SKIP_CEILING`` or
``test/XFAIL_CEILING``, or a parked test marker left under ``src``, ``test`` or ``crates``.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from collections.abc import Sequence
from pathlib import Path

TEST_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = TEST_DIR.parent
MERGED_NAME = "junit.xml"
LIMIT_FILES = ("SUITE_FLOOR", "SKIP_CEILING", "XFAIL_CEILING")
PARK_SCOPE = ("src", "test", "crates")
# Split so the scan does not find its own source and refuse every run.
PARK_MARKER = "lock-pin" + "-park"


def shard_reports(results: Path, label: str, count: int) -> list[Path]:
    """The explicit expected shard set, so a stale or foreign report is never read."""
    return [results / f"junit-{label}-{index}-of-{count}.xml" for index in range(count)]


def outcome(case: ET.Element) -> str:
    """One of ``passed``, ``failed``, ``skipped`` or ``xfailed``."""
    if case.find("failure") is not None or case.find("error") is not None:
        return "failed"
    skip = case.find("skipped")
    if skip is None:
        return "passed"
    return "xfailed" if skip.get("type") == "pytest.xfail" else "skipped"


def case_id(case: ET.Element) -> str:
    return f"{case.get('classname', '')}.{case.get('name', '')}"


def write_merged(cases: Sequence[ET.Element], out: Path) -> None:
    suite = ET.Element(
        "testsuite",
        {
            "name": "acceptance",
            "tests": str(len(cases)),
            "failures": str(sum(case.find("failure") is not None for case in cases)),
            "errors": str(sum(case.find("error") is not None for case in cases)),
            "skipped": str(sum(case.find("skipped") is not None for case in cases)),
            "time": f"{sum(float(case.get('time', '0')) for case in cases):.3f}",
        },
    )
    suite.extend(cases)
    root = ET.Element("testsuites")
    root.append(suite)
    out.parent.mkdir(parents=True, exist_ok=True)
    ET.ElementTree(root).write(out, encoding="utf-8", xml_declaration=True)


def parked_problems(root: Path) -> list[str]:
    """One problem per file under ``PARK_SCOPE`` still carrying the park marker."""
    try:
        scan = subprocess.run(
            [
                "git",
                "-C",
                str(root),
                "grep",
                "-lIF",
                "--untracked",
                "-e",
                PARK_MARKER,
                "--",
                *PARK_SCOPE,
            ],
            capture_output=True,
            encoding="utf-8",
            check=False,
        )
    except OSError as err:
        return [f"marker scan did not run: {err}"]
    # 1 is git grep's "no match"; above it nothing was searched, and a clean result would be false.
    if scan.returncode > 1:
        return [f"marker scan failed: {scan.stderr.strip()}"]
    return [
        f"parked test marker remains in {path}" for path in scan.stdout.splitlines()
    ]


def limit_problems(tally: Counter[str], cases: int, limits_dir: Path) -> list[str]:
    try:
        floor, skip_ceiling, xfail_ceiling = (
            int((limits_dir / name).read_text(encoding="utf-8")) for name in LIMIT_FILES
        )
    except (OSError, ValueError) as err:
        return [
            f"unreadable limit file under {limits_dir} ({', '.join(LIMIT_FILES)}): {err}"
        ]
    problems = []
    if cases < floor:
        problems.append(f"{cases} cases ran, SUITE_FLOOR is {floor}")
    if tally["skipped"] > skip_ceiling:
        problems.append(f"{tally['skipped']} skipped, SKIP_CEILING is {skip_ceiling}")
    if tally["xfailed"] > xfail_ceiling:
        problems.append(f"{tally['xfailed']} xfailed, XFAIL_CEILING is {xfail_ceiling}")
    return problems


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--label", required=True, help="the SHARD_LABEL the shards ran under"
    )
    parser.add_argument("--shard-count", type=int, required=True, help="N, at least 1")
    parser.add_argument("--results", type=Path, default=TEST_DIR / "results")
    parser.add_argument("--limits-dir", type=Path, default=TEST_DIR)
    parser.add_argument(
        "--root",
        type=Path,
        default=REPO_ROOT,
        help="repository to scan for parked tests",
    )
    args = parser.parse_args(argv)
    if args.shard_count < 1:
        parser.error(f"--shard-count {args.shard_count}: need at least 1 shard")

    problems: list[str] = []
    cases: list[ET.Element] = []
    for report in shard_reports(args.results, args.label, args.shard_count):
        try:
            cases.extend(ET.parse(report).getroot().iter("testcase"))
        except FileNotFoundError:
            problems.append(f"missing shard report {report.name}")
        except (OSError, ET.ParseError) as err:
            problems.append(f"unreadable shard report {report.name}: {err}")
    write_merged(cases, args.results / MERGED_NAME)

    tally = Counter(outcome(case) for case in cases)
    print(
        f"accept floor: {len(cases)} cases, {tally['skipped']} skipped, "
        f"{tally['xfailed']} xfailed, {tally['failed']} failed or errored"
    )
    if tally["failed"]:
        problems.append(f"{tally['failed']} failed or errored case(s)")
    seen = Counter(case_id(case) for case in cases)
    problems += [f"duplicate case {name}" for name, count in seen.items() if count > 1]
    problems += limit_problems(tally, len(cases), args.limits_dir)
    problems += parked_problems(args.root)

    for problem in problems:
        print(f"accept floor: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
