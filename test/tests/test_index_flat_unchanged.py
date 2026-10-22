# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A flat index behaves exactly as it did before the anchored root.

The legacy ``all.json`` + ``stats.json`` index is a frozen contract. The tests
that are not skipped pin what a legacy-only index does at the base this work
started from and must stay green; the skipped ones are the probe contract,
which adds exactly one ``c/root.json`` GET per refresh and nothing else a user
can see.

The expected rows in ``data/index_flat_unchanged/search_rows.json`` are the
``items`` of ``grim --format json search --refresh`` from the base binary
(``grim 0.14.3`` at ``7b21abce``) against ``_write_legacy_index``'s tree.
Captured by dumping ``rows`` with ``json.dumps(rows, indent=2)``. The one
normalised value is the loopback index URL (``source.locator``), which carries
a random port and is rewritten to ``<index>``; no row carries a timestamp of
its own. Never re-capture from a newer binary: a row that changed is a
contract change.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.index_fixture import (
    IndexHandler,
    index_config,
    package,
    serve,
    write_all_json,
    write_root_json,
    write_stats,
)
from src.runner import GrimRunner

EXPECTED_ROWS = (
    Path(__file__).parent.parent / "data" / "index_flat_unchanged" / "search_rows.json"
)
ROOT_PROBE = "/c/root.json"
LEGACY_REQUESTS = ["/all.json", "/stats.json"]
INDEX_PLACEHOLDER = "<index>"

# `built_at` and `fetched_at` are the volatile values; only key sets are pinned.
_ENTRY_BASE = [
    "description",
    "fetched_at",
    "kind",
    "registry",
    "repository",
    "repository_url",
]
EXPECTED_CACHE_SHAPE = {
    "top": ["built_at", "entries", "registry", "scope", "truncated", "version"],
    "entries": {
        "ghcr.io/acme/agents/delta": sorted(
            [*_ENTRY_BASE, "deprecated", "replaced_by"]
        ),
        "ghcr.io/acme/skills/alpha": sorted(
            [*_ENTRY_BASE, "downloads", "keywords", "rating", "summary"]
        ),
        "ghcr.io/acme/skills/beta": sorted([*_ENTRY_BASE, "downloads"]),
        "registry.example/acme/rules/gamma": sorted(
            [*_ENTRY_BASE, "keywords", "rating"]
        ),
    },
    "rating": ["host", "provider", "target", "up", "url"],
    "downloads": ["as_of", "total", "versions"],
}


def _write_legacy_index(root: Path) -> None:
    write_all_json(
        root,
        [
            package(
                "alpha",
                "skill",
                "ghcr.io/acme/skills/alpha",
                "Alpha skill",
                keywords=["review", "lint"],
                summary="Reviews code",
            ),
            package("beta", "skill", "ghcr.io/acme/skills/beta", "Beta skill"),
            package(
                "gamma",
                "rule",
                "registry.example/acme/rules/gamma",
                "Gamma rule",
                keywords=["style"],
            ),
            package(
                "delta",
                "agent",
                "ghcr.io/acme/agents/delta",
                "Delta agent",
                deprecated="use alpha",
                replaced_by="ghcr.io/acme/skills/alpha",
            ),
        ],
    )
    write_stats(
        root,
        {
            "ghcr.io/acme/skills/alpha": {
                "rating": {
                    "up": 12,
                    "target": "D_kwDOAlpha",
                    "url": "https://github.com/acme/index/discussions/7",
                },
                "downloads": {
                    "total": 1416,
                    "versions": {"1.0.1": 400, "1.0.10": 116, "1.0.2": 900},
                    "as_of": "2026-09-10T21:48:47Z",
                },
            },
            "ghcr.io/acme/skills/beta": {
                "downloads": {
                    "total": 5,
                    "versions": {"1.0.0": 5},
                    "as_of": "2026-09-10T21:48:47Z",
                }
            },
            "registry.example/acme/rules/gamma": {
                "rating": {
                    "up": 3,
                    "target": "D_kwDOGamma",
                    "url": "https://github.com/acme/index/discussions/8",
                }
            },
        },
        rating_host="github.com",
        downloads_provider="ghcr",
    )


@dataclass(slots=True)
class Refresh:
    """One forced ``grim search --refresh``: what it printed and what it asked for."""

    rows: list[dict]
    sources: list[dict]
    stderr: str
    paths: list[str]

    @property
    def legacy_paths(self) -> list[str]:
        return sorted(p for p in self.paths if p != ROOT_PROBE)


@dataclass(slots=True)
class FlatIndex:
    """A served legacy-only index tree and the runner searching it."""

    root: Path
    base: str
    handler: type[IndexHandler]
    runner: GrimRunner

    def refresh(self) -> Refresh:
        seen = len(self.handler.log)
        result = self.runner.run("--format", "json", "search", "--refresh", check=False)
        assert result.returncode == 0, (
            f"search must exit 0, got {result.returncode}; stderr: {result.stderr}"
        )
        envelope = json.loads(result.stdout.replace(self.base, INDEX_PLACEHOLDER))
        return Refresh(
            rows=envelope["items"],
            sources=envelope["sources"],
            stderr=result.stderr,
            paths=[r.path for r in self.handler.log[seen:]],
        )


@pytest.fixture()
def flat_index(
    tmp_path: Path, project_dir: Path, grim_at: Callable[[Path], GrimRunner]
) -> Iterator[FlatIndex]:
    root = tmp_path / "index-dist-flat"
    root.mkdir()
    _write_legacy_index(root)
    handler = IndexHandler.new()
    with serve(root, handler) as base:
        index_config(project_dir, base)
        yield FlatIndex(root, base, handler, grim_at(project_dir))


def _expected_rows() -> list[dict]:
    return json.loads(EXPECTED_ROWS.read_text())


def _cache_shape(grim_home: Path) -> dict:
    caches = list((grim_home / "catalog").glob("*.json"))
    assert len(caches) == 1, f"one registry, one cache file; got {caches}"
    assert re.fullmatch(r"[0-9a-f]{16}", caches[0].stem), caches[0].name
    doc = json.loads(caches[0].read_text())
    alpha = doc["entries"]["ghcr.io/acme/skills/alpha"]
    return {
        "top": sorted(doc),
        "entries": {ref: sorted(entry) for ref, entry in doc["entries"].items()},
        "rating": sorted(alpha["rating"]),
        "downloads": sorted(alpha["downloads"]),
    }


def test_a_flat_index_lists_the_pinned_rows_with_clean_stderr(
    flat_index: FlatIndex,
) -> None:
    expected = _expected_rows()
    for n in (1, 2):
        refresh = flat_index.refresh()
        assert refresh.stderr == "", f"refresh {n}: a flat index warns about nothing"
        assert refresh.rows == expected, f"refresh {n}: rows drifted from the base"
        assert [s["ok"] for s in refresh.sources] == [True], refresh.sources


def test_a_flat_index_fetches_all_json_and_stats_json_once_per_refresh(
    flat_index: FlatIndex,
) -> None:
    for n in (1, 2):
        refresh = flat_index.refresh()
        assert refresh.legacy_paths == LEGACY_REQUESTS, (
            f"refresh {n}: a flat refresh asks for nothing but the two legacy files; "
            f"saw {refresh.paths}"
        )


def test_a_flat_index_keeps_the_catalog_cache_key_set(
    flat_index: FlatIndex, grim_home: Path
) -> None:
    for n in (1, 2):
        flat_index.refresh()
        assert _cache_shape(grim_home) == EXPECTED_CACHE_SHAPE, f"refresh {n}"


@pytest.mark.skip(reason="contract: GC")
def test_each_refresh_sends_exactly_one_root_probe(flat_index: FlatIndex) -> None:
    for n in (1, 2):
        paths = flat_index.refresh().paths
        assert paths.count(ROOT_PROBE) == 1, f"refresh {n}: saw {paths}"


@pytest.mark.skip(reason="contract: GC")
def test_an_l0_prime_index_gives_the_same_rows_and_clean_stderr(
    flat_index: FlatIndex,
) -> None:
    """L0': the legacy files plus a root with ``flat: null``, which a flat-sync
    client treats as no root at all."""
    write_root_json(flat_index.root, flat=None, stats=None)
    expected = _expected_rows()
    for n in (1, 2):
        refresh = flat_index.refresh()
        assert refresh.stderr == "", f"refresh {n}: L0' warns about nothing"
        assert refresh.rows == expected, (
            f"refresh {n}: L0' rows differ from the flat index"
        )
