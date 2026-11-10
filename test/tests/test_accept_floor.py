# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""C-040, S-031: the floor refuses a suite that did not run whole, and merges it.

Fixture shards stand in for pytest's own JUnit: a skip carries ``type="pytest.skip"``
and an xfail ``type="pytest.xfail"``, the two spellings pytest writes.
"""

from __future__ import annotations

import shutil
import subprocess
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

import pytest
from tools import accept_floor as floor

pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="git not on PATH")

LABEL = "ubuntu-latest"
PASSED = '<testcase classname="tests.test_{shard}" name="test_{case}" time="0.5"/>'
SKIPPED = '<testcase classname="tests.test_s" name="test_{case}"><skipped type="pytest.skip" message="s"/></testcase>'
XFAILED = '<testcase classname="tests.test_x" name="test_{case}"><skipped type="pytest.xfail" message="x"/></testcase>'


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    root.mkdir()
    subprocess.run(["git", "init", "--quiet", str(root)], check=True)
    return root


@pytest.fixture
def results(tmp_path: Path) -> Path:
    """Four shards of two passing cases each: eight unique cases in all."""
    directory = tmp_path / "results"
    directory.mkdir()
    for index in range(4):
        _shard(directory, index)
    return directory


def _shard(
    results: Path, index: int, extra: str = "", label: str = LABEL, count: int = 4
) -> Path:
    cases = "".join(PASSED.format(shard=index, case=case) for case in "ab")
    report = results / f"junit-{label}-{index}-of-{count}.xml"
    report.write_text(
        f'<?xml version="1.0"?><testsuites><testsuite name="pytest">{cases}{extra}</testsuite></testsuites>',
        encoding="utf-8",
        newline="\n",
    )
    return report


def _run(
    repo: Path,
    results: Path,
    *,
    suite: int = 8,
    skips: int = 1,
    xfails: int = 1,
    count: int = 4,
) -> int:
    limits = results.parent / "limits"
    limits.mkdir(exist_ok=True)
    for name, value in (
        ("SUITE_FLOOR", suite),
        ("SKIP_CEILING", skips),
        ("XFAIL_CEILING", xfails),
    ):
        (limits / name).write_text(f"{value}\n", encoding="utf-8", newline="\n")
    return floor.main(
        [
            "--results",
            str(results),
            "--label",
            LABEL,
            "--shard-count",
            str(count),
            "--limits-dir",
            str(limits),
            "--root",
            str(repo),
        ]
    )


def _merged(results: Path) -> list[ET.Element]:
    return list(ET.parse(results / "junit.xml").getroot().iter("testcase"))


def test_a_whole_green_suite_exits_zero_and_reports_its_counts(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert _run(repo, results) == 0

    assert "8 cases" in capsys.readouterr().out


def test_every_input_test_lands_in_the_merged_file_exactly_once(
    repo: Path, results: Path
) -> None:
    assert _run(repo, results) == 0

    merged = Counter(
        (case.get("classname"), case.get("name")) for case in _merged(results)
    )

    assert merged == Counter(
        (f"tests.test_{index}", f"test_{case}") for index in range(4) for case in "ab"
    )


def test_the_merged_suite_totals_are_recounted_from_its_cases(
    repo: Path, results: Path
) -> None:
    _shard(results, 3, SKIPPED.format(case="s") + XFAILED.format(case="x"))
    _shard(
        results,
        2,
        '<testcase classname="tests.test_f" name="test_f"><failure message="f"/></testcase>',
    )
    _shard(
        results,
        1,
        '<testcase classname="tests.test_e" name="test_e"><error message="e"/></testcase>',
    )

    _run(repo, results)

    suite = ET.parse(results / "junit.xml").getroot().find("testsuite")
    assert suite is not None
    assert {
        key: suite.get(key)
        for key in ("tests", "failures", "errors", "skipped", "time")
    } == {
        "tests": "12",
        "failures": "1",
        "errors": "1",
        "skipped": "2",
        "time": "4.000",
    }


@pytest.mark.parametrize("tag", ["failure", "error"])
def test_one_failed_case_refuses_the_suite(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str], tag: str
) -> None:
    _shard(
        results,
        3,
        f'<testcase classname="tests.test_3" name="test_bad"><{tag} message="m"/></testcase>',
    )

    assert _run(repo, results) == 1

    assert "1 failed or errored" in capsys.readouterr().err


def test_a_refused_suite_still_merges_so_the_failure_can_be_published(
    repo: Path, results: Path
) -> None:
    _shard(
        results,
        3,
        '<testcase classname="tests.test_3" name="test_bad"><failure message="m"/></testcase>',
    )

    assert _run(repo, results) == 1

    assert "test_bad" in {case.get("name") for case in _merged(results)}


def test_a_missing_shard_refuses_by_name_even_when_the_floor_is_met(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    (results / f"junit-{LABEL}-2-of-4.xml").unlink()

    assert _run(repo, results, suite=6) == 1

    assert f"junit-{LABEL}-2-of-4.xml" in capsys.readouterr().err


def test_a_missing_shard_still_merges_the_shards_that_ran(
    repo: Path, results: Path
) -> None:
    (results / f"junit-{LABEL}-2-of-4.xml").unlink()

    assert _run(repo, results, suite=6) == 1

    assert len(_merged(results)) == 6


def test_a_torn_shard_report_refuses_by_name(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    (results / f"junit-{LABEL}-1-of-4.xml").write_text(
        "<testsuites><testsuite>", encoding="utf-8", newline="\n"
    )

    assert _run(repo, results, suite=6) == 1

    assert f"junit-{LABEL}-1-of-4.xml" in capsys.readouterr().err


def test_a_report_outside_the_expected_set_is_not_read(
    repo: Path, results: Path
) -> None:
    _shard(results, 0, label="macos-latest")
    _shard(results, 0, count=8)

    assert _run(repo, results) == 0

    assert len(_merged(results)) == 8


def test_an_earlier_merge_is_not_read_as_a_shard(repo: Path, results: Path) -> None:
    assert _run(repo, results) == 0

    assert _run(repo, results) == 0

    assert len(_merged(results)) == 8


def test_a_duplicate_across_shards_refuses_by_name(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _shard(results, 3, PASSED.format(shard=0, case="a"))

    assert _run(repo, results) == 1

    assert "tests.test_0.test_a" in capsys.readouterr().err


def test_the_same_name_in_another_class_is_no_duplicate(
    repo: Path, results: Path
) -> None:
    _shard(results, 3, PASSED.format(shard="0.TestOther", case="a"))

    assert _run(repo, results) == 0


def test_one_case_under_the_floor_refuses(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert _run(repo, results, suite=9) == 1

    assert "8 cases ran, SUITE_FLOOR is 9" in capsys.readouterr().err


def test_a_suite_exactly_at_its_limits_passes(repo: Path, results: Path) -> None:
    _shard(results, 3, SKIPPED.format(case="s") + XFAILED.format(case="x"))

    assert _run(repo, results, suite=10, skips=1, xfails=1) == 0


def test_one_skip_over_the_ceiling_refuses(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _shard(results, 3, SKIPPED.format(case="s1") + SKIPPED.format(case="s2"))

    assert _run(repo, results, skips=1) == 1

    assert "2 skipped, SKIP_CEILING is 1" in capsys.readouterr().err


def test_one_xfail_over_the_ceiling_refuses_and_is_no_skip(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    _shard(results, 3, XFAILED.format(case="x1") + XFAILED.format(case="x2"))

    assert _run(repo, results, skips=0, xfails=1) == 1

    err = capsys.readouterr().err
    assert "2 xfailed, XFAIL_CEILING is 1" in err
    assert "SKIP_CEILING" not in err


@pytest.mark.parametrize("top", ["src", "test", "crates"])
def test_a_parked_test_marker_refuses_tracked_or_not(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str], top: str
) -> None:
    parked = repo / top / "parked.txt"
    parked.parent.mkdir()
    parked.write_text(f"{floor.PARK_MARKER}: P1\n", encoding="utf-8", newline="\n")

    assert _run(repo, results) == 1

    assert f"{top}/parked.txt" in capsys.readouterr().err


def test_a_marker_outside_the_scanned_trees_is_not_a_park(
    repo: Path, results: Path
) -> None:
    (repo / "plan.md").write_text(floor.PARK_MARKER, encoding="utf-8", newline="\n")

    assert _run(repo, results) == 0


def test_a_marker_scan_that_could_not_run_refuses(
    tmp_path: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    not_a_repository = tmp_path / "plain"
    not_a_repository.mkdir()

    assert _run(not_a_repository, results) == 1

    assert "marker scan" in capsys.readouterr().err


def test_the_live_tree_carries_no_parked_test() -> None:
    assert floor.parked_problems(floor.REPO_ROOT) == []


def test_a_missing_limit_file_refuses_and_still_prints_the_counts(
    repo: Path, results: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    empty = results.parent / "no-limits"
    empty.mkdir()

    code = floor.main(
        [
            "--results",
            str(results),
            "--label",
            LABEL,
            "--shard-count",
            "4",
            "--limits-dir",
            str(empty),
            "--root",
            str(repo),
        ]
    )

    assert code == 1
    captured = capsys.readouterr()
    assert "8 cases" in captured.out
    assert "SUITE_FLOOR" in captured.err


def test_the_committed_limits_parse_and_the_floor_is_the_suite_scale() -> None:
    limits = {
        name: int((floor.TEST_DIR / name).read_text(encoding="utf-8"))
        for name in floor.LIMIT_FILES
    }

    assert limits["SUITE_FLOOR"] > 2000
    assert limits["XFAIL_CEILING"] == 0


def test_no_expected_shard_at_all_is_a_usage_error(repo: Path, results: Path) -> None:
    with pytest.raises(SystemExit) as refused:
        _run(repo, results, count=0)

    assert refused.value.code == 2
