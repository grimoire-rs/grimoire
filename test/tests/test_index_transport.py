# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Index transport: gzip negotiation and the decoded-size cap.

grim sends ``Accept-Encoding: gzip`` and decodes a gzip ``all.json``. A
decoded body over 128 MiB degrades the source (``ok: false``) instead of
exhausting memory; an over-cap ``stats.json`` keeps the previous ratings.
A refresh asks for the root once and for nothing else in the ``c/`` tree
beyond the blobs the root names.
"""

from __future__ import annotations

import functools
import gzip
import io
from pathlib import Path
from urllib.parse import urlsplit

from src.index_fixture import (
    Fault,
    KindedIndex,
    index_config,
    package,
    search_envelope,
    write_all_json,
    write_stats,
)

DECODED_CAP = 128 * 1024 * 1024
ENTRY_LIMIT = 100_000
UNPUBLISHED_TREE = ("/c/p/", "/c/s/", "/c/q/", "/c/root.json.")


def test_grim_asks_for_gzip_and_decodes_it(
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    write_all_json(
        kind_server.root,
        [
            package("one", "skill", "ghcr.io/acme/skills/one", "First"),
            package("two", "skill", "ghcr.io/acme/skills/two", "Second"),
        ],
    )
    kind_server.fault("flat", Fault(gzipped=True))
    index_config(project_dir, kind_server.base)
    envelope = search_envelope(grim_at(project_dir))

    assert sorted(r["repo"] for r in envelope["items"]) == [
        "ghcr.io/acme/skills/one",
        "ghcr.io/acme/skills/two",
    ], f"a gzip index body must decode; sources: {envelope['sources']}"
    flat_paths = set(kind_server.document_paths("flat"))
    accepted = [
        r.headers.get("accept-encoding")
        for r in kind_server.log
        if r.path in flat_paths
    ]
    assert any("gzip" in (v or "") for v in accepted), (
        f"grim must advertise gzip; saw Accept-Encoding: {accepted}"
    )


def test_a_refresh_asks_for_the_root_once_and_for_no_other_part_of_the_tree(
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    write_all_json(
        kind_server.root,
        [
            package("one", "skill", "ghcr.io/acme/skills/one", "First"),
            package("two", "skill", "ghcr.io/acme/skills/two", "Second"),
        ],
    )
    write_stats(
        kind_server.root,
        {"ghcr.io/acme/skills/one": {"rating": {"up": 3, "target": "t", "url": "u"}}},
    )
    index_config(project_dir, kind_server.base)
    runner = grim_at(project_dir)

    for refresh in (1, 2):
        seen = len(kind_server.log)
        search_envelope(runner)
        paths = [urlsplit(r.path).path for r in kind_server.log[seen:]]
        assert paths.count("/c/root.json") == 1, f"refresh {refresh}: {paths}"
        strays = [p for p in paths if p.startswith(UNPUBLISHED_TREE)]
        assert strays == [], f"refresh {refresh}: {strays}"


@functools.cache
def _gzip_bomb(head: bytes = b"[", tail: bytes = b"]") -> bytes:
    """About 130 KB of gzip that decodes to ``head`` + whitespace past the
    cap + ``tail``: a well-formed document when the defaults are used, so only
    the size cap can reject it."""
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", compresslevel=9) as gz:
        gz.write(head)
        chunk = b" " * (1024 * 1024)
        for _ in range(DECODED_CAP // len(chunk) + 2):
            gz.write(chunk)
        gz.write(tail)
    return out.getvalue()


def test_a_gzip_bomb_degrades_the_source(
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    write_all_json(kind_server.root, [])
    bomb = _gzip_bomb()
    assert len(bomb) < 200 * 1024, "the fixture must stay a small download"
    kind_server.fault("flat", Fault(body=bomb, headers={"Content-Encoding": "gzip"}))
    index_config(project_dir, kind_server.base)
    envelope = search_envelope(grim_at(project_dir))

    assert envelope["items"] == []
    source = envelope["sources"][0]
    assert source["ok"] is False, f"an over-cap body must fail the source; got {source}"


def test_an_over_cap_stats_sidecar_keeps_the_previous_rating(
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    """An over-cap ``stats.json`` was not observed, so the warm cache's
    rating carries forward (the same arm a 503 takes) and the build exits 0."""
    ref = "ghcr.io/acme/skills/rated"
    write_all_json(kind_server.root, [package("rated", "skill", ref, "Rated")])
    write_stats(
        kind_server.root, {ref: {"rating": {"up": 12, "target": "t", "url": "u"}}}
    )
    # A valid sidecar that declares nothing rated: parsed, it would clear the
    # rating, so only the size cap keeps the previous one.
    bomb = _gzip_bomb(
        b'{"schema_version": 1, "providers": {"rating": "github"}, "entries": {}',
        b"}",
    )
    index_config(project_dir, kind_server.base)
    runner = grim_at(project_dir)

    warm = search_envelope(runner)
    assert warm["items"][0]["rating"]["up"] == 12, "the warm build must see the rating"

    kind_server.fault("stats", Fault(body=bomb, headers={"Content-Encoding": "gzip"}))
    envelope = search_envelope(runner)

    assert envelope["sources"][0]["ok"] is True, (
        f"a sidecar never fails the source; got {envelope['sources']}"
    )
    assert [r["repo"] for r in envelope["items"]] == [ref]
    assert envelope["items"][0]["rating"]["up"] == 12, (
        f"an over-cap sidecar is unobserved, not empty; got {envelope['items'][0]}"
    )


def test_an_index_listing_too_many_entries_degrades_the_source(
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    """The byte cap alone admits millions of tiny entries: the count has its
    own bound, and one past it fails the source instead of truncating."""
    minimal = [
        {"schema": 1, "name": f"p{i}", "kind": "skill", "ref": f"ghcr.io/acme/s/p{i}"}
        for i in range(ENTRY_LIMIT + 1)
    ]
    write_all_json(kind_server.root, minimal)
    index_config(project_dir, kind_server.base)
    envelope = search_envelope(grim_at(project_dir))

    assert envelope["items"] == [], (
        "an over-limit index must not be truncated into a listing"
    )
    source = envelope["sources"][0]
    assert source["ok"] is False, (
        f"an over-limit index must fail the source; got {source}"
    )
