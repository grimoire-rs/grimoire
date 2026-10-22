# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Time, memory and bytes written for a 20,000-entry index.

The synthetic index is the design point: about 41 MB of flat catalog, one
rating per entry. Every number comes from a single ``grim`` child, timed and
sized by a small launcher so the pytest process never counts toward it.
Budgets: a warm search and a quiet refresh each finish in under a second and
300 MB; a quiet refresh is one plain request for the root, answered in under
1 KB, rewrites no catalog file and writes under 1 KB beyond its sidecar; a
refresh that fetches a new catalog peaks at twice a warm search at most. Times
are the best of several runs, so a busy runner cannot fail a budget that the
code meets. Run with ``-m bench -s`` to print the measurements.
"""

from __future__ import annotations

import hashlib
import json
import random
import re
import subprocess
import sys
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from src.index_fixture import (
    IndexHandler,
    Request,
    index_config,
    package,
    serve,
    write_root_json,
)
from src.runner import GrimRunner

pytest.importorskip("resource", reason="peak memory needs the resource module")

pytestmark = pytest.mark.bench

ENTRIES = 20_000
MAX_SECONDS = 1.0
MAX_BYTES = 300 * 10**6
MAX_EXTRA_WRITTEN = 1024
MAX_QUIET_BYTES = 1024
REPEATS = 5
GRIM_TIMEOUT = 120

WARM = ("--format", "json", "search")
# One row of output keeps a refresh's own footprint from hiding behind the
# result it prints.
REFRESHING = ("--format", "json", "search", "--limit", "1")

_LAUNCHER = f"""\
import json, resource, subprocess, sys, time
start = time.perf_counter()
done = subprocess.run(
    sys.argv[1:], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
    stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
    timeout={GRIM_TIMEOUT},
)
seconds = time.perf_counter() - start
peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
print(json.dumps([done.returncode, seconds, peak, done.stderr]))
"""


@dataclass(frozen=True, slots=True)
class Run:
    seconds: float
    peak_bytes: int

    def __str__(self) -> str:
        return f"{self.seconds:.2f} s, {self.peak_bytes / 10**6:.0f} MB"


@dataclass(frozen=True, slots=True)
class Quiet:
    run: Run
    requests: list[Request]
    catalog_rewritten: bool
    extra_bytes_written: int


@dataclass(frozen=True, slots=True)
class Measured:
    warm: Run
    warm_limited: Run
    warm_requests: int
    quiet: list[Quiet]
    cold: Run
    cold_requests: list[Request]
    changed: Run
    changed_requests: list[Request]


def _generation(revision: int) -> tuple[bytes, bytes]:
    """The ``all.json`` and ``stats.json`` bytes of one catalog generation."""
    rng = random.Random(ENTRIES)
    vocabulary = [
        f"{rng.choice('abcdefghijklmnopqrstuvwxyz')}{rng.getrandbits(40):x}"
        for _ in range(400)
    ]

    def words(count: int) -> str:
        return " ".join(rng.choices(vocabulary, k=count))

    packages = []
    for i in range(ENTRIES):
        pkg = package(
            f"pkg-{i:05d}",
            ("skill", "rule", "agent", "bundle")[i % 4],
            f"ghcr.io/bench/pkg-{i:05d}",
            words(110) + (" revised" if revision and i == 7 else ""),
            keywords=[f"kw{i % 97}-{j}" for j in range(8)],
            summary=words(14),
        )
        pkg |= {
            "namespace": "github.com/bench",
            "tags": [f"tag{j}" for j in range(8)],
            "authors": [
                "Ada Lovelace <ada@example.test>",
                "Grace Hopper <grace@example.test>",
            ],
            "license": "Apache-2.0",
            "version": "1.2.3",
            "created": "2026-10-01T00:00:00Z",
            "url": f"https://example.test/pkg/{i}",
        }
        packages.append(pkg)
    stats = {
        "schema_version": 1,
        "generated_at": "2026-10-21T00:00:00Z",
        "providers": {"rating": "github"},
        "entries": {
            pkg["ref"]: {
                "rating": {
                    "up": i % 50,
                    "target": "D_kwDOAbCdEf",
                    "url": f"https://github.com/acme/index/discussions/{i}",
                }
            }
            for i, pkg in enumerate(packages)
        },
    }
    return (
        json.dumps(packages, separators=(",", ":")).encode(),
        json.dumps(stats, separators=(",", ":")).encode(),
    )


def _publish(root: Path, flat: bytes, stats: bytes) -> None:
    (root / "all.json").write_bytes(flat)
    (root / "stats.json").write_bytes(stats)
    write_root_json(root, flat, stats)


def measure(runner: GrimRunner, *args: str) -> Run:
    """One ``grim`` run: wall time and peak resident memory of the child."""
    out = subprocess.run(
        [sys.executable, "-I", "-c", _LAUNCHER, str(runner.binary), *args],
        env=runner.env,
        cwd=runner.cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=GRIM_TIMEOUT + 30,
        check=False,
    )
    assert out.returncode == 0, f"launcher failed: {out.stderr[-2000:]}"
    code, seconds, peak, stderr = json.loads(out.stdout)
    assert code == 0, f"grim {' '.join(args)} exited {code}; stderr: {stderr}"
    # ru_maxrss is kilobytes on Linux and bytes on macOS.
    return Run(seconds, peak if sys.platform == "darwin" else peak * 1024)


def _best(runs: list[Run]) -> Run:
    """The fastest time and the largest memory of repeated runs."""
    return Run(
        min(run.seconds for run in runs),
        max(run.peak_bytes for run in runs),
    )


def _catalog_files(home: Path) -> tuple[Path, Path]:
    (catalog,) = (
        p
        for p in (home / "catalog").glob("*.json")
        if not p.name.endswith(".sync.json")
    )
    return catalog, catalog.with_name(f"{catalog.stem}.sync.json")


def _age(home: Path) -> None:
    """Make the cached catalog two hours old while keeping its sidecar bound
    to it, so the next search refreshes it without being forced to."""
    catalog, sidecar = _catalog_files(home)
    data = catalog.read_bytes()
    stamp = re.search(rb'"built_at": "([^"]+)"', data)
    assert stamp, "the catalog file records when it was built"
    built_at = (datetime.now(timezone.utc) - timedelta(hours=2)).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    data = data.replace(stamp.group(0), f'"built_at": "{built_at}"'.encode(), 1)
    catalog.write_bytes(data)
    state = json.loads(sidecar.read_text())
    state.update(
        catalog_sha256=hashlib.sha256(data).hexdigest(),
        catalog_built_at=built_at,
        checked_at=built_at,
    )
    sidecar.write_text(json.dumps(state, indent=2))


def _files(home: Path) -> dict[Path, tuple[int, int, int]]:
    """Inode, modification time and size of every file under ``home``."""
    return {
        path: (stat.st_ino, stat.st_mtime_ns, stat.st_size)
        for path in home.rglob("*")
        if path.is_file()
        for stat in [path.stat()]
    }


def _quiet_refresh(runner: GrimRunner, handler: type[IndexHandler]) -> Quiet:
    _age(runner.grim_home)
    catalog, sidecar = _catalog_files(runner.grim_home)
    before, requested = _files(runner.grim_home), len(handler.log)
    run = measure(runner, *REFRESHING)
    after = _files(runner.grim_home)
    changed = {path for path, seen in after.items() if before.get(path) != seen}
    return Quiet(
        run=run,
        requests=handler.log[requested:],
        catalog_rewritten=catalog in changed,
        extra_bytes_written=sum(after[path][2] for path in changed - {sidecar}),
    )


@pytest.fixture(scope="module")
def measured(
    tmp_path_factory: pytest.TempPathFactory, grim_binary: Path
) -> Iterator[Measured]:
    work = tmp_path_factory.mktemp("index-bench")
    dist, project, home = work / "dist", work / "project", work / "grim-home"
    for directory in (dist, project, home):
        directory.mkdir()
    _publish(dist, *_generation(0))
    handler = IndexHandler.new()
    with serve(dist, handler) as base:
        index_config(project, base)
        runner = GrimRunner(grim_binary, home, cwd=project)

        def refreshed(*args: str) -> tuple[Run, list[Request]]:
            requested = len(handler.log)
            return measure(runner, *args), handler.log[requested:]

        cold, cold_requests = refreshed(*REFRESHING, "--refresh")
        requested = len(handler.log)
        warm = _best([measure(runner, *WARM) for _ in range(REPEATS)])
        warm_limited = _best([measure(runner, *REFRESHING) for _ in range(REPEATS)])
        warm_requests = len(handler.log) - requested

        quiet = [_quiet_refresh(runner, handler) for _ in range(REPEATS)]

        _publish(dist, *_generation(1))
        changed, changed_requests = refreshed(*REFRESHING, "--refresh")
    result = Measured(
        warm,
        warm_limited,
        warm_requests,
        quiet,
        cold,
        cold_requests,
        changed,
        changed_requests,
    )
    quiet_run = _best([q.run for q in quiet])
    quiet_sent = max(sum(r.length for r in q.requests) for q in quiet)
    quiet_written = max(q.extra_bytes_written for q in quiet)
    print(
        f"\nbench {ENTRIES} entries (best of {REPEATS}): warm search {warm}; "
        f"warm search, one row {warm_limited}; quiet refresh {quiet_run}, "
        f"{quiet_sent} B received, {quiet_written} B written beyond the sidecar; "
        f"cold refresh {cold} ({cold.peak_bytes / warm_limited.peak_bytes:.2f}x warm "
        f"peak); changed-catalog refresh {changed} "
        f"({changed.peak_bytes / warm_limited.peak_bytes:.2f}x warm peak)"
    )
    yield result


@pytest.fixture()
def quiet(measured: Measured) -> list[Quiet]:
    """The quiet runs, each proven to have asked for the root and nothing
    else: a run that skipped the probe or fetched more says nothing about the
    budgets it is measured against."""
    for run in measured.quiet:
        assert [r.path for r in run.requests] == ["/c/root.json"], run.requests
    return measured.quiet


def test_a_warm_search_stays_within_one_second_and_300_mb(measured: Measured) -> None:
    assert measured.warm.seconds < MAX_SECONDS, f"warm search: {measured.warm}"
    assert measured.warm.peak_bytes < MAX_BYTES, f"warm search: {measured.warm}"


def test_a_warm_search_makes_no_request(measured: Measured) -> None:
    assert measured.warm_requests == 0, "a fresh cache needs no network"


def test_a_quiet_refresh_stays_within_one_second_and_300_mb(quiet: list[Quiet]) -> None:
    best = _best([q.run for q in quiet])
    assert best.seconds < MAX_SECONDS, f"quiet refresh: {best}"
    assert best.peak_bytes < MAX_BYTES, f"quiet refresh: {best}"


def test_a_quiet_refresh_is_one_plain_root_get_under_a_kilobyte(
    quiet: list[Quiet],
) -> None:
    for run in quiet:
        (request,) = run.requests
        assert request.method == "GET", request
        assert not {"if-none-match", "if-modified-since"} & request.headers.keys(), (
            f"the root probe is never conditional: {request.headers}"
        )
        assert 0 < request.length <= MAX_QUIET_BYTES, (
            f"the root answer is {request.length} bytes"
        )


def test_a_quiet_refresh_rewrites_no_catalog_file_and_writes_under_a_kilobyte_more(
    quiet: list[Quiet],
) -> None:
    assert not any(q.catalog_rewritten for q in quiet), (
        "unchanged stats must leave the catalog file alone"
    )
    written = max(q.extra_bytes_written for q in quiet)
    assert written <= MAX_EXTRA_WRITTEN, f"{written} bytes written beyond the sidecar"


def test_a_refresh_that_fetches_a_catalog_peaks_within_twice_a_warm_search(
    measured: Measured,
) -> None:
    for name, requests in (
        ("cold", measured.cold_requests),
        ("changed", measured.changed_requests),
    ):
        assert any(r.path.startswith("/c/f/") for r in requests), (
            f"the {name} refresh fetched no catalog blob: {requests}"
        )
    # Both sides print one row, so the comparison is a refresh against the
    # same search with a fresh cache.
    limit = 2 * measured.warm_limited.peak_bytes
    assert measured.cold.peak_bytes <= limit, (
        f"cold refresh {measured.cold}; warm {measured.warm_limited}"
    )
    assert measured.changed.peak_bytes <= limit, (
        f"changed refresh {measured.changed}; warm {measured.warm_limited}"
    )


def test_a_quiet_refresh_peaks_no_higher_than_a_cold_refresh(
    measured: Measured, quiet: list[Quiet]
) -> None:
    """A quiet refresh holds the cached catalog once; a cold one holds the
    fetched catalog once, so neither may peak above the other."""
    best = _best([q.run for q in quiet])
    assert best.peak_bytes <= measured.cold.peak_bytes, (
        f"quiet refresh {best}; cold refresh {measured.cold}"
    )
