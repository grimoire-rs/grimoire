# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A flat index behaves exactly as it did before the anchored root.

The legacy ``all.json`` + ``stats.json`` index is a frozen contract. Most of
these tests pin what a legacy-only index does at the base this work started
from and must stay green; the probe tests pin the one ``c/root.json`` GET per
refresh that a root-aware client adds, and nothing else a user can see.

The expected rows in ``data/index_flat_unchanged/search_rows.json`` are the
``items`` of ``grim --format json search --refresh`` from the base binary
(``grim 0.14.3`` at ``7b21abce``) against ``_write_legacy_index``'s tree.
Captured by dumping ``rows`` with ``json.dumps(rows, indent=2)``. The one
normalised value is the loopback index URL (``source.locator``), which carries
a random port and is rewritten to ``<index>``; no row carries a timestamp of
its own. Never re-capture from a newer binary: a row that changed is a
contract change.

``data/index_flat_unchanged/catalog_cache.json`` is the ``CatalogFile`` that
same ``grim 0.14.3`` wrote to ``$GRIM_HOME/catalog/<h16>.json`` for the same
tree, after two ``search --refresh`` runs. Only three values are rewritten, as
text substitutions on the raw bytes so key order and whitespace survive:
``built_at`` and every ``fetched_at`` become ``<built_at>`` / ``<fetched_at>``,
and the loopback port becomes ``<port>``. Never re-capture it from a newer
binary: a byte that changed is a contract change.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.index_fixture import (
    Fault,
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

EXPECTED_CACHE = (
    Path(__file__).parent.parent
    / "data"
    / "index_flat_unchanged"
    / "catalog_cache.json"
)
# Applied to the raw cache bytes; the fixture was normalised with the same two.
VOLATILE_CACHE_VALUES = [
    (re.compile(rb'"(built_at|fetched_at)": "[^"]*"'), rb'"\1": "<\1>"'),
    (re.compile(rb"(127\.0\.0\.1:)\d+"), rb"\1<port>"),
]


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


def _normalised_cache(grim_home: Path) -> str:
    caches = [
        p
        for p in (grim_home / "catalog").glob("*.json")
        if not p.name.endswith(".sync.json")
    ]
    assert len(caches) == 1, f"one registry, one cache file; got {caches}"
    assert re.fullmatch(r"[0-9a-f]{16}", caches[0].stem), caches[0].name
    raw = caches[0].read_bytes()
    for pattern, replacement in VOLATILE_CACHE_VALUES:
        raw = pattern.sub(replacement, raw)
    return raw.decode()


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


def test_a_flat_index_writes_the_catalog_cache_bytes_of_the_base(
    flat_index: FlatIndex, grim_home: Path
) -> None:
    expected = EXPECTED_CACHE.read_bytes().decode()
    for n in (1, 2):
        flat_index.refresh()
        assert _normalised_cache(grim_home) == expected, f"refresh {n}"


def test_each_refresh_sends_exactly_one_root_probe(flat_index: FlatIndex) -> None:
    for n in (1, 2):
        paths = flat_index.refresh().paths
        assert paths.count(ROOT_PROBE) == 1, f"refresh {n}: saw {paths}"


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


@pytest.mark.parametrize(
    "fault",
    [
        pytest.param(Fault(status=404), id="not-found"),
        pytest.param(Fault(status=403), id="forbidden"),
        pytest.param(Fault(status=500), id="server-error"),
        pytest.param(Fault(body=b"<html>welcome</html>"), id="html-page"),
    ],
)
def test_an_unusable_root_probe_leaves_the_flat_refresh_unchanged(
    flat_index: FlatIndex, fault: Fault
) -> None:
    """A root that cannot be used sends the refresh down the legacy path: same
    rows, nothing on stderr, still one probe and the two legacy files."""
    flat_index.handler.faults[ROOT_PROBE] = fault
    expected = _expected_rows()
    for n in (1, 2):
        refresh = flat_index.refresh()
        assert refresh.stderr == "", f"refresh {n}: {refresh.stderr}"
        assert refresh.rows == expected, f"refresh {n}"
        assert refresh.paths.count(ROOT_PROBE) == 1, f"refresh {n}: {refresh.paths}"
        assert refresh.legacy_paths == LEGACY_REQUESTS, f"refresh {n}"


def test_a_throttled_root_probe_warns_once_and_the_refresh_still_lists_the_rows(
    flat_index: FlatIndex,
) -> None:
    flat_index.handler.faults[ROOT_PROBE] = Fault(
        status=429, headers={"Retry-After": "120"}
    )
    refresh = flat_index.refresh()
    lines = [line for line in refresh.stderr.splitlines() if line.strip()]
    assert len(lines) == 1, refresh.stderr
    assert "rate limiting" in lines[0], lines[0]
    assert flat_index.base.removeprefix("http://") in lines[0], lines[0]
    assert refresh.rows == _expected_rows()
