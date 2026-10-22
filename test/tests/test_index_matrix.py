# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Every grim generation against every kind of index.

G0 is grim 0.14.3, the last release that caps an index at 500 rows; G1 is
0.15.0, which pages; G2 is the grim under test. The index kinds are legacy-only
(``all.json``), ``c/`` plus legacy and ``c/``-only. G0 and G1 read ``all.json``
and never ask for ``c/``, so a ``c/``-only index is empty to them. A leg whose
binary is not provided is skipped: ``task test:index-matrix`` resolves G0 and
G1 from ocx.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from functools import cache
from pathlib import Path
from typing import Any

import pytest

from src.helpers import make_artifact
from src.index_fixture import (
    INDEX_KINDS,
    ConformanceKind,
    IndexHandler,
    KindedIndex,
    Request,
    conformance_tree,
    filtered_index_config,
    index_config,
    package,
    serve,
    serve_kind,
    write_stats,
)
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner

TREE = Path(__file__).resolve().parents[1] / "data" / "conformance"
OLD_CAP = 500
COUNT = 600
LATE = "ghcr.io/acme/skills/pkg-550"

LEGS = ("g0", "g1", "g2")
_LEG_INPUTS = {
    "g0": ("GRIM_G0_COMMAND", "GRIM_G0_COMMAND must name a grim 0.14.3 binary"),
    "g1": ("GRIM_G1_COMMAND", "0.15.0 not published"),
}
_LEG_VERSIONS = {"g0": "grim 0.14.3", "g1": "grim 0.15.0"}
GRIM_TIMEOUT = 120


def _reads(leg: str, kind: ConformanceKind) -> bool:
    """Whether a grim of this generation can read an index of ``kind``."""
    return kind != "c-only" or leg == "g2"


def _cells(legs: tuple[str, ...] = LEGS, *, readable: bool = False):
    return [
        pytest.param(leg, kind, id=f"{leg}-{kind}")
        for leg in legs
        for kind in INDEX_KINDS
        if _reads(leg, kind) or not readable
    ]


ALL_CELLS = _cells()
READABLE_CELLS = _cells(readable=True)
PAGING_CELLS = _cells(("g1", "g2"), readable=True)


@cache
def _version(command: str) -> str:
    done = subprocess.run(
        [command, "--version"], capture_output=True, text=True, timeout=30, check=True
    )
    return done.stdout.strip()


def _binary(leg: str, grim_binary: Path) -> Path:
    if leg == "g2":
        return grim_binary
    variable, unavailable = _LEG_INPUTS[leg]
    if not (command := os.environ.get(variable)):
        pytest.skip(unavailable)
    assert _version(command) == _LEG_VERSIONS[leg], (
        f"{variable} must name {_LEG_VERSIONS[leg]}, got {_version(command)!r}"
    )
    return Path(command)


class _TimedRunner(GrimRunner):
    """A hung grim fails the test instead of the whole run."""

    def run(self, *args: str, timeout: float | None = GRIM_TIMEOUT, **kwargs: Any):
        return super().run(*args, timeout=timeout, **kwargs)


@pytest.fixture()
def runner_for(grim_binary: Path, tmp_path: Path):
    """``(leg, base) -> GrimRunner`` for a project reading the index at ``base``."""

    def _make(leg: str, base: str) -> GrimRunner:
        project, home = tmp_path / "project", tmp_path / "grim-home"
        project.mkdir(exist_ok=True)
        home.mkdir(exist_ok=True)
        index_config(project, base)
        return _TimedRunner(_binary(leg, grim_binary), home, cwd=project)

    return _make


@contextmanager
def _served(kind: ConformanceKind, work: Path) -> Iterator[KindedIndex]:
    """Plain ``all.json`` and ``stats.json`` written into the root are
    published in the shape of ``kind``."""
    root = work / "dist"
    root.mkdir()
    with serve_kind(root, kind) as index:
        yield index


def _search(runner: GrimRunner, *args: str) -> tuple[dict, str]:
    """The ``search --refresh`` envelope and everything written to stderr."""
    result = runner.run("--format", "json", "search", "--refresh", *args, check=False)
    assert result.returncode == 0, (
        f"{runner.binary} search exited {result.returncode}; stderr: {result.stderr}"
    )
    return json.loads(result.stdout), result.stderr


def _rows(envelope: dict) -> dict[str, tuple[str, int | None]]:
    repos = [item["repo"] for item in envelope["items"]]
    assert len(repos) == len(set(repos)), "a repo is listed once"
    return {
        item["repo"]: (item["kind"], (item["rating"] or {}).get("up"))
        for item in envelope["items"]
    }


def _tree_pointers() -> list[dict]:
    return json.loads((TREE / "all.json").read_text(encoding="utf-8"))


def _null_list_record() -> dict:
    """A pointer whose list fields are null, as an older or other indexer can
    emit them. The tree has none: the indexer clamps them to ``[]``."""
    return {
        **package(
            "null-lists", "skill", "ghcr.io/acme/skills/null-lists", "Null lists"
        ),
        "keywords": None,
        "tags": None,
    }


def _tree_rows() -> dict[str, tuple[str, int | None]]:
    """What a grim should list for the tree: schema 1 only, the last pointer
    of a ref winning."""
    stats = json.loads((TREE / "stats.json").read_text(encoding="utf-8"))["entries"]
    return {
        pkg["ref"]: (
            pkg["kind"],
            (stats.get(pkg["ref"], {}).get("rating") or {}).get("up"),
        )
        for pkg in _tree_pointers()
        if pkg["schema"] == 1
    }


@contextmanager
def _served_tree(
    kind: ConformanceKind, work: Path
) -> Iterator[tuple[str, list[Request]]]:
    """The checked-in tree served as published in the shape of ``kind``;
    yields the base URL and the request log."""
    root = conformance_tree(work / "dist", kind)
    handler = IndexHandler.new()
    with serve(root, handler) as base:
        yield base, handler.log


@contextmanager
def _seeded_tree(kind: ConformanceKind, work: Path) -> Iterator[KindedIndex]:
    """The tree's legacy files copied into a ``KindedIndex``, whose root is
    regenerated from them: for a test that changes the catalog mid-run."""
    with _served(kind, work) as index:
        for name in ("all.json", "stats.json"):
            shutil.copyfile(TREE / name, index.root / name)
        yield index


@pytest.mark.parametrize(("leg", "kind"), ALL_CELLS)
def test_each_grim_lists_the_conformance_generation_in_each_index_kind(
    leg: str, kind: ConformanceKind, runner_for, tmp_path: Path
) -> None:
    with _served_tree(kind, tmp_path) as (base, log):
        envelope, stderr = _search(runner_for(leg, base), "--show-deprecated")

    paths = [r.path for r in log]
    if _reads(leg, kind):
        expected = _tree_rows()
        assert _rows(envelope) == expected
        assert len(envelope["items"]) == len(expected)
        assert [s["ok"] for s in envelope["sources"]] == [True], envelope["sources"]
        assert [
            line for line in stderr.splitlines() if "unsupported schema 2" not in line
        ] == [], "the schema-2 pointer is the only thing worth a warning"
    else:
        assert envelope["items"] == [], "a cold start on a c/-only index is empty"
        assert [s["ok"] for s in envelope["sources"]] == [False], envelope["sources"]
    if leg != "g2":
        assert set(paths) <= {"/all.json", "/stats.json"}, (
            "an old grim never asks for c/"
        )
        return
    if kind != "legacy-only":
        assert (TREE / "c" / "p").is_dir() and (TREE / "c" / "s").is_dir(), (
            "the tree publishes planes and shards, or the check below cannot fail"
        )
        assert not [p for p in paths if p.startswith(("/c/p/", "/c/s/"))], (
            "a flat and stats search asks for neither planes nor shards"
        )
    assert ("/all.json" in paths) == (kind == "legacy-only"), (
        "the flat blob replaces all.json"
    )
    assert any(p.startswith("/c/f/") for p in paths) == (kind != "legacy-only"), paths


@pytest.mark.parametrize(("leg", "kind"), READABLE_CELLS)
def test_every_grim_picks_the_last_of_two_pointers_to_one_ref(
    leg: str, kind: ConformanceKind, runner_for, tmp_path: Path
) -> None:
    shared = "ghcr.io/acme/shared"
    first, last = [p for p in _tree_pointers() if p["ref"] == shared]
    assert first["description"] != last["description"], "two different pointers"
    with _served_tree(kind, tmp_path) as (base, _):
        envelope, _ = _search(runner_for(leg, base))

    (winner,) = [i for i in envelope["items"] if i["repo"] == shared]
    assert winner["description"] == last["description"]


def test_the_built_grim_fetches_the_flat_blob_only_when_the_flat_ref_changes(
    runner_for, tmp_path: Path
) -> None:
    with _seeded_tree("c-plus-legacy", tmp_path) as index:
        runner = runner_for("g2", index.base)

        def blob_fetched() -> bool:
            blob = index.document_paths("flat")[-1]
            fetched = blob in [r.path for r in index.log]
            index.log.clear()
            return fetched

        first, _ = _search(runner)
        assert blob_fetched(), "a cold sync reads the flat blob"

        _search(runner)
        assert not blob_fetched(), "an unchanged flat ref"

        added = package("extra", "skill", "ghcr.io/acme/extra", "Added")
        (index.root / "all.json").write_text(json.dumps([*_tree_pointers(), added]))
        changed, _ = _search(runner)
        assert blob_fetched(), "a new flat ref is fetched"

    assert len(changed["items"]) == len(first["items"]) + 1


@pytest.mark.parametrize("kind", INDEX_KINDS)
def test_the_built_grim_keeps_a_synthetic_null_list_record_and_the_whole_index(
    kind: ConformanceKind, runner_for, tmp_path: Path
) -> None:
    record = _null_list_record()
    with _seeded_tree(kind, tmp_path) as index:
        (index.root / "all.json").write_text(json.dumps([*_tree_pointers(), record]))
        envelope, _ = _search(runner_for("g2", index.base), "--show-deprecated")

    assert [s["ok"] for s in envelope["sources"]] == [True], envelope["sources"]
    assert _rows(envelope) == {**_tree_rows(), record["ref"]: ("skill", None)}


def test_g0_lists_every_schema_1_row_of_the_published_tree(
    runner_for, tmp_path: Path
) -> None:
    expected = _tree_rows()
    with _served_tree("legacy-only", tmp_path) as (base, _):
        envelope, _ = _search(runner_for("g0", base), "--show-deprecated")

    assert [s["ok"] for s in envelope["sources"]] == [True], envelope["sources"]
    assert len(envelope["items"]) == len(expected)
    assert set(_rows(envelope)) == set(expected)
    assert "ghcr.io/acme/hostile-wrong-shape" in expected, (
        "the clamped record is among the rows G0 lists"
    )


def test_g0_still_refuses_a_synthetic_document_with_a_null_list_record(
    runner_for, tmp_path: Path
) -> None:
    with _seeded_tree("legacy-only", tmp_path) as index:
        (index.root / "all.json").write_text(
            json.dumps([*_tree_pointers(), _null_list_record()])
        )
        envelope, _ = _search(runner_for("g0", index.base))

    assert envelope["items"] == []
    assert [s["ok"] for s in envelope["sources"]] == [False], envelope["sources"]


def _write_big_index(index: KindedIndex, *, rating: bool = False) -> None:
    packages = [
        package(
            f"pkg-{i:03d}",
            "skill",
            f"ghcr.io/acme/skills/pkg-{i:03d}",
            "Zebra crossing helper" if i == 550 else "Generic helper",
        )
        for i in range(COUNT)
    ]
    (index.root / "all.json").write_text(json.dumps(packages))
    if rating:
        vote = {
            "up": 5,
            "target": "D_kwDOAbCdEf",
            "url": "https://github.com/acme/index/discussions/7",
        }
        write_stats(index.root, {LATE: {"rating": vote}})


@pytest.mark.parametrize(("leg", "kind"), ALL_CELLS)
def test_an_index_past_500_rows_lists_whole_unless_the_grim_caps_it(
    leg: str, kind: ConformanceKind, runner_for, tmp_path: Path
) -> None:
    with _served(kind, tmp_path) as index:
        _write_big_index(index)
        envelope, _ = _search(runner_for(leg, index.base))

    if not _reads(leg, kind):
        assert envelope["items"] == []
    elif leg == "g0":
        assert len(envelope["items"]) == OLD_CAP, "the control: G0 stops at 500 rows"
    else:
        assert len(envelope["items"]) == COUNT
        assert envelope["total"] == COUNT and envelope["truncated"] is False


@pytest.mark.parametrize(("leg", "kind"), PAGING_CELLS)
def test_a_query_and_a_browse_filter_reach_rows_past_the_old_cap(
    leg: str, kind: ConformanceKind, runner_for, tmp_path: Path
) -> None:
    with _served(kind, tmp_path) as index:
        _write_big_index(index)
        runner = runner_for(leg, index.base)
        queried, _ = _search(runner, "zebra")
        filtered_index_config(runner.cwd, index.base, include=("acme/skills/pkg-55*",))
        filtered, _ = _search(runner)

    assert [i["repo"] for i in queried["items"]] == [LATE]
    assert sorted(i["repo"] for i in filtered["items"]) == [
        f"ghcr.io/acme/skills/pkg-{i:03d}" for i in range(550, 560)
    ]


@pytest.mark.parametrize(("leg", "kind"), PAGING_CELLS)
def test_rate_resolves_a_row_past_the_old_cap(
    leg: str, kind: ConformanceKind, runner_for, tmp_path: Path
) -> None:
    with _served(kind, tmp_path) as index:
        _write_big_index(index, rating=True)
        runner = runner_for(leg, index.base)
        _search(runner)

        result = runner.run("--format", "json", "rate", LATE, "--dry-run", check=False)

    assert result.returncode == 0, (
        f"79 means the row is missing; stderr: {result.stderr}"
    )
    assert json.loads(result.stdout)["ref"] == LATE


@pytest.mark.parametrize(("leg", "kind"), PAGING_CELLS)
def test_status_check_reports_a_deprecation_past_the_old_cap(
    leg: str, kind: ConformanceKind, runner_for, tmp_path: Path, registry: str
) -> None:
    repo = f"grim-test/{uuid.uuid4().hex[:12]}/old-skill"
    make_artifact(
        repo,
        "skill",
        {"old-skill/SKILL.md": "---\nname: old-skill\ndescription: old\n---\n# old\n"},
    )
    with _served(kind, tmp_path) as index:
        _write_big_index(index)
        packages = json.loads((index.root / "all.json").read_text())
        packages[550] = package(
            "old-skill",
            "skill",
            f"{REGISTRY_HOST}/{repo}",
            "Late and deprecated",
            deprecated="use new-skill instead",
        )
        (index.root / "all.json").write_text(json.dumps(packages))
        runner = runner_for(leg, index.base)
        runner.run("add", f"{REGISTRY_HOST}/{repo}:latest")

        report = runner.json("status", "--check")

    row = next((i for i in report["items"] if i["name"] == "old-skill"), None)
    assert row is not None, report
    assert row["deprecated"] == "use new-skill instead", report
