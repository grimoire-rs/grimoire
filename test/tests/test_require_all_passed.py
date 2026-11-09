# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A gate over a few named modules refuses any case that did not pass.

The old-grim interop modules skip when ``GRIM_OLD_BIN`` is unset, and pytest
exits 0 over a run that only skipped, so a gate reading that exit code alone
passes while nothing ran. Fixture reports stand in for pytest's JUnit.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from tools import require_all_passed as gate

TEST_DIR = Path(__file__).resolve().parent.parent

PASSED = '<testcase classname="{module}" name="test_{case}" time="0.1"/>'
SKIPPED = '<testcase classname="tests.test_a" name="test_s"><skipped type="pytest.skip" message="s"/></testcase>'
XFAILED = '<testcase classname="tests.test_a" name="test_x"><skipped type="pytest.xfail" message="x"/></testcase>'
FAILED = (
    '<testcase classname="tests.test_a" name="test_f"><failure message="f"/></testcase>'
)
ERRORED = (
    '<testcase classname="tests.test_a" name="test_e"><error message="e"/></testcase>'
)
SELECTIONS = ["tests/test_a.py", "tests/test_b.py::test_one"]


def _report(tmp_path: Path, extra: str = "") -> Path:
    cases = PASSED.format(module="tests.test_a", case="a") + PASSED.format(
        module="tests.test_b", case="one"
    )
    report = tmp_path / "junit.xml"
    report.write_text(
        f'<?xml version="1.0"?><testsuites><testsuite name="pytest">{cases}{extra}</testsuite></testsuites>',
        encoding="utf-8",
    )
    return report


def _run(report: Path, selections: list[str] = SELECTIONS) -> int:
    return gate.main(["--junit", str(report), *selections])


def test_a_run_where_every_case_passed_exits_zero(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert _run(_report(tmp_path)) == 0

    assert "2 cases, 2 passed, 0 skipped" in capsys.readouterr().out


@pytest.mark.parametrize(
    ("case", "verdict"),
    [
        (SKIPPED, "test_s skipped"),
        (XFAILED, "test_x xfailed"),
        (FAILED, "test_f failed"),
        (ERRORED, "test_e failed"),
    ],
)
def test_one_case_that_did_not_pass_refuses_the_run_by_name(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], case: str, verdict: str
) -> None:
    assert _run(_report(tmp_path, case)) == 1

    assert f"tests.test_a.{verdict}" in capsys.readouterr().err


def test_a_named_module_with_no_case_in_the_report_refuses(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert _run(_report(tmp_path), [*SELECTIONS, "tests/test_c.py"]) == 1

    assert "tests/test_c.py ran no case" in capsys.readouterr().err


def test_a_case_in_a_test_class_counts_for_its_module(tmp_path: Path) -> None:
    in_class = PASSED.format(module="tests.test_c.TestGroup", case="k")

    assert _run(_report(tmp_path, in_class), [*SELECTIONS, "tests/test_c.py"]) == 0


def test_a_module_name_that_only_shares_a_prefix_is_not_its_case(
    tmp_path: Path,
) -> None:
    assert _run(_report(tmp_path), ["tests/test_a.py", "tests/test_.py"]) == 1


def test_an_unreadable_report_refuses(tmp_path: Path) -> None:
    torn = tmp_path / "junit.xml"
    torn.write_text("<testsuites><testsuite>", encoding="utf-8")

    assert _run(torn) == 1
    assert _run(tmp_path / "missing.xml") == 1


def _old_grim_selections(taskfile: str) -> list[str]:
    """The selections the `old-grim` task's MODULES var names, in order."""
    # No DOTALL: each `.` stays on one line, so the match ends with the task and its list.
    task = re.search(
        r"^  old-grim:\n(?P<body>(?:(?:    .*)?\n)*)", taskfile, re.MULTILINE
    )
    assert task, "test/taskfile.yml: old-grim task not found"
    block = re.search(
        r"^      MODULES: >-\n(?P<lines>(?:        \S.*\n)+)",
        task["body"],
        re.MULTILINE,
    )
    assert block, "test/taskfile.yml: old-grim MODULES list not found"
    return block["lines"].split()


def _old_grim_modules_on_disk() -> set[str]:
    """Every test module that runs grim 0.15.1, by name or by importing its fixture."""
    return {
        f"tests/{path.name}"
        for path in (TEST_DIR / "tests").glob("test_*.py")
        if path.name.startswith("test_old_grim_")
        or re.search(
            r"^(from|import) src\.old_grim\b", path.read_text("utf-8"), re.MULTILINE
        )
    }


def test_every_module_that_runs_grim_0_15_1_is_in_the_old_grim_task() -> None:
    """A module left out of `test:old-grim` skips everywhere else and never runs."""
    on_disk = _old_grim_modules_on_disk()
    assert on_disk, "found no old-grim module: the detection is broken"
    selections = _old_grim_selections(
        (TEST_DIR / "taskfile.yml").read_text(encoding="utf-8")
    )

    missing = sorted(
        on_disk - {selection.split("::", 1)[0] for selection in selections}
    )
    twice = sorted(
        {selection for selection in selections if selections.count(selection) > 1}
    )

    assert not missing, (
        f"add to the old-grim MODULES list in test/taskfile.yml: {missing}"
    )
    assert not twice, f"the old-grim MODULES list names these more than once: {twice}"


def test_the_old_grim_list_ends_with_its_own_block() -> None:
    """A path a later task names must not count as an old-grim selection."""
    taskfile = (
        "tasks:\n"
        "  old-grim:\n"
        "    vars:\n"
        "      MODULES: >-\n"
        "        tests/test_old_grim_a.py\n"
        "      REPORT: results/old-grim/junit.xml\n"
        "    cmds:\n"
        "      - pytest {{.MODULES}}\n"
        "  other:\n"
        "    vars:\n"
        "      MODULES: >-\n"
        "        tests/test_old_grim_b.py\n"
    )

    assert _old_grim_selections(taskfile) == ["tests/test_old_grim_a.py"]
