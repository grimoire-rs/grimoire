# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Paging ``grim search`` over ``oci`` sources and mixed oci + index scopes.

``test_index_scale.py`` pages an index. An ``oci`` source differs in two
ways a paging consumer feels: its ``_catalog`` listing is capped at 500
repositories when the catalog is built, so ``total`` is then a lower bound
and ``truncated`` is true on every page, and its rows come from a different
code path than an index's. These tests pin that the pages still partition
the one ordered list.
"""

from __future__ import annotations

import json
import threading
import uuid
from http.server import SimpleHTTPRequestHandler
from pathlib import Path

import pytest

from src.helpers import make_artifact, paged_namespace
from src.index_fixture import (
    index_config,
    package,
    serve,
    write_all_json,
    write_stats,
)
from src.registry import REGISTRY_HOST

CAP = 500
UNDER = 150
OVER = 600


def _fresh_namespace() -> str:
    return f"grim-test/{uuid.uuid4().hex[:12]}"


@pytest.fixture(scope="module")
def ns_under(registry: str) -> str:
    """A namespace of 150 repos, under the cap."""
    return paged_namespace(UNDER)


@pytest.fixture(scope="module")
def ns_over(registry: str) -> str:
    """A namespace of 600 repos, past the 500 cap (~2.5 s to push)."""
    return paged_namespace(OVER)


def _locator(ns: str) -> str:
    return f"{REGISTRY_HOST}/{ns}"


def _envelope(runner, *args: str, offline: bool = False) -> dict:
    """``grim --format json search --refresh|--offline <args>``, exit 0."""
    mode = "--offline" if offline else "--refresh"
    result = runner.run("--format", "json", "search", mode, *args, check=False)
    assert result.returncode == 0, (
        f"search {args} must exit 0, got {result.returncode}; stderr: {result.stderr}"
    )
    return json.loads(result.stdout)


def _walk(runner, size: int, *extra: str) -> list[dict]:
    """The documented paging recipe: a refreshing first call, ``--offline``
    after it, ``offset += len(items)`` while ``offset < total``."""
    pages: list[dict] = []
    offset = 0
    while True:
        page = _envelope(
            runner,
            *extra,
            "--offset",
            str(offset),
            "--limit",
            str(size),
            offline=bool(pages),
        )
        pages.append(page)
        offset += len(page["items"])
        if not page["items"] or offset >= page["total"]:
            return pages


def _keys(envelope_or_pages) -> list[tuple[str | None, str]]:
    pages = (
        envelope_or_pages
        if isinstance(envelope_or_pages, list)
        else [envelope_or_pages]
    )
    return [(r["source"]["alias"], r["repo"]) for p in pages for r in p["items"]]


def _rows(pages: list[dict]) -> list[dict]:
    """Every row of every page, whole, in page order."""
    return [r for p in pages for r in p["items"]]


def _write_config(project_dir: Path, *entries: str) -> None:
    (project_dir / "grimoire.toml").write_text(
        "\n".join(entries) + "\n[skills]\n\n[rules]\n"
    )


def _oci_entry(alias: str, ns: str) -> str:
    return f'[[registries]]\nalias = "{alias}"\noci = "{_locator(ns)}"\n'


def _index_entry(alias: str, base: str) -> str:
    return f'[[registries]]\nalias = "{alias}"\nindex = "{base}"\n'


def test_oci_pages_partition_the_unpaged_list(grim_at, project_dir, ns_under) -> None:
    runner = grim_at(project_dir)
    scope = ("--registry", _locator(ns_under))

    full = _envelope(runner, *scope)
    assert len(full["items"]) == UNDER
    assert full["total"] == UNDER
    assert full["truncated"] is False

    for size in (100, 70, 33):
        pages = _walk(runner, size, *scope)
        repos = [r["repo"] for r in _rows(pages)]
        assert _rows(pages) == full["items"], (
            f"pages of {size} must concatenate to the unpaged list"
        )
        assert len(set(repos)) == UNDER, "no repo may appear on two pages"
        assert all(p["total"] == UNDER for p in pages)
        assert pages[0]["truncated"] is (size < UNDER)
        assert all(p["truncated"] for p in pages[1:]), (
            "a page at offset > 0 skipped rows, so it is truncated"
        )


def test_oci_over_the_cap_reports_a_lower_bound_on_every_page(
    grim_at, project_dir, ns_over
) -> None:
    runner = grim_at(project_dir)
    scope = ("--registry", _locator(ns_over))

    full = _envelope(runner, *scope)
    assert len(full["items"]) == CAP
    assert full["total"] == CAP
    assert full["truncated"] is True
    assert full["sources"][0]["ok"] is True

    for size in (100, 70):
        pages = _walk(runner, size, *scope)
        assert len(pages) == -(-CAP // size)
        assert all(p["total"] == CAP and p["truncated"] is True for p in pages)
        assert _rows(pages) == full["items"], f"pages of {size} must stitch"
        repos = [r["repo"] for r in _rows(pages)]
        assert len(set(repos)) == CAP
        assert repos[-1] == f"{_locator(ns_over)}/pkg-499"

    past = runner.run(
        "--format", "json", "search", "--offline", *scope,
        "--offset", str(CAP), "--limit", "100", check=False,
    )
    assert past.returncode == 0, past.stderr
    empty = json.loads(past.stdout)
    assert empty["items"] == []
    assert empty["total"] == CAP
    assert empty["truncated"] is True


def test_a_query_cannot_reach_past_the_oci_cap(grim_at, project_dir, ns_over) -> None:
    runner = grim_at(project_dir)
    scope = ("--registry", _locator(ns_over))

    inside = _envelope(runner, *scope, "pkg-499")
    assert [r["repo"] for r in inside["items"]] == [f"{_locator(ns_over)}/pkg-499"]

    result = runner.run(
        "--format", "json", "search", "--refresh", *scope, "pkg-550", check=False
    )
    assert result.returncode == 0, result.stderr
    past = json.loads(result.stdout)
    assert past["items"] == []
    assert past["total"] == 0
    assert past["truncated"] is True, "rows past the cap were never seen"
    assert "capped at 500" in result.stderr


def _index_rows(count: int) -> list[dict]:
    return [
        package(
            f"idx-{i:03d}",
            "skill",
            f"ghcr.io/acme/skills/idx-{i:03d}",
            "Helper for indexes" if i % 3 == 0 else "Generic tooling",
        )
        for i in range(count)
    ]


def test_mixed_oci_and_index_pages_partition_the_unpaged_list(
    grim_at, project_dir, http_index, ns_under
) -> None:
    root, base = http_index
    write_all_json(root, _index_rows(150))
    _write_config(project_dir, _oci_entry("reg", ns_under), _index_entry("hub", base))
    runner = grim_at(project_dir)

    full = _envelope(runner)
    assert full["total"] == 300
    assert full["truncated"] is False
    assert [r["source"]["alias"] for r in full["items"][:150]] == ["reg"] * 150, (
        "an empty query lists sources in declaration order"
    )

    for size in (100, 70, 333):
        pages = _walk(runner, size)
        assert _rows(pages) == full["items"], f"pages of {size} must stitch"
        assert pages[-1]["total"] == 300
        assert len(pages) == -(-300 // size)

    for extra, size in ((("helper",), 40), (("--sort", "name"), 90)):
        unpaged = _envelope(runner, *extra)
        pages = _walk(runner, size, *extra)
        assert _rows(pages) == unpaged["items"], (
            f"{extra} at size {size} must stitch"
        )
        assert all(p["total"] == unpaged["total"] for p in pages)


def test_mixed_scope_with_a_capped_oci_source(
    grim_at, project_dir, http_index, ns_over
) -> None:
    root, base = http_index
    write_all_json(root, _index_rows(150))
    _write_config(project_dir, _oci_entry("reg", ns_over), _index_entry("hub", base))
    runner = grim_at(project_dir)

    full = _envelope(runner)
    assert len(full["items"]) == CAP + 150
    assert full["total"] == CAP + 150
    assert full["truncated"] is True
    assert [s["ok"] for s in full["sources"]] == [True, True]

    for size in (100, 70, 333):
        pages = _walk(runner, size)
        assert _rows(pages) == full["items"], f"pages of {size} must stitch"
        assert all(p["truncated"] for p in pages), "the cap taints every page"


def test_duplicate_refs_across_two_sources_page_deterministically(
    grim_at, project_dir, ns_under
) -> None:
    """A ref served by two sources pages as one stable list, source ``a``
    first. Incoming order is already declaration order, so this cannot go red
    if the source-rank tie key is deleted; the Rust unit test
    ``a_reference_served_by_two_sources_orders_by_declaration_rank_in_every_mode``
    (reversed input) is that key's guard.
    """
    _write_config(project_dir, _oci_entry("a", ns_under), _oci_entry("b", ns_under))
    runner = grim_at(project_dir)

    for extra in ((), ("--sort", "name")):
        unpaged = _envelope(runner, *extra)
        pages = _walk(runner, 77, *extra)
        keys = _keys(pages)
        assert _rows(pages) == unpaged["items"], (
            f"{extra or 'relevance'} pages must stitch"
        )
        assert len(keys) == 2 * UNDER
        assert len(set(keys)) == len(keys), "(alias, repo) is unique"
        position = {key: i for i, key in enumerate(keys)}
        for alias, repo in keys:
            if alias == "a":
                assert position[("a", repo)] < position[("b", repo)], (
                    f"duplicate {repo} must list source a before b ({extra})"
                )


def test_an_exact_reference_query_ranks_that_row_first(
    grim_at, project_dir, http_index
) -> None:
    ns = _fresh_namespace()
    for name in ("pkg-010", "pkg-010-extra"):
        make_artifact(
            f"{ns}/{name}",
            "skill",
            {"p/SKILL.md": "---\nname: p\n---\n# P\n"},
            annotations={"org.opencontainers.image.description": "Generic helper"},
        )
    _write_config(project_dir, _oci_entry("reg", ns))
    exact = f"{_locator(ns)}/pkg-010"

    top = _envelope(grim_at(project_dir), "--limit", "10", "--", exact)
    assert top["items"][0]["repo"] == exact

    root, base = http_index
    ref = "ghcr.io/acme/skills/one"
    write_all_json(
        root,
        [
            package("one", "skill", ref, "First"),
            package("one-extra", "skill", f"{ref}-extra", "Second"),
        ],
    )
    write_stats(
        root,
        {
            ref: {
                "rating": {"up": 7, "target": "t", "url": "https://f/1"},
                "downloads": {"total": 42, "versions": {}, "as_of": "2026-09-10T00:00:00Z"},
            }
        },
        downloads_provider="artifactory",
    )
    index_config(project_dir, base)

    only = _envelope(grim_at(project_dir), "--limit", "10", "--", ref)
    row = only["items"][0]
    assert row["repo"] == ref
    assert row["rating"] is not None and row["rating"]["up"] == 7
    assert row["downloads"] is not None and row["downloads"]["total"] == 42


def test_offline_pages_after_a_stale_first_page(grim_at, project_dir, tmp_path) -> None:
    root = tmp_path / "index-dist-paging-stale"
    root.mkdir()
    write_all_json(root, _index_rows(6))
    failing = threading.Event()

    class Flaky(SimpleHTTPRequestHandler):
        def do_GET(self):  # http.server's own casing
            if self.path.endswith("all.json") and failing.is_set():
                self.send_error(500)
                return
            super().do_GET()

    with serve(root, Flaky) as base:
        index_config(project_dir, base)
        runner = grim_at(project_dir)
        assert _envelope(runner)["total"] == 6, "the cache must warm first"

        failing.set()
        first = _envelope(runner, "--offset", "0", "--limit", "3")
        second = _envelope(runner, "--offset", "3", "--limit", "3", offline=True)

    assert first["sources"][0]["ok"] is False, "a stale first page reports the failure"
    assert len(first["items"]) == 3
    assert second["sources"][0]["ok"] is True, "an --offline page reads the cache cleanly"
    keys = _keys([first, second])
    assert len(keys) == 6 and len(set(keys)) == 6
    assert first["total"] == second["total"] == 6


def test_sources_report_which_oci_listing_was_capped(
    grim_at, project_dir, http_index, ns_over, ns_under
) -> None:
    root, base = http_index
    write_all_json(root, _index_rows(150))
    _write_config(
        project_dir,
        _oci_entry("big", ns_over),
        _oci_entry("small", ns_under),
        _index_entry("hub", base),
    )
    runner = grim_at(project_dir)

    for page in _walk(runner, 200):
        capped = {s["alias"]: s["capped"] for s in page["sources"]}
        assert capped == {"big": True, "small": False, "hub": False}, (
            "every page names the capped source, so a consumer can tell a cap "
            f"from an ordinary page: {page['sources']}"
        )
