# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A failed index refresh serves the cached catalog instead of nothing.

The source still reports ``ok: false`` with its ``error``, and stderr says
the rows are stale. With no cache to fall back on the browse is empty. A
failure also starts a backoff: until it runs out an unforced run serves the
cache without sending a request, while ``--refresh`` and ``--offline`` ignore it.
A sidecar that confirms an old catalog current keeps it from being refreshed.
"""

from __future__ import annotations

import hashlib
import json
from datetime import UTC, datetime
from pathlib import Path

from src.index_fixture import (
    KindedIndex,
    index_config,
    package,
    search_envelope,
    write_all_json,
)

STALE_NOTICE = "serving the cached catalog"


def _refresh(runner):
    return runner.run("--format", "json", "search", "--refresh", check=False)


def _publish(index: KindedIndex) -> None:
    write_all_json(
        index.root,
        [
            package("one", "skill", "ghcr.io/acme/skills/one", "First"),
            package("two", "skill", "ghcr.io/acme/skills/two", "Second"),
        ],
    )


def test_a_failed_refresh_serves_the_cached_rows(
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    _publish(kind_server)
    index_config(project_dir, kind_server.base)
    runner = grim_at(project_dir)
    assert len(search_envelope(runner)["items"]) == 2, "the cache must warm first"

    kind_server.outage(True)
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
    grim_at, project_dir: Path, kind_server: KindedIndex
) -> None:
    _publish(kind_server)
    kind_server.outage(True)
    index_config(project_dir, kind_server.base)
    result = _refresh(grim_at(project_dir))

    assert result.returncode == 0, (
        f"a degraded source never fails the run: {result.stderr}"
    )
    envelope = json.loads(result.stdout)
    assert envelope["items"] == []
    assert envelope["sources"][0]["ok"] is False
    assert STALE_NOTICE not in result.stderr, "there is no cache to serve"


def _search(runner, *flags: str):
    return runner.run("--format", "json", *flags, "search", check=False)


def _age_cache(grim_home: Path) -> None:
    """Back-date the cache so the next unforced run has to refresh it."""
    (cache,) = (
        p
        for p in (grim_home / "catalog").glob("*.json")
        if not p.name.endswith(".sync.json")
    )
    doc = json.loads(cache.read_text())
    doc["built_at"] = "2020-01-01T00:00:00Z"
    cache.write_text(json.dumps(doc))


def _sidecar(grim_home: Path) -> dict:
    (sidecar,) = (grim_home / "catalog").glob("*.sync.json")
    return json.loads(sidecar.read_text())


def test_a_failed_refresh_defers_the_next_unforced_run(
    grim_at, grim_home: Path, project_dir: Path, kind_server: KindedIndex
) -> None:
    _publish(kind_server)
    index_config(project_dir, kind_server.base)
    runner = grim_at(project_dir)
    assert len(search_envelope(runner)["items"]) == 2, "the cache must warm first"
    _age_cache(grim_home)
    kind_server.outage(True)

    first = _search(runner)
    requests = len(kind_server.log)
    second = _search(runner)

    assert len(kind_server.log) == requests, "a run inside the wait sends no request"

    failed = json.loads(first.stdout)["sources"][0]
    assert failed["ok"] is False and failed["error"], (
        "the first run reports the failure"
    )
    assert "refresh deferred" not in failed["error"]
    envelope = json.loads(second.stdout)
    assert second.returncode == 0, second.stderr
    assert len(envelope["items"]) == 2, "the cached rows are still served"
    deferred = envelope["sources"][0]
    assert deferred["ok"] is False
    assert "refresh deferred until" in deferred["error"], deferred
    assert _sidecar(grim_home)["failures"] == 1, "a deferral is not another failure"


def test_refresh_and_offline_ignore_the_backoff_and_a_success_clears_it(
    grim_at, grim_home: Path, project_dir: Path, kind_server: KindedIndex
) -> None:
    _publish(kind_server)
    index_config(project_dir, kind_server.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    _age_cache(grim_home)
    kind_server.outage(True)
    _search(runner)
    requests = len(kind_server.log)

    offline = json.loads(_search(runner, "--offline").stdout)
    assert len(kind_server.log) == requests, "offline never reaches the network"
    assert len(offline["items"]) == 2
    assert offline["sources"][0]["error"] is None, "offline never reads the backoff"

    forced = json.loads(_refresh(runner).stdout)
    assert len(kind_server.log) > requests, "--refresh goes out inside the wait"
    assert "refresh deferred" not in forced["sources"][0]["error"]
    assert _sidecar(grim_home)["failures"] == 2

    kind_server.outage(False)
    healed = json.loads(_refresh(runner).stdout)

    assert healed["sources"][0]["ok"] is True
    state = _sidecar(grim_home)
    assert state["failures"] == 0
    assert state["last_error"] is None and state["retry_after"] is None


def _confirm_cache(grim_home: Path) -> None:
    """Write the sidecar a quiet refresh leaves: the aged catalog, current now."""
    (cache,) = (
        p
        for p in (grim_home / "catalog").glob("*.json")
        if not p.name.endswith(".sync.json")
    )
    doc = json.loads(cache.read_text())
    cache.with_suffix(".sync.json").write_text(
        json.dumps(
            {
                "version": 1,
                "locator_sha256": hashlib.sha256(doc["registry"].encode()).hexdigest(),
                "catalog_built_at": doc["built_at"],
                "checked_at": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "fallbacks": 0,
                "failures": 0,
            }
        )
    )


def test_a_confirmed_catalog_is_not_refreshed_inside_the_window(
    grim_at, grim_home: Path, project_dir: Path, kind_server: KindedIndex
) -> None:
    _publish(kind_server)
    index_config(project_dir, kind_server.base)
    runner = grim_at(project_dir)
    search_envelope(runner)

    _age_cache(grim_home)
    requests = len(kind_server.log)
    _search(runner)
    assert len(kind_server.log) > requests, (
        "an old catalog with nothing confirming it refreshes"
    )

    _age_cache(grim_home)
    _confirm_cache(grim_home)
    requests = len(kind_server.log)
    result = _search(runner)

    assert len(kind_server.log) == requests, "a confirmed catalog sends no request"

    assert result.returncode == 0, result.stderr
    envelope = json.loads(result.stdout)
    assert len(envelope["items"]) == 2
    assert envelope["sources"][0]["ok"] is True
