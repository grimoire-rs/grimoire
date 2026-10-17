# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Index transport: gzip negotiation and the decoded-size cap.

grim sends ``Accept-Encoding: gzip`` and decodes a gzip ``all.json``. A
decoded body over 128 MiB degrades the source (``ok: false``) instead of
exhausting memory; an over-cap ``stats.json`` keeps the previous ratings.
"""

from __future__ import annotations

import functools
import gzip
import io
import threading
from http.server import SimpleHTTPRequestHandler
from pathlib import Path

from src.index_fixture import (
    index_config,
    package,
    search_envelope,
    serve,
    write_all_json,
    write_stats,
)

DECODED_CAP = 128 * 1024 * 1024
ENTRY_LIMIT = 100_000


def _gzip_handler(
    body: bytes,
) -> tuple[type[SimpleHTTPRequestHandler], list[str | None]]:
    """A handler serving ``all.json`` as ``body`` with ``Content-Encoding:
    gzip``, plus the log of ``Accept-Encoding`` values it was asked with."""
    accept_encodings: list[str | None] = []

    class GzipAllJson(SimpleHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 — http.server's own casing
            if not self.path.endswith("all.json"):
                super().do_GET()
                return
            accept_encodings.append(self.headers.get("Accept-Encoding"))
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Encoding", "gzip")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    return GzipAllJson, accept_encodings


def test_grim_asks_for_gzip_and_decodes_it(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    root = tmp_path / "index-dist-gzip"
    root.mkdir()
    write_all_json(
        root,
        [
            package("one", "skill", "ghcr.io/acme/skills/one", "First"),
            package("two", "skill", "ghcr.io/acme/skills/two", "Second"),
        ],
    )
    handler, accept_encodings = _gzip_handler(
        gzip.compress((root / "all.json").read_bytes())
    )
    with serve(root, handler) as base:
        index_config(project_dir, base)
        envelope = search_envelope(grim_at(project_dir))

    assert sorted(r["repo"] for r in envelope["items"]) == [
        "ghcr.io/acme/skills/one",
        "ghcr.io/acme/skills/two",
    ], f"a gzip index body must decode; sources: {envelope['sources']}"
    assert any("gzip" in (v or "") for v in accept_encodings), (
        f"grim must advertise gzip; saw Accept-Encoding: {accept_encodings}"
    )


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
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    root = tmp_path / "index-dist-bomb"
    root.mkdir()
    bomb = _gzip_bomb()
    assert len(bomb) < 200 * 1024, "the fixture must stay a small download"
    handler, _ = _gzip_handler(bomb)
    with serve(root, handler) as base:
        index_config(project_dir, base)
        envelope = search_envelope(grim_at(project_dir))

    assert envelope["items"] == []
    source = envelope["sources"][0]
    assert source["ok"] is False, f"an over-cap body must fail the source; got {source}"


def test_an_over_cap_stats_sidecar_keeps_the_previous_rating(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    """An over-cap ``stats.json`` was not observed, so the warm cache's
    rating carries forward (the same arm a 503 takes) and the build exits 0."""
    ref = "ghcr.io/acme/skills/rated"
    root = tmp_path / "index-dist-stats-bomb"
    root.mkdir()
    write_all_json(root, [package("rated", "skill", ref, "Rated")])
    write_stats(root, {ref: {"rating": {"up": 12, "target": "t", "url": "u"}}})
    # A valid sidecar that declares nothing rated: parsed, it would clear the
    # rating, so only the size cap keeps the previous one.
    bomb = _gzip_bomb(b'{"schema_version": 1, "providers": {"rating": "github"}, "entries": {}', b"}")
    armed = threading.Event()

    class BombStats(SimpleHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 — http.server's own casing
            if not (armed.is_set() and self.path.endswith("stats.json")):
                super().do_GET()
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Encoding", "gzip")
            self.send_header("Content-Length", str(len(bomb)))
            self.end_headers()
            self.wfile.write(bomb)

    with serve(root, BombStats) as base:
        index_config(project_dir, base)
        runner = grim_at(project_dir)

        warm = search_envelope(runner)
        assert warm["items"][0]["rating"]["up"] == 12, "the warm build must see the rating"

        armed.set()
        envelope = search_envelope(runner)

    assert envelope["sources"][0]["ok"] is True, f"a sidecar never fails the source; got {envelope['sources']}"
    assert [r["repo"] for r in envelope["items"]] == [ref]
    assert envelope["items"][0]["rating"]["up"] == 12, (
        f"an over-cap sidecar is unobserved, not empty; got {envelope['items'][0]}"
    )


def test_an_index_listing_too_many_entries_degrades_the_source(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    """The byte cap alone admits millions of tiny entries: the count has its
    own bound, and one past it fails the source instead of truncating."""
    root = tmp_path / "index-dist-too-many"
    root.mkdir()
    minimal = [
        {"schema": 1, "name": f"p{i}", "kind": "skill", "ref": f"ghcr.io/acme/s/p{i}"}
        for i in range(ENTRY_LIMIT + 1)
    ]
    write_all_json(root, minimal)
    with serve(root, SimpleHTTPRequestHandler) as base:
        index_config(project_dir, base)
        envelope = search_envelope(grim_at(project_dir))

    assert envelope["items"] == [], "an over-limit index must not be truncated into a listing"
    source = envelope["sources"][0]
    assert source["ok"] is False, f"an over-limit index must fail the source; got {source}"
