# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim status`` fetches what the local store lacks, within fixed bounds.

Online, a node the store cannot prove is fetched from its registry through
one bounded reader for the whole run: 10 s a call, 30 s a run, 64 manifest
and 64 layer fetches. A host that fails is skipped by every later run for 10
minutes (the miss memo). Offline, the store alone is read. Concurrent runs
never fail and never lose or resurrect a record another command wrote.
``adr_output_ownership_without_state.md`` validation cases 25, 26, 28 and 32,
and ``research_install_record_cache_test_matrix.md`` S55, S56 and S62.
"""

from __future__ import annotations

import json
import os
import shutil
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest
import tomllib

from src.helpers import held_flock, make_artifact, write_config
from src.oci_proxy import OciProxy
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.scenarios import (
    Team,
    allow_insecure,
    assert_unchanged,
    cold_runner,
    gone_pin,
    move_tag,
    state_snapshot,
)

NAMES = ("moving", "style", "steady")
ADOPTED = "adopted {} artifacts into the install record; {} left unproven"

posix_only = pytest.mark.skipif(
    os.name == "nt", reason="the state-file lock is a flock"
)


def _moving(version: str) -> dict[str, str]:
    return {
        "moving/SKILL.md": f"# moving {version}\n",
        "moving/ref.md": f"ref {version}\n",
    }


def _style(version: str) -> dict[str, str]:
    return {"style.md": f"---\npaths: ['**/*.rs']\n---\n# style {version}\n"}


def _status(runner: GrimRunner, *flags: str, timeout: float = 120):
    """``grim --format json status``; return ``(exit, parsed stdout, stderr)``."""
    result = runner.run(*flags, "status", format="json", check=False, timeout=timeout)
    try:
        doc = json.loads(result.stdout)
    except json.JSONDecodeError:
        doc = {}
    return result.returncode, doc, result.stderr


def _states(doc: dict) -> dict[str, str]:
    return {row["name"]: row["state"] for row in doc.get("items", [])}


def _keys(doc: dict) -> tuple[list[str], list[str]]:
    return sorted(doc), sorted({key for row in doc["items"] for key in row})


def _requests(proxy: OciProxy) -> int:
    return sum(proxy.counts.values())


def _reads(sent: dict[str, int], kind: str) -> int:
    return sum(n for path, n in sent.items() if kind in path)


def _records(work: Path) -> list[dict]:
    doc = json.loads((work / ".grimoire" / "state.json").read_text(encoding="utf-8"))
    return sorted(doc["records"], key=lambda r: (r["kind"], r["name"]))


def _untagged(pinned: str) -> str:
    """``pinned`` without its advisory tag: two records naming one digest are one pin."""
    reference, digest = pinned.rsplit("@", 1)
    head, leaf = reference.rsplit("/", 1)
    return f"{head}/{leaf.split(':', 1)[0]}@{digest}"


def _comparable(work: Path) -> list[dict]:
    return [{**r, "pinned": _untagged(r["pinned"])} for r in _records(work)]


def _record_pins(work: Path) -> dict[str, str]:
    return {r["name"]: r["pinned"].rsplit("@", 1)[1] for r in _records(work)}


def _lock_pins(work: Path) -> dict[str, str]:
    doc = tomllib.loads((work / "grimoire.lock").read_text(encoding="utf-8"))
    return {
        row["name"]: row["pinned"].rsplit("@", 1)[1]
        for kind in ("skill", "rule")
        for row in doc.get(kind, [])
    }


def _client_files(work: Path) -> list[Path]:
    return [p for p in sorted((work / ".claude").rglob("*")) if p.is_file()]


def _shared(team, repo: str, proxy: OciProxy, names: tuple[str, ...] = NAMES) -> Team:
    """Alice locks, installs and commits ``names`` through ``proxy``; Bob clones, installs nothing."""
    t = team("CO")
    allow_insecure(t.alice.runner, proxy.host)
    allow_insecure(t.bob.runner, proxy.host)
    make_artifact(f"{repo}/moving", "skill", _moving("v1"), tag="rolling")
    make_artifact(f"{repo}/style", "rule", _style("v1"), tag="rolling")
    make_artifact(
        f"{repo}/steady", "skill", {"steady/SKILL.md": "# steady\n"}, tag="stable"
    )
    refs = {
        "moving": f"{proxy.host}/{repo}/moving:rolling",
        "steady": f"{proxy.host}/{repo}/steady:stable",
        "style": f"{proxy.host}/{repo}/style:rolling",
    }
    write_config(
        t.alice.work,
        skills={n: refs[n] for n in ("moving", "steady") if n in names},
        rules={n: refs[n] for n in ("style",) if n in names},
    )
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    return t


def _warm(t: Team) -> GrimRunner:
    """Bob's workspace on Alice's machine, whose store holds every pin she rendered."""
    a = t.alice.runner
    runner = GrimRunner(a.binary, a.grim_home, cwd=t.bob.work)
    runner.env = dict(a.env)
    return runner


def _pulled_v2(t: Team, repo: str) -> None:
    """Bob installs v1; Alice moves to v2 and pushes; Bob pulls: his records lag (row 4)."""
    t.bob.runner.run("install")
    move_tag(f"{repo}/moving", _moving("v2"), tag="rolling", kind="skill")
    move_tag(f"{repo}/style", _style("v2"), tag="rolling", kind="rule")
    t.alice.runner.run("update")
    t.alice.commit_all("chore: v2")
    t.alice.push()
    t.bob.pull()


# ===========================================================================
# A cold store: online it fetches once and adopts, offline it sends nothing
# ===========================================================================


@pytest.mark.parametrize("network", ["online", "offline"])
def test_a_cold_store_fetches_once_online_and_sends_nothing_offline(
    team, unique_repo, oci_proxy, network
):
    # adr_output_ownership_without_state.md validation cases 25 and 26
    proxy = oci_proxy("count")
    t = _shared(team, unique_repo, proxy, names=("steady",))
    work = t.bob.work
    flags = ("--offline",) if network == "offline" else ()
    requests = _requests(proxy)
    before = proxy.counts
    rc, doc, stderr = _status(t.bob.runner, *flags)
    assert rc == 0, stderr
    if network == "offline":
        assert _states(doc) == {"steady": "missing"}
        assert "offline: 1 artifact was not proven" in stderr
        assert "run `grim status` again online" in stderr
        assert "adopted" not in stderr
        assert not (work / ".grimoire").exists()
        assert _requests(proxy) == requests
        return
    assert _states(doc) == {"steady": "installed"}
    assert "adopted 1 artifact into the install record; 0 left unproven" in stderr
    sent = {path: n - before.get(path, 0) for path, n in proxy.counts.items()}
    assert _reads(sent, "/manifests/") == 1, sent
    assert _reads(sent, "/blobs/") == 1, sent
    assert _record_pins(work) == _lock_pins(work)
    snap = state_snapshot(work / ".grimoire" / "state.json", *_client_files(work))
    requests = _requests(proxy)
    rc, again, stderr = _status(t.bob.runner)
    assert rc == 0, stderr
    assert _states(again) == {"steady": "installed"}
    assert "adopted" not in stderr
    assert _requests(proxy) == requests
    assert_unchanged(snap)
    assert _keys(doc) == _keys(again)


def test_an_online_pull_with_a_cold_store_adopts_the_lock_pin_once(
    team, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md row 4, online
    proxy = oci_proxy("count")
    t = _shared(team, unique_repo, proxy)
    _pulled_v2(t, unique_repo)
    work = t.bob.work
    stale = _record_pins(work)
    assert stale != _lock_pins(work)
    requests = _requests(proxy)
    rc, doc, stderr = _status(t.bob.runner)
    assert rc == 0, stderr
    assert _states(doc) == {name: "installed" for name in NAMES}
    assert _requests(proxy) > requests, "the cold store was not fetched from"
    assert _record_pins(work) == _lock_pins(work)
    snap = state_snapshot(work / ".grimoire" / "state.json", *_client_files(work))
    requests = _requests(proxy)
    rc, again, stderr = _status(t.bob.runner)
    assert rc == 0, stderr
    assert _states(again) == {name: "installed" for name in NAMES}
    assert "adopted" not in stderr
    assert _requests(proxy) == requests
    assert_unchanged(snap)


def test_a_pin_the_registry_no_longer_holds_reads_missing_and_is_not_asked_twice(
    team, unique_repo, oci_proxy
):
    proxy = oci_proxy("count")
    t = _shared(team, unique_repo, proxy, names=("steady",))
    work = t.bob.work
    gone_pin(work / "grimoire.lock", "steady")
    requests = _requests(proxy)
    rc, doc, stderr = _status(t.bob.runner)
    assert rc == 0, stderr
    assert _states(doc) == {"steady": "missing"}
    assert _requests(proxy) > requests, "the gone pin was never asked for"
    assert not (work / ".grimoire").exists()
    requests = _requests(proxy)
    rc, doc, stderr = _status(t.bob.runner)
    assert rc == 0, stderr
    assert _states(doc) == {"steady": "missing"}
    assert _requests(proxy) == requests
    assert not (work / ".grimoire").exists()


# ===========================================================================
# A registry that accepts and never answers
# ===========================================================================


def test_a_silent_registry_ends_inside_the_deadline_and_is_not_asked_again(
    team, unique_repo, oci_proxy
):
    # adr_output_ownership_without_state.md validation case 32;
    # research_install_record_cache_test_matrix.md S55
    proxy = oci_proxy("count")
    t = _shared(team, unique_repo, proxy)
    work = t.bob.work
    proxy.mode = "hang"
    started = time.monotonic()
    rc, doc, stderr = _status(t.bob.runner, timeout=60)
    elapsed = time.monotonic() - started
    assert rc == 0, stderr
    # One reader for the run: the host fails once, at the 10 s call deadline,
    # and the other two artifacts are not sent to it.
    assert 9 < elapsed < 20, elapsed
    assert _states(doc) == {name: "missing" for name in NAMES}
    assert ADOPTED.format(0, 3) in stderr
    assert not (work / ".grimoire").exists()
    proxy.mode = "count"
    requests = _requests(proxy)
    rc, doc, stderr = _status(t.bob.runner, timeout=60)
    assert rc == 0, stderr
    assert _states(doc) == {name: "missing" for name in NAMES}
    assert _requests(proxy) == requests


# ===========================================================================
# More candidates than one run may fetch
# ===========================================================================

MANY = 70
CAP = 64


def test_candidates_past_the_fetch_cap_are_named_and_adopted_by_the_next_run(
    grim_at, project_dir, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md S56
    skills = {}
    for i in range(MANY):
        name = f"s{i:02d}"
        make_artifact(
            f"{unique_repo}/{name}",
            "skill",
            {f"{name}/SKILL.md": f"# {name}\n"},
            tag="1",
        )
        skills[name] = f"{REGISTRY_HOST}/{unique_repo}/{name}:1"
    write_config(project_dir, skills=skills)
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    shutil.rmtree(project_dir / ".grimoire")
    cold = cold_runner(runner, tmp_path, "cold")
    rc, doc, stderr = _status(cold, "--log-level", "debug")
    assert rc == 0, stderr[-2000:]
    states = list(_states(doc).values())
    assert states.count("installed") == CAP
    assert states.count("missing") == MANY - CAP
    assert ADOPTED.format(CAP, MANY - CAP) in stderr
    named = [
        line
        for line in stderr.splitlines()
        if "left unproven: NeverAttempted(FetchBudget)" in line
    ]
    assert len(named) == MANY - CAP
    rc, doc, stderr = _status(cold)
    assert rc == 0, stderr
    assert set(_states(doc).values()) == {"installed"}
    assert ADOPTED.format(MANY - CAP, 0) in stderr
    assert _record_pins(project_dir) == _lock_pins(project_dir)


# ===========================================================================
# Concurrency: a held state-file lock, and a command beside 20 status runs
# ===========================================================================

RUNS = 20


@posix_only
def test_a_held_state_lock_keeps_twenty_concurrent_runs_from_writing(
    team, unique_repo, oci_proxy
):
    # adr_output_ownership_without_state.md validation case 28 (held lock)
    t = _shared(team, unique_repo, oci_proxy("count"))
    _pulled_v2(t, unique_repo)
    work = t.bob.work
    runner = _warm(t)
    snap = state_snapshot(work / ".grimoire" / "state.json", *_client_files(work))
    with held_flock(work / ".grimoire" / "state.json.lock"):
        with ThreadPoolExecutor(max_workers=RUNS) as pool:
            results = list(pool.map(lambda _: _status(runner), range(RUNS)))
        for rc, doc, stderr in results:
            assert rc == 0, stderr
            assert _states(doc) == {name: "installed" for name in NAMES}
            assert "records not written:" in stderr
        assert_unchanged(snap)
    rc, doc, stderr = _status(runner)
    assert rc == 0, stderr
    assert ADOPTED.format(2, 0) in stderr
    assert _record_pins(work) == _lock_pins(work)


@posix_only
@pytest.mark.parametrize("command", ["install", "uninstall"])
def test_a_command_beside_twenty_status_runs_keeps_its_records(
    team, unique_repo, oci_proxy, tmp_path, command
):
    # adr_output_ownership_without_state.md validation case 28;
    # research_install_record_cache_test_matrix.md S62
    t = _shared(team, unique_repo, oci_proxy("count"))
    _pulled_v2(t, unique_repo)
    work = t.bob.work
    runner = _warm(t)
    args = ("install",) if command == "install" else ("uninstall", "skill", "moving")
    # The same command, then one status, in a copy of the workspace on the same machine.
    twin = tmp_path / "twin"
    shutil.copytree(work, twin, symlinks=True)
    alone = GrimRunner(runner.binary, runner.grim_home, cwd=twin)
    alone.env = dict(runner.env)
    assert alone.run(*args, check=False).returncode == 0
    assert _status(alone)[0] == 0
    with ThreadPoolExecutor(max_workers=RUNS + 1) as pool:
        statuses = [pool.submit(_status, runner) for _ in range(RUNS)]
        done = pool.submit(runner.run, *args, check=False, timeout=120)
        results = [f.result() for f in statuses]
    assert done.result().returncode == 0, done.result().stderr
    for rc, _, stderr in results:
        assert rc == 0, stderr
    rc, _, stderr = _status(runner)
    assert rc == 0, stderr
    assert _comparable(work) == _comparable(twin)
    if command == "uninstall":
        assert "moving" not in _record_pins(work)
        assert not (work / ".claude" / "skills" / "moving").exists()
