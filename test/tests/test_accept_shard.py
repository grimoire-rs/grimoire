# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""C-039, S-031: the shard selector partitions the acceptance modules."""

from __future__ import annotations

from collections import Counter
from pathlib import Path

import pytest
from tools import accept_shard as shard

TESTS_DIR = Path(__file__).resolve().parent


def _tests_dir(tmp_path: Path, stems: str) -> Path:
    tests = tmp_path / "tests"
    tests.mkdir()
    for stem in stems.split():
        (tests / f"{stem}.py").write_text("", encoding="utf-8", newline="\n")
    return tests


def _main(tests: Path, count: str, index: str) -> int:
    return shard.main(
        ["--tests-dir", str(tests), "--shard-count", count, "--shard-index", index]
    )


def test_every_live_module_lands_in_exactly_one_shard() -> None:
    live = sorted(path.stem for path in TESTS_DIR.glob("test_*.py"))
    assert len(live) > 100, "the glob stopped matching the suite"

    selected = Counter(
        stem for index in range(4) for stem in shard.select(shard.modules(), 4, index)
    )

    assert selected == Counter(live)
    assert set(selected.values()) == {1}


def test_this_module_is_itself_selected() -> None:
    assert Path(__file__).stem in shard.modules()


def test_shard_i_of_n_takes_every_nth_stem_from_i_of_the_sorted_list(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # Written out of order, so a selector that skips the sort lands elsewhere.
    tests = _tests_dir(tmp_path, "test_e test_b test_d test_a test_f test_c")

    assert _main(tests, "4", "1") == 0

    assert capsys.readouterr().out.split() == ["tests/test_b.py", "tests/test_f.py"]


def test_only_test_modules_are_selected(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    tests = _tests_dir(tmp_path, "conftest helpers test_a")

    assert _main(tests, "1", "0") == 0

    assert capsys.readouterr().out.split() == ["tests/test_a.py"]


@pytest.mark.parametrize(
    ("count", "index"),
    [("0", "0"), ("-1", "0"), ("4", "-1"), ("4", "4"), ("x", "0"), ("4", "")],
    ids=[
        "no-shards",
        "negative-count",
        "negative-index",
        "index-past-end",
        "count-nan",
        "index-empty",
    ],
)
def test_a_shard_that_cannot_exist_is_a_usage_error(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], count: str, index: str
) -> None:
    tests = _tests_dir(tmp_path, "test_a")

    with pytest.raises(SystemExit) as refused:
        _main(tests, count, index)

    assert refused.value.code == 2
    assert capsys.readouterr().out == ""


def test_a_shard_past_the_module_count_is_red_rather_than_an_empty_run(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    tests = _tests_dir(tmp_path, "test_a")

    assert _main(tests, "2", "1") == 1

    assert "selects no module" in capsys.readouterr().err


def test_a_directory_without_modules_is_red(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert _main(_tests_dir(tmp_path, ""), "1", "0") == 1

    assert "selects no module" in capsys.readouterr().err
