# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim status`` writes the records it proves from the local store.

A record is trusted while the files match it. When they do not and the lock
names another pin, or there is no record and the files exist, ``status``
compares the files with a render of the lock's pin from the local store. A
match reads ``installed`` and its record is written under the state-file lock;
a second run finds nothing to prove and writes nothing. A write that cannot
happen (a held lock, a read-only or linked ``.grimoire``) leaves the report and
the exit as they are. Column b of ``research_install_record_cache_test_matrix.md``
lines 01 to 14, its S10, S13, S24, S45, S59, S60, E12 and E13, and
``adr_output_ownership_without_state.md`` validation cases 24, 27, 29 and 30.

World: a skill ``moving`` and a rule ``style`` whose tags move from v1 to v2,
and a skill ``steady`` whose tag never moves.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import (
    held_flock,
    make_artifact,
    make_settings,
    settings_sandbox,
    write_config,
)
from src.oci_proxy import OciProxy
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.scenarios import (
    Team,
    allow_insecure,
    assert_unchanged,
    clone_workspace,
    cold_runner,
    fresh_runner,
    git,
    move_tag,
    state_snapshot,
)

NAMES = ("moving", "style", "steady")
MODELS = ["CO", "GO"]
EDIT = "# local edit\n"

posix_only = pytest.mark.skipif(
    os.name == "nt", reason="needs POSIX links, modes and flock"
)
not_root = pytest.mark.skipif(
    os.name != "nt" and os.geteuid() == 0,
    reason="root ignores directory modes",
)
sh_only = pytest.mark.skipif(
    os.name == "nt", reason="the hook script is a /bin/sh file"
)


def _moving(version: str) -> dict[str, str]:
    return {
        "moving/SKILL.md": f"# moving {version}\n",
        "moving/ref.md": f"ref {version}\n",
    }


def _style(version: str) -> dict[str, str]:
    return {"style.md": f"---\npaths: ['**/*.rs']\n---\n# style {version}\n"}


def _publish_v1(repo: str) -> None:
    make_artifact(f"{repo}/moving", "skill", _moving("v1"), tag="rolling")
    make_artifact(f"{repo}/style", "rule", _style("v1"), tag="rolling")
    make_artifact(
        f"{repo}/steady", "skill", {"steady/SKILL.md": "# steady\n"}, tag="stable"
    )


def _publish_v2(repo: str) -> None:
    move_tag(f"{repo}/moving", _moving("v2"), tag="rolling", kind="skill")
    move_tag(f"{repo}/style", _style("v2"), tag="rolling", kind="rule")


def _declare(root: Path, repo: str, host: str) -> None:
    write_config(
        root,
        skills={
            "moving": f"{host}/{repo}/moving:rolling",
            "steady": f"{host}/{repo}/steady:stable",
        },
        rules={"style": f"{host}/{repo}/style:rolling"},
    )


# ---------------------------------------------------------------------------
# Where a scope keeps its state, and what a run did to it
# ---------------------------------------------------------------------------


@dataclass
class Place:
    runner: GrimRunner
    flags: tuple[str, ...]
    state: Path
    root: Path
    lock: Path

    def status(self, runner: GrimRunner | None = None, pre: tuple[str, ...] = ()):
        """Run ``grim --format json status``; return ``(exit, parsed stdout, stderr)``."""
        result = (runner or self.runner).run(
            *pre, *self.flags, "status", format="json", check=False
        )
        try:
            doc = json.loads(result.stdout)
        except json.JSONDecodeError:
            doc = {}
        return result.returncode, doc, result.stderr

    def client_files(self) -> list[Path]:
        return [p for p in sorted((self.root / ".claude").rglob("*")) if p.is_file()]

    def snapshot(self):
        """``state`` bytes, mtime and inode, plus mtime and inode of every client file."""
        return state_snapshot(self.state, *self.client_files())

    def record_pins(self) -> dict[str, str]:
        doc = json.loads(self.state.read_text(encoding="utf-8"))
        return {r["name"]: r["pinned"].rsplit("@", 1)[1] for r in doc["records"]}

    def lock_pins(self) -> dict[str, str]:
        doc = tomllib.loads(self.lock.read_text(encoding="utf-8"))
        return {
            row["name"]: row["pinned"].rsplit("@", 1)[1]
            for kind in ("skill", "rule")
            for row in doc.get(kind, [])
        }


def _project(runner: GrimRunner) -> Place:
    work = runner.cwd
    assert work is not None
    return Place(
        runner, (), work / ".grimoire" / "state.json", work, work / "grimoire.lock"
    )


def _global(runner: GrimRunner) -> Place:
    home = runner.grim_home
    return Place(
        runner,
        ("--global",),
        home / "state" / "global.json",
        runner.home,
        home / "grimoire.lock",
    )


def _states(doc: dict) -> dict[str, str]:
    return {row["name"]: row["state"] for row in doc.get("items", [])}


def _requests(proxy: OciProxy) -> int:
    return sum(proxy.counts.values())


def _assert_recorded_then_silent(
    place: Place, runner: GrimRunner | None = None
) -> None:
    """One run writes the records the lock pins and touches no client file; the next writes nothing."""
    before = place.snapshot()
    rc, doc, _ = place.status(runner)
    assert rc == 0, doc
    assert _states(doc) == {name: "installed" for name in NAMES}
    assert place.record_pins() == place.lock_pins()
    state, *outputs = before.files
    assert place.state.read_bytes() != state.content
    for output in outputs:
        assert output.path.stat().st_mtime_ns == output.mtime_ns, output.path
    again = place.snapshot()
    rc, doc, stderr = place.status(runner)
    assert rc == 0, doc
    assert _states(doc) == {name: "installed" for name in NAMES}
    assert "WARN" not in stderr
    assert_unchanged(again)


# ---------------------------------------------------------------------------
# A team: Alice installed v1 and pushed; Bob pulled and installed it
# ---------------------------------------------------------------------------


def _v1(
    team: Callable[[str], Team], repo: str, proxy: OciProxy, model: str = "CO"
) -> Team:
    t = team(model)
    allow_insecure(t.alice.runner, proxy.host)
    allow_insecure(t.bob.runner, proxy.host)
    _publish_v1(repo)
    _declare(t.alice.work, repo, proxy.host)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    return t


def _alice_to_v2(t: Team, repo: str, *, lock_only: bool = False) -> None:
    _publish_v2(repo)
    t.alice.runner.run("update")
    if lock_only:
        git(t.alice.work, "add", "grimoire.lock")
        git(t.alice.work, "commit", "-q", "-m", "chore: bump lock")
    else:
        t.alice.commit_all("chore: v2")
    t.alice.push()
    t.bob.pull()


def _warm(t: Team) -> GrimRunner:
    """Bob's workspace on Alice's machine, whose store holds every pin she rendered."""
    a = t.alice.runner
    runner = GrimRunner(a.binary, a.grim_home, cwd=t.bob.work)
    runner.env = dict(a.env)
    return runner


def _edit(place: Place, *names: str) -> None:
    for name in names:
        path = place.root / ".claude" / "skills" / name / "SKILL.md"
        if not path.exists():
            path = place.root / ".claude" / "rules" / f"{name}.md"
        path.write_text(
            path.read_text(encoding="utf-8") + EDIT, encoding="utf-8", newline="\n"
        )


def _row4(t: Team, repo: str) -> Place:
    """Lock and files at v2, Bob's record at v1: a pull, or a render copied in."""
    _alice_to_v2(t, repo)
    if t.model == "GO":
        shutil.copytree(
            t.alice.work / ".claude", t.bob.work / ".claude", dirs_exist_ok=True
        )
    return _project(t.bob.runner)


def _row5(t: Team, repo: str) -> Place:
    """Lock and files at v2, no record: a clone, or ``.grimoire/`` removed."""
    bob = _row4(t, repo)
    shutil.rmtree(bob.root / ".grimoire")
    return bob


# ===========================================================================
# Column b, lines 01 to 06: rows 1 to 3 read no content and write nothing
# ===========================================================================


@pytest.mark.parametrize("row", ["current", "lagging", "edited"])
@pytest.mark.parametrize("model", MODELS)
def test_rows_one_to_three_read_no_content_and_write_nothing(
    team, unique_repo, oci_proxy, tmp_path, model, row
):
    # research_install_record_cache_test_matrix.md M01b-M06b
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy, model)
    bob = _project(t.bob.runner)
    expected = {name: "installed" for name in NAMES}
    if row == "lagging":
        _alice_to_v2(t, unique_repo, lock_only=model == "CO")
        expected |= {"moving": "outdated", "style": "outdated"}
    elif row == "edited":
        _edit(bob, "moving", "style")
        expected |= {"moving": "modified", "style": "modified"}
    cold = cold_runner(t.bob.runner, tmp_path, "empty")
    snap = bob.snapshot()
    requests = _requests(proxy)
    rc, doc, _ = bob.status(cold)
    assert rc == 0, doc
    assert _states(doc) == expected
    assert_unchanged(snap)
    assert list(cold.grim_home.rglob("*")) == []
    assert _requests(proxy) == requests


# ===========================================================================
# Lines 07 to 10: row 4, a record that lags files the lock may own
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_a_pulled_render_is_re_pinned_then_status_stays_silent(
    team, unique_repo, oci_proxy, model
):
    # research_install_record_cache_test_matrix.md M07b, M08b, S59 (status half)
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy, model)
    bob = _row4(t, unique_repo)
    requests = _requests(proxy)
    _assert_recorded_then_silent(bob, _warm(t))
    assert _requests(proxy) == requests


@pytest.mark.parametrize("model", MODELS)
def test_a_pulled_render_with_an_edit_writes_nothing(
    team, unique_repo, oci_proxy, model
):
    # research_install_record_cache_test_matrix.md M09b, M10b
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy, model)
    bob = _row4(t, unique_repo)
    _edit(bob, "moving", "style")
    for _ in range(2):
        snap = bob.snapshot()
        rc, doc, stderr = bob.status(_warm(t))
        assert rc == 0, doc
        assert _states(doc) == {
            "moving": "modified",
            "style": "modified",
            "steady": "installed",
        }
        assert "2 left unproven" in stderr
        assert_unchanged(snap)


# ===========================================================================
# Lines 11 to 14: row 5, no record and files on disk
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_no_record_status_adopts_from_a_warm_store_then_stays_silent(
    team, unique_repo, oci_proxy, model
):
    # research_install_record_cache_test_matrix.md M11b, M12b, S59 (status half);
    # adr_output_ownership_without_state.md validation case 24
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy, model)
    bob = _row5(t, unique_repo)
    requests = _requests(proxy)
    _assert_recorded_then_silent(bob, _warm(t))
    assert _requests(proxy) == requests


@pytest.mark.parametrize("model", MODELS)
def test_no_record_status_on_a_cold_store_reads_missing_and_writes_nothing(
    team, unique_repo, oci_proxy, tmp_path, model
):
    # research_install_record_cache_test_matrix.md M11b, M12b (cold store)
    # Offline: online, a cold store now fetches and adopts (ADR validation case 25).
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy, model)
    bob = _row5(t, unique_repo)
    requests = _requests(proxy)
    rc, doc, _ = bob.status(
        cold_runner(t.bob.runner, tmp_path, "empty"), pre=("--offline",)
    )
    assert rc == 0, doc
    assert _states(doc) == {name: "missing" for name in NAMES}
    assert not (bob.root / ".grimoire").exists()
    assert _requests(proxy) == requests


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_writes_nothing(
    team, unique_repo, oci_proxy, model
):
    # research_install_record_cache_test_matrix.md M13b, M14b
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy, model)
    bob = _row5(t, unique_repo)
    _edit(bob, *NAMES)
    for _ in range(2):
        rc, doc, stderr = bob.status(_warm(t))
        assert rc == 0, doc
        assert _states(doc) == {name: "missing" for name in NAMES}
        assert "3 left unproven" in stderr
        assert not (bob.root / ".grimoire").exists()


# ===========================================================================
# Scenarios: another tree on the same machine, a pull after a status write
# ===========================================================================


@pytest.mark.parametrize("how", ["worktree", "copy"])
def test_another_tree_on_the_same_machine_adopts_from_the_shared_store(
    team, unique_repo, oci_proxy, tmp_path, how
):
    # research_install_record_cache_test_matrix.md S10 (status half), S24
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy)
    main = _project(t.bob.runner)
    tree = tmp_path / "bob-other"
    if how == "worktree":
        git(t.bob.work, "worktree", "add", "-q", "--detach", str(tree), "HEAD")
    else:
        clone_workspace(t.bob.work, tree)
    runner = GrimRunner(t.bob.runner.binary, t.bob.runner.grim_home, cwd=tree)
    runner.env = dict(t.bob.runner.env)
    other = _project(runner)
    assert not other.state.exists()
    main_before = main.snapshot()
    requests = _requests(proxy)
    rc, doc, _ = other.status()
    assert rc == 0, doc
    assert _states(doc) == {name: "installed" for name in NAMES}
    assert other.record_pins() == other.lock_pins()
    assert _requests(proxy) == requests
    assert_unchanged(main_before)


def test_records_status_wrote_survive_a_pull_and_install_exits_zero(
    team, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md S60
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy)
    bob = _project(t.bob.runner)
    shutil.rmtree(bob.root / ".grimoire")
    rc, doc, _ = bob.status()
    assert rc == 0, doc
    assert bob.record_pins() == bob.lock_pins()
    _alice_to_v2(t, unique_repo)
    result = t.bob.runner.run("install", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert {row["status"] for row in json.loads(result.stdout)["items"]} == {
        "unchanged"
    }
    assert bob.record_pins() == bob.lock_pins()


# ===========================================================================
# Both scopes: no outputs create nothing, a deleted global record comes back
# ===========================================================================


def _machine(tmp_path: Path, unique_repo: str, scope: str) -> Place:
    if scope == "project":
        work = tmp_path / "work"
        (work / ".claude").mkdir(parents=True)
        place = _project(fresh_runner(tmp_path, "machine", cwd=work))
    else:
        place = _global(fresh_runner(tmp_path, "machine"))
        (place.root / ".claude").mkdir(parents=True)
    _publish_v1(unique_repo)
    _declare(place.lock.parent, unique_repo, REGISTRY_HOST)
    assert place.runner.run(*place.flags, "lock", check=False).returncode == 0
    return place


@pytest.mark.parametrize("scope", ["project", "global"])
def test_no_outputs_create_no_state(tmp_path, unique_repo, scope):
    # adr_output_ownership_without_state.md validation case 29
    place = _machine(tmp_path, unique_repo, scope)
    rc, doc, _ = place.status()
    assert rc == 0, doc
    assert _states(doc) == {name: "missing" for name in NAMES}
    assert not place.state.exists()
    if scope == "project":
        assert not place.state.parent.exists()


def test_global_status_adopts_after_global_json_is_deleted(tmp_path, unique_repo):
    # adr_output_ownership_without_state.md § Part B, B1 (both scopes)
    place = _machine(tmp_path, unique_repo, "global")
    assert place.runner.run("--global", "install", check=False).returncode == 0
    place.state.unlink()
    _assert_recorded_then_silent(place)


# ===========================================================================
# A write that cannot happen: the report comes from memory, the exit is kept
# ===========================================================================


def _outside_files(root: Path) -> dict[Path, tuple[bytes, int]]:
    return {
        p: (p.read_bytes(), p.stat().st_mtime_ns)
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def _assert_reported_from_memory(bob: Place, runner: GrimRunner) -> None:
    rc, doc, stderr = bob.status(runner)
    assert rc == 0, doc
    assert _states(doc) == {name: "installed" for name in NAMES}
    # adr_output_ownership_without_state.md § Part B, B5
    assert "records not written:" in stderr
    assert "adopted" not in stderr


@posix_only
@pytest.mark.parametrize("link", ["directory", "state-file"])
def test_a_linked_state_is_reported_from_memory_and_left_alone(
    team, unique_repo, oci_proxy, tmp_path, link
):
    # research_install_record_cache_test_matrix.md E12;
    # adr_output_ownership_without_state.md validation case 27 (linked half)
    t = _v1(team, unique_repo, oci_proxy("count"))
    bob = _row4(t, unique_repo)
    away = tmp_path / "away"
    away.mkdir()
    local = bob.root / ".grimoire"
    if link == "directory":
        shutil.move(local, away / "grimoire")
        local.symlink_to(away / "grimoire", target_is_directory=True)
    else:
        shutil.move(bob.state, away / "state.json")
        bob.state.symlink_to(away / "state.json")
    before = _outside_files(away)
    for _ in range(2):
        _assert_reported_from_memory(bob, _warm(t))
    assert _outside_files(away) == before
    assert (local if link == "directory" else bob.state).is_symlink()


@posix_only
@not_root
def test_a_read_only_state_directory_is_reported_from_memory(
    team, unique_repo, oci_proxy
):
    # adr_output_ownership_without_state.md validation case 27 (read-only half)
    t = _v1(team, unique_repo, oci_proxy("count"))
    bob = _row4(t, unique_repo)
    folder = bob.state.parent
    (folder / "state.json.lock").unlink(missing_ok=True)
    snap = bob.snapshot()
    mode = folder.stat().st_mode
    folder.chmod(stat.S_IRUSR | stat.S_IXUSR)
    try:
        _assert_reported_from_memory(bob, _warm(t))
    finally:
        folder.chmod(mode)
    assert_unchanged(snap)
    assert not (folder / "state.json.lock").exists()


@posix_only
def test_a_held_state_lock_skips_the_write(team, unique_repo, oci_proxy):
    # adr_output_ownership_without_state.md § Part B, B3 (contention)
    t = _v1(team, unique_repo, oci_proxy("count"))
    bob = _row4(t, unique_repo)
    snap = bob.snapshot()
    with held_flock(bob.state.parent / "state.json.lock"):
        _assert_reported_from_memory(bob, _warm(t))
        assert_unchanged(snap)
    # Released, the same run writes.
    _assert_recorded_then_silent(bob, _warm(t))


@posix_only
def test_a_grimoire_file_keeps_the_exit_of_status_and_install(
    team, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md E13
    t = _v1(team, unique_repo, oci_proxy("count"))
    bob = _row5(t, unique_repo)
    marker = bob.root / ".grimoire"
    marker.write_bytes(b"not a directory\n")
    for command in ("status", "install"):
        result = _warm(t).run(command, format="json", check=False)
        assert result.returncode == 74, (command, result.stderr)
        assert json.loads(result.stdout)["error"]["code"] == "io"
    assert marker.read_bytes() == b"not a directory\n"


# ===========================================================================
# What is left alone: a stale lock, hooks and settings, the legacy file
# ===========================================================================


def test_a_stale_lock_adopts_nothing_until_relocked(team, unique_repo, oci_proxy):
    # research_install_record_cache_test_matrix.md S13 (Part B half)
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy)
    bob = _row5(t, unique_repo)
    make_artifact(
        f"{unique_repo}/extra", "skill", {"extra/SKILL.md": "# extra\n"}, tag="v1"
    )
    config = bob.root / "grimoire.toml"
    text = config.read_text(encoding="utf-8").replace(
        "[skills]\n", f'[skills]\nextra = "{proxy.host}/{unique_repo}/extra:v1"\n', 1
    )
    config.write_text(text, encoding="utf-8", newline="\n")
    requests = _requests(proxy)
    rc, doc, _ = bob.status(_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {name: "stale" for name in ("extra", *NAMES)}
    assert not (bob.root / ".grimoire").exists()
    assert _requests(proxy) == requests
    assert t.bob.runner.run("lock", check=False).returncode == 0
    rc, doc, _ = bob.status(_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {"extra": "missing", **{name: "installed" for name in NAMES}}
    assert bob.record_pins() == {
        name: pin for name, pin in bob.lock_pins().items() if name != "extra"
    }


HOOK_TOML = """\
schema = 1
name = "guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""


def _watched(runner: GrimRunner, work: Path) -> dict[Path, tuple[bytes, int]]:
    """Every file under the hook and settings homes, and every client settings file."""
    roots = [runner.grim_home / "hooks", runner.grim_home / "settings"]
    files = [p for root in roots for p in root.rglob("*") if p.is_file()]
    files += list((work / ".claude").glob("settings*.json"))
    return {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in sorted(files)}


@sh_only
def test_status_writes_no_hook_or_settings_record(
    grim_at, project_dir, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md S45 (status half)
    hook = make_artifact(
        f"{unique_repo}/guard",
        "hook",
        {
            "guard/hook.toml": HOOK_TOML,
            "guard/guard.sh": "#!/bin/sh\ncat > /dev/null\necho '{}'\n",
        },
        tag="1",
    )
    team_settings = make_settings(
        f"{unique_repo}/team",
        "1",
        {"description": "team", "claude": {"model": "sonnet"}},
    )
    solo = make_artifact(
        f"{unique_repo}/solo", "skill", {"solo/SKILL.md": "# solo\n"}, tag="1"
    )
    write_config(
        project_dir,
        skills={"solo": solo.fq},
        hooks={"guard": hook.fq},
        settings={"team": team_settings.fq},
        options={"clients": json.dumps(["claude"])},
    )
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    runner.run("lock")
    for flag in ("hooks", "settings"):
        runner.run("config", "set", f"options.experimental.{flag}", "true")
    runner.run("consent", "allow")
    runner.run("install")
    assert (runner.grim_home / "hooks" / "dispatch.json").is_file()
    assert list((runner.grim_home / "settings").glob("*.json"))
    shutil.rmtree(project_dir / ".grimoire")
    before = _watched(runner, project_dir)
    for _ in range(2):
        result = runner.run("status", format="json", check=False)
        assert result.returncode == 0, result.stderr
        assert {r["name"]: r["state"] for r in json.loads(result.stdout)["items"]}[
            "solo"
        ] == "installed"
        records = json.loads(
            (project_dir / ".grimoire" / "state.json").read_text(encoding="utf-8")
        )["records"]
        assert {(r["kind"], r["name"]) for r in records} == {("skill", "solo")}
        assert _watched(runner, project_dir) == before


def test_a_status_write_reaps_the_legacy_state_and_keeps_settings_records(
    grim_at, project_dir, unique_repo, tmp_path
):
    # adr_output_ownership_without_state.md validation case 30
    make_artifact(f"{unique_repo}/moving", "skill", _moving("v1"), tag="rolling")
    team_settings = make_settings(
        f"{unique_repo}/team",
        "1",
        {"description": "team", "claude": {"model": "sonnet"}},
    )
    write_config(
        project_dir,
        skills={"moving": f"{REGISTRY_HOST}/{unique_repo}/moving:rolling"},
        settings={"team": team_settings.fq},
        options={"clients": json.dumps(["claude"])},
    )
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    runner.run("lock")
    runner.run("config", "set", "options.experimental.settings", "true")
    runner.run("consent", "allow")
    runner.run("install")
    place = _project(runner)
    saved = place.state.read_bytes()
    move_tag(f"{unique_repo}/moving", _moving("v2"), tag="rolling", kind="skill")
    assert runner.run("update", check=False).returncode == 0
    # The pre-relocation file: sha256 of the canonical config path under `state/projects/`.
    sha = hashlib.sha256(
        str((project_dir / "grimoire.toml").resolve()).encode()
    ).hexdigest()
    legacy = runner.grim_home / "state" / "projects" / f"{sha}.json"
    legacy.parent.mkdir(parents=True, exist_ok=True)
    legacy.write_bytes(saved)
    place.state.unlink()
    settings_records = _outside_files(runner.grim_home / "settings")
    assert settings_records
    result = runner.run("status", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert {r["name"]: r["state"] for r in json.loads(result.stdout)["items"]}[
        "moving"
    ] == "installed"
    assert place.record_pins()["moving"] == place.lock_pins()["moving"]
    assert not legacy.exists()
    assert _outside_files(runner.grim_home / "settings") == settings_records


# ===========================================================================
# What a run says: the running line on the progress sink, then one outcome
# ===========================================================================

ADOPTED = "adopted 3 artifacts into the install record"


def _key_sets(doc: dict) -> tuple[list[str], list[str]]:
    return sorted(doc), sorted({key for row in doc["items"] for key in row})


def _events(stderr: str) -> list[dict]:
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def test_json_stdout_keeps_its_keys_through_an_adoption(team, unique_repo, oci_proxy):
    # adr_output_ownership_without_state.md validation case 25 (stdout half)
    t = _v1(team, unique_repo, oci_proxy("count"))
    bob = _row5(t, unique_repo)
    rc, adopted, stderr = bob.status(_warm(t))
    assert rc == 0, adopted
    assert f"{ADOPTED}; 0 left unproven" in stderr
    rc, settled, stderr = bob.status(_warm(t))
    assert rc == 0, settled
    assert "adopted" not in stderr
    assert _key_sets(adopted) == _key_sets(settled)


def test_progress_json_names_each_adoption_on_stderr(team, unique_repo, oci_proxy):
    # adr_output_ownership_without_state.md validation case 31 (`--progress json`)
    t = _v1(team, unique_repo, oci_proxy("count"))
    _row5(t, unique_repo)
    runner = _warm(t)
    first = runner.run("--progress", "json", "status", format="json", check=False)
    assert first.returncode == 0, first.stderr
    assert {row["state"] for row in json.loads(first.stdout)["items"]} == {"installed"}
    events = _events(first.stderr)
    assert [e["event"] for e in events] == ["start", *["advance"] * 3, "finish"]
    assert events[0]["total"] == 3
    assert sorted(e["label"] for e in events[1:-1]) == [
        "adopt rule style",
        "adopt skill moving",
        "adopt skill steady",
    ]
    again = runner.run("--progress", "json", "status", format="json", check=False)
    assert again.returncode == 0, again.stderr
    assert _events(again.stderr) == []


@pytest.mark.parametrize("flags", [("--progress", "none"), ("--log-level", "off")])
def test_progress_none_hides_the_running_line_and_log_level_off_the_outcome(
    team, unique_repo, oci_proxy, flags
):
    # adr_output_ownership_without_state.md validation case 31
    t = _v1(team, unique_repo, oci_proxy("count"))
    bob = _row5(t, unique_repo)
    result = _warm(t).run(*flags, "status", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert {row["state"] for row in json.loads(result.stdout)["items"]} == {"installed"}
    assert _events(result.stderr) == []
    assert "\r" not in result.stderr
    if flags[0] == "--progress":
        assert ADOPTED in result.stderr
    else:
        assert result.stderr == ""
    assert bob.record_pins() == bob.lock_pins()


def test_offline_on_a_cold_store_counts_what_was_not_proven(
    team, unique_repo, oci_proxy, tmp_path
):
    # adr_output_ownership_without_state.md validation case 26;
    # research_install_record_cache_test_matrix.md M11b (offline and cold)
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy)
    bob = _row5(t, unique_repo)
    requests = _requests(proxy)
    cold = cold_runner(t.bob.runner, tmp_path, "empty")
    result = cold.run("--offline", "status", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert _states(json.loads(result.stdout)) == {name: "missing" for name in NAMES}
    assert "offline: 3 artifacts were not proven" in result.stderr
    assert "adopted" not in result.stderr
    assert not (bob.root / ".grimoire").exists()
    assert _requests(proxy) == requests
