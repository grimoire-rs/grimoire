# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Indexes that publish a root: the flat blob it names is the browse listing.

A root is adopted only after the blob it names verifies. A blob that does
not verify sends a legacy-backed index to ``all.json`` and fails a root-only
index; a blob the host cannot serve fails the refresh and keeps the cache.
A stats blob is fetched once per digest and never fails the refresh.

The root is fetched with a plain GET on every refresh past the TTL, whatever
validators the host serves: an unchanged root leaves the cache and its blobs
alone, and no conditional header is ever sent.
"""

from __future__ import annotations

import gzip
import hashlib
import json
from collections.abc import Iterator
from datetime import datetime
from pathlib import Path

import pytest

from src.index_fixture import (
    ConformanceKind,
    Fault,
    KindedIndex,
    Request,
    index_config,
    package,
    search_envelope,
    serve_kind,
    write_all_json,
    write_stats,
)

ONE = "ghcr.io/acme/skills/one"
TWO = "ghcr.io/acme/skills/two"


def _served(tmp_path: Path, kind: ConformanceKind) -> Iterator[KindedIndex]:
    root = tmp_path / "index-dist"
    root.mkdir()
    with serve_kind(root, kind) as index:
        yield index


@pytest.fixture(params=["c-plus-legacy", "c-only"])
def anchored_index(
    request: pytest.FixtureRequest, tmp_path: Path
) -> Iterator[KindedIndex]:
    yield from _served(tmp_path, request.param)


@pytest.fixture()
def legacy_backed_index(tmp_path: Path) -> Iterator[KindedIndex]:
    yield from _served(tmp_path, "c-plus-legacy")


@pytest.fixture()
def root_only_index(tmp_path: Path) -> Iterator[KindedIndex]:
    yield from _served(tmp_path, "c-only")


def _blob_path(index: KindedIndex) -> str:
    return next(p for p in index.document_paths("flat") if p.startswith("/c/f/"))


def _requested(index: KindedIndex) -> list[str]:
    return [r.path for r in index.log]


def test_a_root_is_read_through_its_blob_and_never_through_all_json(
    grim_at, project_dir: Path, anchored_index: KindedIndex
) -> None:
    write_all_json(
        anchored_index.root,
        [package("one", "skill", ONE, "First"), package("two", "skill", TWO, "Second")],
    )
    index_config(project_dir, anchored_index.base)

    envelope = search_envelope(grim_at(project_dir))

    assert sorted(r["repo"] for r in envelope["items"]) == [ONE, TWO]
    assert envelope["sources"][0]["ok"] is True
    requested = _requested(anchored_index)
    assert requested[:2] == ["/c/root.json", _blob_path(anchored_index)], requested
    assert "/all.json" not in requested, requested


def test_a_blob_that_is_not_the_one_the_root_names_falls_back_to_all_json(
    grim_at, project_dir: Path, legacy_backed_index: KindedIndex
) -> None:
    write_all_json(legacy_backed_index.root, [package("one", "skill", ONE, "First")])
    other = gzip.compress(b"[]", mtime=0)
    legacy_backed_index.handler.faults[_blob_path(legacy_backed_index)] = Fault(
        body=other
    )
    index_config(project_dir, legacy_backed_index.base)

    envelope = search_envelope(grim_at(project_dir))

    assert [r["repo"] for r in envelope["items"]] == [ONE], (
        "the listing must come from all.json when the blob does not verify"
    )
    assert envelope["sources"][0]["ok"] is True
    requested = _requested(legacy_backed_index)
    assert requested[:3] == [
        "/c/root.json",
        _blob_path(legacy_backed_index),
        "/all.json",
    ], requested


def test_a_root_only_index_with_a_bad_blob_degrades_the_source(
    grim_at, grim_home: Path, project_dir: Path, root_only_index: KindedIndex
) -> None:
    write_all_json(root_only_index.root, [package("one", "skill", ONE, "First")])
    root_only_index.handler.faults[_blob_path(root_only_index)] = Fault(status=404)
    index_config(project_dir, root_only_index.base)

    envelope = search_envelope(grim_at(project_dir))

    assert envelope["items"] == []
    assert envelope["sources"][0]["ok"] is False, envelope["sources"]
    catalog = grim_home / "catalog"
    cached = [p for p in catalog.glob("*.json") if not p.name.endswith(".sync.json")]
    assert cached == [], "a refresh that never verified a blob must cache no catalog"
    (sidecar,) = catalog.glob("*.sync.json")
    state = json.loads(sidecar.read_text())
    assert state["root"] is None, "a blob that failed adopts no root"
    assert state["failures"] == 1


def _publish_two(index: KindedIndex) -> None:
    write_all_json(
        index.root,
        [package("one", "skill", ONE, "First"), package("two", "skill", TWO, "Second")],
    )


def test_a_blob_the_host_cannot_serve_keeps_the_warm_cache(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    write_all_json(anchored_index.root, [package("one", "skill", ONE, "First")])
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    warm = search_envelope(runner)
    assert [r["repo"] for r in warm["items"]] == [ONE]
    cached = _cache_file(grim_home).read_bytes()
    adopted = _sidecar(grim_home)["root"]

    _publish_two(anchored_index)
    anchored_index.fault("flat", Fault(status=503))
    seen = len(anchored_index.log)
    envelope = search_envelope(runner)

    assert [r["repo"] for r in envelope["items"]] == [ONE], (
        "an unreachable blob must leave the cached listing in place"
    )
    assert envelope["sources"][0]["ok"] is False, envelope["sources"]
    assert "/all.json" not in _requested(anchored_index)[seen:], (
        "a blob the host cannot serve must not send the refresh to all.json"
    )
    assert _cache_file(grim_home).read_bytes() == cached
    state = _sidecar(grim_home)
    assert state["root"] == adopted, "the failed root must not be adopted"
    assert state["failures"] == 1


def test_a_blob_that_recovers_is_adopted_by_the_next_refresh(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    write_all_json(anchored_index.root, [package("one", "skill", ONE, "First")])
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    adopted = _sidecar(grim_home)["root"]
    _publish_two(anchored_index)
    anchored_index.fault("flat", Fault(status=503))
    assert search_envelope(runner)["sources"][0]["ok"] is False
    anchored_index.fault("flat", None)

    envelope = search_envelope(runner)

    assert envelope["sources"][0]["ok"] is True, envelope["sources"]
    assert {r["repo"] for r in envelope["items"]} == {ONE, TWO}
    state = _sidecar(grim_home)
    assert state["root"] != adopted
    assert state["root"]["sha256"] == _digest(anchored_index.root / "c" / "root.json")
    assert state["failures"] == 0


OLD = "2020-01-01T00:00:00Z"


def _publish(index: KindedIndex, up: int, **extra: object) -> None:
    write_all_json(index.root, [package("one", "skill", ONE, "First")])
    write_stats(
        index.root,
        {ONE: {"rating": {"up": up, "target": "t", "url": "https://f/1"}, **extra}},
    )


def _stats_blob(index: KindedIndex) -> str:
    return next(p for p in index.document_paths("stats") if p.startswith("/c/f/"))


def _cache_file(grim_home: Path) -> Path:
    (cache,) = (
        p
        for p in (grim_home / "catalog").glob("*.json")
        if not p.name.endswith(".sync.json")
    )
    return cache


def _sidecar(grim_home: Path) -> dict:
    return json.loads(_cache_file(grim_home).with_suffix(".sync.json").read_text())


def _age_cache(grim_home: Path) -> None:
    """Back-date the cache and keep the sidecar bound to it.

    The next unforced run revalidates against the index instead of serving
    the cache, and the sidecar still describes the bytes on disk, so the
    revalidation is quiet rather than a rebuild.
    """
    cache = _cache_file(grim_home)
    doc = json.loads(cache.read_text())
    doc["built_at"] = OLD
    data = json.dumps(doc).encode()
    cache.write_bytes(data)
    sidecar = cache.with_suffix(".sync.json")
    state = json.loads(sidecar.read_text())
    state.update(
        catalog_sha256=hashlib.sha256(data).hexdigest(),
        catalog_built_at=OLD,
        checked_at=OLD,
    )
    sidecar.write_text(json.dumps(state))


def _browse(runner, *flags: str) -> dict:
    """An unforced ``grim search``: the refresh happens only if the cache is stale."""
    result = runner.run("--format", "json", *flags, "search", check=False)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def _row(envelope: dict) -> dict:
    return next(r for r in envelope["items"] if r["repo"] == ONE)


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_a_stats_digest_is_fetched_once_and_then_left_alone(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    blob = _stats_blob(anchored_index)

    warm = search_envelope(runner)

    assert _row(warm)["rating"]["up"] == 7
    assert _requested(anchored_index).count(blob) == 1
    _age_cache(grim_home)
    seen = len(anchored_index.log)

    envelope = _browse(runner)

    assert _row(envelope)["rating"]["up"] == 7
    assert _requested(anchored_index)[seen:] == ["/c/root.json"], (
        "a digest the cache already carries must cost no stats request"
    )


def test_a_quiet_refresh_applies_new_ratings_and_counts(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    counts = {"total": 50, "as_of": "2026-10-01T00:00:00Z", "versions": {"1.0.0": 40}}
    _publish(anchored_index, up=9, downloads=counts)
    _age_cache(grim_home)
    seen = len(anchored_index.log)

    row = _row(_browse(runner))

    assert (row["rating"]["up"], row["downloads"]["total"]) == (9, 50)
    assert _requested(anchored_index)[seen:] == [
        "/c/root.json",
        _stats_blob(anchored_index),
    ], "the listing is unchanged, so only the new stats blob is fetched"
    offline = _row(_browse(runner, "--offline"))
    assert (offline["rating"]["up"], offline["downloads"]["total"]) == (9, 50), (
        "the quiet refresh must persist what it joined"
    )
    assert _sidecar(grim_home)["stats"]["applied"] == _digest(
        anchored_index.root / "stats.json"
    )


def test_a_stats_blob_that_changes_nothing_leaves_the_catalog_file_alone(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    stats = anchored_index.root / "stats.json"
    doc = json.loads(stats.read_text())
    doc["generated_at"] = "2026-09-01T00:00:00Z"
    stats.write_text(json.dumps(doc))
    _age_cache(grim_home)
    before = _cache_file(grim_home).read_bytes()

    envelope = _browse(runner)

    assert _row(envelope)["rating"]["up"] == 7
    assert _cache_file(grim_home).read_bytes() == before, (
        "stats that join to the same values must not rewrite the cache"
    )
    assert _sidecar(grim_home)["stats"]["applied"] == _digest(stats)


def test_a_root_that_stops_naming_stats_keeps_the_ratings(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    applied = _sidecar(grim_home)["stats"]["applied"]
    (anchored_index.root / "stats.json").unlink()
    _age_cache(grim_home)
    seen = len(anchored_index.log)

    envelope = _browse(runner)

    assert _row(envelope)["rating"]["up"] == 7, (
        "a withdrawn stats ref is not an erasure"
    )
    assert _requested(anchored_index)[seen:] == ["/c/root.json"]
    stats = _sidecar(grim_home)["stats"]
    assert (stats["advertised"], stats["applied"]) == (None, applied)


def test_a_rejected_stats_blob_is_not_asked_for_again_until_forced(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    _publish(anchored_index, up=9)
    blob = _stats_blob(anchored_index)
    anchored_index.fault("stats", Fault(body=gzip.compress(b"{}", mtime=0)))

    for _ in range(2):
        _age_cache(grim_home)
        envelope = _browse(runner)
        assert _row(envelope)["rating"]["up"] == 7, "the cached rating stays"
        assert envelope["sources"][0]["ok"] is True

    assert _requested(anchored_index).count(blob) == 1, (
        "a digest that failed verification must not be fetched on every refresh"
    )
    assert _sidecar(grim_home)["stats"]["rejected"] == _digest(
        anchored_index.root / "stats.json"
    )

    search_envelope(runner)
    assert _requested(anchored_index).count(blob) == 2, "--refresh asks again"
    anchored_index.fault("stats", None)
    healed = search_envelope(runner)

    assert _row(healed)["rating"]["up"] == 9
    stats = _sidecar(grim_home)["stats"]
    assert stats["rejected"] is None, "a verified blob clears the rejection"
    assert stats["applied"] == _digest(anchored_index.root / "stats.json")


def test_a_stats_blob_the_host_cannot_serve_is_asked_for_again(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    applied = _sidecar(grim_home)["stats"]["applied"]
    _publish(anchored_index, up=9)
    blob = _stats_blob(anchored_index)
    anchored_index.fault("stats", Fault(status=503))

    for _ in range(2):
        _age_cache(grim_home)
        envelope = _browse(runner)
        assert _row(envelope)["rating"]["up"] == 7
        assert envelope["sources"][0]["ok"] is True, (
            "an unreachable stats blob must not fail the refresh"
        )

    assert _requested(anchored_index).count(blob) == 2, "an outage is not a verdict"
    stats = _sidecar(grim_home)["stats"]
    assert (stats["applied"], stats["rejected"]) == (applied, None)


CONDITIONAL = ("if-none-match", "if-modified-since")


def _conditional(request: Request) -> dict[str, str]:
    return {k: v for k, v in request.headers.items() if k in CONDITIONAL}


def _root_requests(index: KindedIndex, since: int) -> list[Request]:
    return [r for r in index.log[since:] if r.path == "/c/root.json"]


def test_an_unchanged_root_is_fetched_plainly_and_nothing_else_is_fetched(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    root = _sidecar(grim_home)["root"]
    assert set(root) == {"sha256", "generation"}, "a served validator is not stored"
    _age_cache(grim_home)
    before = _cache_file(grim_home).read_bytes()
    seen = len(anchored_index.log)

    envelope = _browse(runner)

    assert envelope["sources"][0]["ok"] is True
    assert [(r.path, _conditional(r)) for r in anchored_index.log[seen:]] == [
        ("/c/root.json", {})
    ], "an unchanged root ends the refresh: no blob, no all.json"
    assert _cache_file(grim_home).read_bytes() == before
    state = _sidecar(grim_home)
    assert state["checked_at"] != OLD, "an unchanged root must restamp the sidecar"
    assert (state["root"], state["fallbacks"]) == (root, 0)


def test_refresh_asks_for_the_root_once_and_without_a_validator(
    grim_at, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    seen = len(anchored_index.log)

    search_envelope(runner)

    assert [_conditional(r) for r in _root_requests(anchored_index, seen)] == [{}], (
        "--refresh must see what the host serves now, with one plain GET"
    )


def test_a_changed_root_is_adopted_and_stored_without_validators(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    old = _sidecar(grim_home)["root"]
    write_all_json(
        anchored_index.root,
        [package("one", "skill", ONE, "First"), package("two", "skill", TWO, "Second")],
    )
    _age_cache(grim_home)
    seen = len(anchored_index.log)

    envelope = _browse(runner)

    assert {r["repo"] for r in envelope["items"]} == {ONE, TWO}
    assert [_conditional(r) for r in _root_requests(anchored_index, seen)] == [{}]
    current = _sidecar(grim_home)["root"]
    assert current["sha256"] == _digest(anchored_index.root / "c" / "root.json")
    assert current != old
    assert set(current) == {"sha256", "generation"}


def test_a_root_whose_blob_fails_is_not_adopted(
    grim_at, grim_home: Path, project_dir: Path, legacy_backed_index: KindedIndex
) -> None:
    index = legacy_backed_index
    _publish(index, up=7)
    index_config(project_dir, index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    before = _sidecar(grim_home)
    assert before["root"] is not None and before["fallbacks"] == 0
    write_all_json(
        index.root,
        [package("one", "skill", ONE, "First"), package("two", "skill", TWO, "Second")],
    )
    index.handler.faults[_blob_path(index)] = Fault(status=404)
    _age_cache(grim_home)

    envelope = _browse(runner)

    assert envelope["sources"][0]["ok"] is True
    assert {r["repo"] for r in envelope["items"]} == {ONE, TWO}, (
        "the listing must come from all.json, the fallback"
    )
    state = _sidecar(grim_home)
    assert state["root"] is None, "a root whose blob failed is not adopted"
    assert (state["fallbacks"], state["failures"]) == (1, 0)
    assert state["flat"]["sha256"] == _digest(index.root / "all.json")


def test_stats_the_root_advertises_are_fetched_behind_an_unchanged_root(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    index_config(project_dir, anchored_index.base)
    runner = grim_at(project_dir)
    search_envelope(runner)
    _publish(anchored_index, up=9)
    blob = _stats_blob(anchored_index)
    anchored_index.fault("stats", Fault(status=503))
    _age_cache(grim_home)
    assert _row(_browse(runner))["rating"]["up"] == 7
    anchored_index.fault("stats", None)
    _age_cache(grim_home)
    seen = len(anchored_index.log)

    envelope = _browse(runner)

    assert _row(envelope)["rating"]["up"] == 9
    assert [(r.path, _conditional(r)) for r in anchored_index.log[seen:]] == [
        ("/c/root.json", {}),
        (blob, {}),
    ], "the root is unchanged, so the ahead stats must still come through"
    offline = _row(_browse(runner, "--offline"))
    assert offline["rating"]["up"] == 9, "the stats fetched behind it must be persisted"


def _deferral(sidecar: dict) -> float:
    """Seconds the sidecar defers the next refresh past the failure that set it."""

    def stamp(key: str) -> datetime:
        return datetime.fromisoformat(sidecar[key])

    return (stamp("retry_after") - stamp("last_failure_at")).total_seconds()


def test_a_throttled_root_warns_once_serves_the_cache_and_defers_the_next_run(
    grim_at, grim_home: Path, project_dir: Path, anchored_index: KindedIndex
) -> None:
    _publish(anchored_index, up=7)
    secret = "s3cret"
    index_config(
        project_dir, anchored_index.base.replace("http://", f"http://user:{secret}@")
    )
    runner = grim_at(project_dir)
    search_envelope(runner)
    anchored_index.handler.faults["/c/root.json"] = Fault(
        status=429, headers={"Retry-After": "600"}
    )
    seen = len(anchored_index.log)

    result = runner.run("--format", "json", "search", "--refresh", check=False)

    assert result.returncode == 0, f"a throttled index never fails the run: {result}"
    envelope = json.loads(result.stdout)
    assert [r["repo"] for r in envelope["items"]] == [ONE], "the cache must be served"
    assert envelope["sources"][0]["ok"] is False
    assert [r.path for r in anchored_index.log[seen:]] == ["/c/root.json"], (
        "a throttled root ends the refresh: no blob, no all.json"
    )
    throttled = [ln for ln in result.stderr.splitlines() if "rate limiting" in ln]
    assert len(throttled) == 1, result.stderr
    assert anchored_index.base.removeprefix("http://") in throttled[0], throttled[0]
    state = _sidecar(grim_home)
    for leaked in (result.stderr, json.dumps(state)):
        assert secret not in leaked, "credentials must never reach a log or the sidecar"
    assert state["failures"] == 1
    assert _deferral(state) >= 600, "the Retry-After must set the backoff"

    _age_cache(grim_home)
    seen = len(anchored_index.log)
    deferred = _browse(runner)

    assert len(anchored_index.log) == seen, "a run inside the wait sends no request"
    assert [r["repo"] for r in deferred["items"]] == [ONE]
    assert "refresh deferred until" in deferred["sources"][0]["error"]
