# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""An index built by the pinned indexer still works for the oldest grim that
reads an index and for the grim under test.

The indexer at ``data/index_e2e/INDEXER_REF`` builds the small index root in
``data/index_e2e/`` three ways. With no ``sync`` block it publishes the legacy
files and a ``c/root.json`` that names no flat catalog: grim 0.14.3 predates the
root and must list the same rows as before, and the built grim the same rows
without a word on stderr. With ``layout: anchored`` the root names the flat
blob and the built grim lists the rows the flat build lists, a changed package
costing it one new blob. With ``legacy: false`` as well no legacy file is
published and the built grim never asks for one.
"""

from __future__ import annotations

import gzip
import json
import os
import shutil
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.index_fixture import IndexHandler, Request, index_config, serve
from src.runner import GrimRunner

FIXTURE = Path(__file__).resolve().parents[1] / "data" / "index_e2e"

INDEXER_DIR = os.environ.get("GRIM_INDEXER_DIR")
G0_COMMAND = os.environ.get("GRIM_G0_COMMAND")
REQUIRED = os.environ.get("GRIM_REQUIRE_INDEX_E2E") == "1"

_MISSING = [
    name
    for name, value in (
        ("GRIM_INDEXER_DIR", INDEXER_DIR),
        ("GRIM_G0_COMMAND", G0_COMMAND),
    )
    if not value
]
_HOW_TO_RUN = (
    f"{' and '.join(_MISSING)} must name a built indexer checkout "
    "(`npm ci --ignore-scripts && npm run build`) and a grim 0.14.3 binary; "
    "`task test:index-e2e` resolves the second."
)

if REQUIRED and _MISSING:
    pytest.fail(f"GRIM_REQUIRE_INDEX_E2E=1: {_HOW_TO_RUN}", pytrace=False)

pytestmark = [
    pytest.mark.index_e2e,
    pytest.mark.skipif(bool(_MISSING), reason=_HOW_TO_RUN),
]


@dataclass(frozen=True, slots=True)
class Published:
    """The indexer's input root and output directory, the loopback URL serving
    the output and the log of requests that URL has answered."""

    root: Path
    dist: Path
    base: str
    log: list[Request]


def _build(root: Path, dist: Path) -> None:
    assert INDEXER_DIR is not None
    built = subprocess.run(
        [
            "node",
            str(Path(INDEXER_DIR) / "dist" / "cli" / "index.js"),
            "build",
            str(root),
            "--out-dir",
            str(dist),
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    assert built.returncode == 0, (
        f"indexer build failed:\n{built.stdout}\n{built.stderr}"
    )


@contextmanager
def _published(work: Path, sync: dict[str, object] | None) -> Iterator[Published]:
    """Build the fixture with ``sync`` as its ``sync`` block (none: no block)
    and serve the result."""
    # The indexer renames a scratch directory from under the root into the
    # output directory, which fails across filesystems: both live in one dir.
    root, dist = work / "root", work / "dist"
    shutil.copytree(FIXTURE, root, ignore=shutil.ignore_patterns("INDEXER_REF"))
    if sync is not None:
        config = root / "index.config.json"
        config.write_text(json.dumps({**json.loads(config.read_text()), "sync": sync}))
    _build(root, dist)
    handler = IndexHandler.new()
    with serve(dist, handler) as base:
        yield Published(root, dist, base, handler.log)


@pytest.fixture(scope="module")
def published(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Published]:
    with _published(tmp_path_factory.mktemp("index-e2e"), None) as index:
        yield index


@pytest.fixture(scope="module")
def published_anchored(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Published]:
    work = tmp_path_factory.mktemp("index-e2e-anchored")
    with _published(work, {"layout": "anchored"}) as index:
        yield index


@pytest.fixture(scope="module")
def published_legacy_free(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[Published]:
    work = tmp_path_factory.mktemp("index-e2e-legacy-free")
    with _published(work, {"layout": "anchored", "legacy": False}) as index:
        yield index


@pytest.fixture
def published_incremental(tmp_path: Path) -> Iterator[Published]:
    # Its own build: the test rebuilds it, and a module-scoped one would
    # change under every other test.
    with _published(tmp_path, {"layout": "anchored"}) as index:
        yield index


def _fixture_ratings() -> dict[str, int]:
    entries = json.loads((FIXTURE / ".stats.json").read_text())["entries"]
    return {ref: entry["rating"]["up"] for ref, entry in entries.items()}


def _fixture_refs() -> list[str]:
    metadata = FIXTURE.glob("index/*/*/*/metadata.json")
    return sorted(json.loads(path.read_text())["ref"] for path in metadata)


def _runner(binary: str, published: Published, leg: Path) -> GrimRunner:
    """A grim in a fresh home, configured with the served index as its only
    registry."""
    project = leg / "project"
    project.mkdir(parents=True)
    index_config(project, published.base)
    home = leg / "grim-home"
    home.mkdir()
    return GrimRunner(Path(binary), home, cwd=project)


def _search(runner: GrimRunner) -> tuple[list[dict], str]:
    """``grim search --refresh``: the rows and everything the run wrote to
    stderr."""
    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, (
        f"{runner.binary} search exited {result.returncode}; stderr: {result.stderr}"
    )
    envelope = json.loads(result.stdout)
    assert [s["ok"] for s in envelope["sources"]] == [True], envelope["sources"]
    return envelope["items"], result.stderr


def _list_rows(binary: str, published: Published, leg: Path) -> tuple[list[dict], str]:
    return _search(_runner(binary, published, leg))


def _list_with_requests(
    binary: str, published: Published, leg: Path
) -> tuple[list[dict], str, list[str]]:
    """``_list_rows`` and the paths the served index answered meanwhile."""
    before = len(published.log)
    rows, stderr = _list_rows(binary, published, leg)
    return rows, stderr, [request.path for request in published.log[before:]]


def _ratings_by_repo(rows: list[dict]) -> dict[str, int | None]:
    return {row["repo"]: (row["rating"] or {}).get("up") for row in rows}


def _without_locator(rows: list[dict]) -> list[dict]:
    # `source.locator` is the loopback URL a build is served on, and every
    # build has its own port; it is the only row field that differs.
    return [{**row, "source": {**row["source"], "locator": None}} for row in rows]


def _blob(ref: dict[str, object]) -> str:
    return f"/c/f/sha256/{ref['sha256']}.json.gz"


def test_a_default_build_publishes_a_flat_null_root_and_gzip_siblings(
    published: Published,
) -> None:
    root = json.loads((published.dist / "c" / "root.json").read_text())
    assert root["flat"] is None, "the default config publishes no flat catalog blob"
    for name in ("all.json", "stats.json"):
        sibling = gzip.decompress((published.dist / f"{name}.gz").read_bytes())
        assert sibling == (published.dist / name).read_bytes(), (
            f"{name}.gz must inflate to {name}"
        )


def test_grim_0_14_3_lists_the_fixture_packages_and_rating(
    published: Published, tmp_path: Path
) -> None:
    assert G0_COMMAND is not None
    rows, stderr = _list_rows(G0_COMMAND, published, tmp_path)
    assert stderr == "", "an index with a root is no news to a grim that never reads it"
    ratings = _fixture_ratings()
    refs = _fixture_refs()
    assert len(rows) == len(refs), "one row per fixture package, no duplicates"
    assert {r["repo"]: (r["rating"] or {}).get("up") for r in rows} == {
        ref: ratings.get(ref) for ref in refs
    }


def test_the_built_grim_lists_the_same_rows_with_clean_stderr(
    published: Published, grim_binary: Path, tmp_path: Path
) -> None:
    assert G0_COMMAND is not None
    expected, _ = _list_rows(G0_COMMAND, published, tmp_path / "g0")
    rows, stderr = _list_rows(str(grim_binary), published, tmp_path / "built")
    assert stderr == "", "a default-config index warns about nothing"
    assert rows == expected, "the built grim must list what grim 0.14.3 lists"


def test_the_flat_and_anchored_builds_list_equal_rows(
    published: Published,
    published_anchored: Published,
    grim_binary: Path,
    tmp_path: Path,
) -> None:
    flat, _, flat_asked = _list_with_requests(
        str(grim_binary), published, tmp_path / "flat"
    )
    anchored, stderr, anchored_asked = _list_with_requests(
        str(grim_binary), published_anchored, tmp_path / "anchored"
    )
    assert stderr == "", "an anchored index warns about nothing"
    # Two routes, or the equality below proves nothing: the flat root names no
    # blob, so grim reads /all.json; the anchored root names one.
    assert "/all.json" in flat_asked, flat_asked
    assert "/all.json" not in anchored_asked, anchored_asked
    assert len(anchored) == len(_fixture_refs()), "one row per fixture package"
    assert _without_locator(anchored) == _without_locator(flat)


def test_a_changed_package_costs_the_root_and_one_new_flat_blob(
    published_incremental: Published, grim_binary: Path, tmp_path: Path
) -> None:
    index = published_incremental
    runner = _runner(str(grim_binary), index, tmp_path / "grim")
    before, stderr = _search(runner)
    assert stderr == ""
    first = json.loads((index.dist / "c" / "root.json").read_text())
    seen = {request.path for request in index.log}
    mark = len(index.log)

    # The indexer holds no state between builds without `enrich`: it
    # recompiles the root into the same output directory, and what carries over
    # is grim's catalog.
    beta = index.root / "index" / "github.com" / "acme" / "e2e-beta" / "metadata.json"
    package = json.loads(beta.read_text())
    package["description"] = "Beta rule, edited after the first sync."
    beta.write_text(json.dumps(package))
    _build(index.root, index.dist)
    second = json.loads((index.dist / "c" / "root.json").read_text())
    assert second["flat"]["sha256"] != first["flat"]["sha256"]
    assert second["stats"] == first["stats"], "no package edit reaches the stats"

    after, stderr = _search(runner)
    assert stderr == ""
    asked = [request.path for request in index.log[mark:]]
    # `search --refresh` fetches the stats blob again; it is not a new blob.
    fresh = [p for p in asked if p.startswith("/c/f/") and p not in seen]
    assert fresh == [_blob(second["flat"])], asked
    assert "/c/root.json" in asked
    assert all(p == "/c/root.json" or p.startswith("/c/f/sha256/") for p in asked), (
        asked
    )
    assert after == [
        {**row, "description": package["description"]}
        if row["repo"] == package["ref"]
        else row
        for row in before
    ]


def test_a_legacy_free_build_publishes_only_the_anchored_tree(
    published_legacy_free: Published,
) -> None:
    dist = published_legacy_free.dist
    for name in ("all.json", "all.json.gz", "stats.json", "stats.json.gz"):
        assert not (dist / name).exists(), f"legacy: false publishes no {name}"
    root = json.loads((dist / "c" / "root.json").read_text())
    for ref in (root["flat"], root["stats"]):
        assert (dist / _blob(ref).removeprefix("/")).is_file(), (
            "the root names the blobs a legacy-free reader needs"
        )


def test_the_built_grim_lists_a_legacy_free_index_without_asking_for_all_json(
    published_legacy_free: Published, grim_binary: Path, tmp_path: Path
) -> None:
    rows, stderr, asked = _list_with_requests(
        str(grim_binary), published_legacy_free, tmp_path
    )
    assert stderr == "", "a legacy-free index warns about nothing"
    ratings = _fixture_ratings()
    refs = _fixture_refs()
    assert len(rows) == len(refs), "one row per fixture package, no duplicates"
    assert _ratings_by_repo(rows) == {ref: ratings.get(ref) for ref in refs}
    assert "/c/root.json" in asked
    assert all(p == "/c/root.json" or p.startswith("/c/f/sha256/") for p in asked), (
        f"no /all.json or /stats.json request: {asked}"
    )
