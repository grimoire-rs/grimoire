# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A failed index refresh serves the cached catalog instead of nothing.

The source still reports ``ok: false`` with its ``error``, and stderr says
the rows are stale. With no cache to fall back on the browse is empty.
"""

from __future__ import annotations

import json
import threading
from http.server import SimpleHTTPRequestHandler
from pathlib import Path

from src.index_fixture import (
    index_config,
    package,
    search_envelope,
    serve,
    write_all_json,
)

STALE_NOTICE = "serving the cached catalog"


def _flaky_handler(failing: threading.Event) -> type[SimpleHTTPRequestHandler]:
    class FlakyAllJson(SimpleHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 — http.server's own casing
            if self.path.endswith("all.json") and failing.is_set():
                self.send_error(500)
                return
            super().do_GET()

    return FlakyAllJson


def _refresh(runner):
    return runner.run("--format", "json", "search", "--refresh", check=False)


def _index(tmp_path: Path) -> Path:
    root = tmp_path / "index-dist-stale"
    root.mkdir()
    write_all_json(
        root,
        [
            package("one", "skill", "ghcr.io/acme/skills/one", "First"),
            package("two", "skill", "ghcr.io/acme/skills/two", "Second"),
        ],
    )
    return root


def test_a_failed_refresh_serves_the_cached_rows(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    failing = threading.Event()
    with serve(_index(tmp_path), _flaky_handler(failing)) as base:
        index_config(project_dir, base)
        runner = grim_at(project_dir)
        assert len(search_envelope(runner)["items"]) == 2, "the cache must warm first"

        failing.set()
        result = _refresh(runner)

    assert result.returncode == 0, (
        f"a degraded source never fails the run: {result.stderr}"
    )
    envelope = json.loads(result.stdout)
    assert sorted(r["repo"] for r in envelope["items"]) == [
        "ghcr.io/acme/skills/one",
        "ghcr.io/acme/skills/two",
    ], "the warm rows must still be served"
    assert envelope["total"] == 2, "stale rows count in total"
    assert envelope["truncated"] is False
    source = envelope["sources"][0]
    assert source["ok"] is False, "serving stale rows is not a healthy refresh"
    assert source["error"], "the failure stays visible on the source"
    assert STALE_NOTICE in result.stderr, (
        f"stderr must say the rows are stale: {result.stderr}"
    )


def test_a_failed_refresh_with_no_cache_is_empty(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    failing = threading.Event()
    failing.set()
    with serve(_index(tmp_path), _flaky_handler(failing)) as base:
        index_config(project_dir, base)
        result = _refresh(grim_at(project_dir))

    assert result.returncode == 0, (
        f"a degraded source never fails the run: {result.stderr}"
    )
    envelope = json.loads(result.stdout)
    assert envelope["items"] == []
    assert envelope["sources"][0]["ok"] is False
    assert STALE_NOTICE not in result.stderr, "there is no cache to serve"
