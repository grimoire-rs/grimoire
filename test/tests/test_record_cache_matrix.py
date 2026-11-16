# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Core matrix of the install-record cache: five states by seven commands, two models.

Each test is one line-and-column cell of
``research_install_record_cache_test_matrix.md``; the ``model`` parameter flips
the checkout model (committed outputs ``CO``, gitignored outputs ``GO``) and
changes only how the state is produced. Lines 01 to 06 describe today's
behaviour. Lines 07 to 14 assert the target behaviour and fail until the
record-cache work lands.

World: a bundle ``stack`` (members ``helper`` and ``extra``) and a direct skill
``solo``. Alice installs, commits and pushes; Bob pulls and installs, so Bob
holds a record, the lock and the files at pin A.
"""

from __future__ import annotations

import json
import shutil
import tomllib
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, retag, tag_digest
from src.runner import GrimRunner
from src.scenarios import (
    Snapshot,
    Team,
    assert_unchanged,
    cold_runner,
    fresh_runner,
    git,
    move_tag,
    render_at,
    state_snapshot,
)

MODELS = ["CO", "GO"]
EDIT = "# local edit\n"


# ---------------------------------------------------------------------------
# World
# ---------------------------------------------------------------------------


def _helper_files(version: str) -> dict[str, str]:
    files = {"helper/SKILL.md": f"# helper {version}\n"}
    if version != "v3":
        files["helper/ref.md"] = f"ref {version}\n"
    return files


@dataclass
class World:
    team: Team
    tmp: Path
    repo: str
    model: str
    arts: dict[str, PublishedArtifact] = field(default_factory=dict)
    renders: int = 0

    @property
    def alice(self):
        return self.team.alice

    @property
    def bob(self):
        return self.team.bob

    @property
    def work(self) -> Path:
        return self.bob.work

    @property
    def bundle_repo(self) -> str:
        return f"{self.repo}/stack"

    @property
    def state_path(self) -> Path:
        return self.work / ".grimoire" / "state.json"

    def skill_dir(self, name: str) -> Path:
        return self.work / ".claude" / "skills" / name

    def text(self, name: str, leaf: str = "SKILL.md") -> str:
        return (self.skill_dir(name) / leaf).read_text(encoding="utf-8")

    def warm(self) -> GrimRunner:
        """Bob's workspace on Alice's machine, whose store holds every pin she rendered."""
        a = self.alice.runner
        return GrimRunner(a.binary, a.grim_home, cwd=self.work)

    def cold(self) -> GrimRunner:
        self.renders += 1
        return cold_runner(self.bob.runner, self.tmp, f"cold{self.renders}")


def _world(team: Callable[[str], Team], tmp_path: Path, unique_repo: str, model: str) -> World:
    t = team(model)
    w = World(t, tmp_path, unique_repo, model)
    for v in ("v1", "v2", "v3"):
        w.arts[f"helper:{v}"] = make_artifact(f"{unique_repo}/helper", "skill", _helper_files(v), tag=v)
    make_artifact(f"{unique_repo}/extra", "skill", {"extra/SKILL.md": "# extra\n"}, tag="v1")
    make_artifact(f"{unique_repo}/solo", "skill", {"solo/SKILL.md": "# solo v1\n"}, tag="stable")
    _bundle(w, "v1", with_extra=True)
    write_config(
        t.alice.work,
        bundles={"stack": f"{REGISTRY_HOST}/{w.bundle_repo}:rolling"},
        skills={"solo": f"{REGISTRY_HOST}/{unique_repo}/solo:stable"},
    )
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: stack")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    return w


def _bundle(w: World, helper_tag: str, *, with_extra: bool) -> None:
    try:
        old = tag_digest(w.bundle_repo, "rolling")
        retag(w.bundle_repo, f"keep-{old.removeprefix('sha256:')[:12]}", old)
    except Exception:  # first publish: nothing to keep
        pass
    members = [("skill", "helper", f"{REGISTRY_HOST}/{w.repo}/helper:{helper_tag}")]
    if with_extra:
        members.append(("skill", "extra", f"{REGISTRY_HOST}/{w.repo}/extra:v1"))
    make_bundle(w.bundle_repo, members, tag="rolling")


def _move_solo(w: World, version: str) -> None:
    move_tag(f"{w.repo}/solo", {"solo/SKILL.md": f"# solo {version}\n"}, tag="stable")


def _roll_b(w: World) -> None:
    """Registry at B: helper v2 and solo v2, extra kept."""
    _bundle(w, "v2", with_extra=True)
    _move_solo(w, "v2")


def _roll_c(w: World) -> None:
    """Registry at C: helper v3 without ``ref.md``, ``extra`` dropped from the bundle, solo v3."""
    _bundle(w, "v3", with_extra=False)
    _move_solo(w, "v3")


# ---------------------------------------------------------------------------
# Row setups. After each, Bob's lock names B; the record and files differ per row.
# ---------------------------------------------------------------------------


def _alice_to_b(w: World, *, lock_only: bool) -> None:
    _roll_b(w)
    w.alice.runner.run("update")
    if lock_only:
        git(w.alice.work, "add", "grimoire.lock")
        git(w.alice.work, "commit", "-q", "-m", "chore: bump lock")
    else:
        w.alice.commit_all("chore: update")
    w.alice.push()
    w.bob.pull()


def _row2(w: World) -> None:
    """Lock at B, record and files at A (a lock-only commit, or a pull with ignored outputs)."""
    _alice_to_b(w, lock_only=w.model == "CO")


def _row4(w: World) -> None:
    """Lock and files at B, record at A (a pull, or a render copied in)."""
    _alice_to_b(w, lock_only=False)
    if w.model == "GO":
        shutil.copytree(w.alice.work / ".claude", w.work / ".claude", dirs_exist_ok=True)


def _row5(w: World) -> None:
    """Lock and files at B, no record."""
    _row4(w)
    shutil.rmtree(w.work / ".grimoire")


def _edit(w: World, *names: str) -> None:
    for name in names:
        path = w.skill_dir(name) / "SKILL.md"
        path.write_text(path.read_text(encoding="utf-8") + EDIT, encoding="utf-8", newline="\n")


def _mismatch(w: World, name: str) -> None:
    """Files that match neither the record nor the lock: a hand edit (CO), a third pin's render (GO)."""
    if w.model == "GO" and name == "helper":
        w.renders += 1
        count = w.renders
        render_at(
            lambda scratch: fresh_runner(w.tmp, f"render{count}", cwd=scratch),
            w.arts["helper:v3"],
            w.work,
            clients=["claude"],
        )
    else:
        _edit(w, name)


def _row4_nomatch(w: World, *names: str) -> None:
    _row4(w)
    for name in names:
        _mismatch(w, name)


def _row5_nomatch(w: World, *names: str) -> None:
    _row4_nomatch(w, *names)
    shutil.rmtree(w.work / ".grimoire")


# ---------------------------------------------------------------------------
# Observation
# ---------------------------------------------------------------------------


def _go(runner: GrimRunner, *args: str, offline: bool = False):
    """Run ``grim --format json`` and return ``(exit, parsed stdout, stderr)``."""
    flags = ["--offline"] if offline else []
    result = runner.run(*flags, *args, format="json", check=False)
    try:
        doc = json.loads(result.stdout)
    except json.JSONDecodeError:
        doc = {}
    return result.returncode, doc, result.stderr


def _rows(doc: dict) -> dict[str, dict]:
    return {row["name"]: row for row in doc.get("items", [])}


def _empty_store(runner: GrimRunner) -> None:
    shutil.rmtree(runner.grim_home / "blobs", ignore_errors=True)


def _digest(pinned: str) -> str:
    return pinned.rsplit("@", 1)[1]


def _record_pins(w: World) -> dict[str, str]:
    doc = json.loads(w.state_path.read_text(encoding="utf-8"))
    return {r["name"]: _digest(r["pinned"]) for r in doc["records"]}


def _lock_pins(w: World) -> dict[str, str]:
    doc = tomllib.loads((w.work / "grimoire.lock").read_text(encoding="utf-8"))
    return {row["name"]: _digest(row["pinned"]) for row in doc.get("skill", [])}


def _snap(w: World, *names: str):
    paths = [w.skill_dir(n) / "SKILL.md" for n in names]
    return state_snapshot(w.state_path, *paths)


def _frozen(w: World, *names: str) -> Callable[[], None]:
    """Record bytes plus output bytes and mtimes; an install may rewrite an identical record."""
    before = w.state_path.read_bytes()
    paths = [w.skill_dir(n) / "SKILL.md" for n in names]
    outputs = state_snapshot(paths[0], *paths[1:])

    def check() -> None:
        assert w.state_path.read_bytes() == before
        assert_unchanged(outputs)

    return check


def _lines_naming(stderr: str, token: str) -> list[str]:
    return [line for line in stderr.splitlines() if token in line]


def _record(w: World, name: str) -> dict:
    doc = json.loads(w.state_path.read_text(encoding="utf-8"))
    return next(r for r in doc["records"] if r["name"] == name)


def _assert_recorded_then_silent(w: World, runner: GrimRunner, snap: Snapshot, states: dict[str, str]) -> None:
    """Status wrote the state file and kept every output; a second run reads ``states`` and writes nothing."""
    state, *outputs = snap.files
    assert w.state_path.read_bytes() != state.content
    assert_unchanged(Snapshot(tuple(outputs)))
    again = _snap(w, "helper", "solo", "extra")
    rc, doc, _ = _go(runner, "status", offline=True)
    assert rc == 0
    assert {r["name"]: r["state"] for r in doc["items"] if r["kind"] == "skill"} == states
    assert_unchanged(again)
    pins, recorded = _lock_pins(w), _record_pins(w)
    for name, state_name in states.items():
        if state_name == "installed":
            assert recorded[name] == pins[name], name


# ===========================================================================
# Lines 01 and 02: row 1, steady state. Holds today.
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_steady_state_status_reads_installed_without_content_or_write(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M01a, M02a
    w = _world(team, tmp_path, unique_repo, model)
    _empty_store(w.bob.runner)
    snap = _snap(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.bob.runner, "status", offline=True)
    assert rc == 0
    assert {r["name"]: r["state"] for r in doc["items"] if r["kind"] == "skill"} == {
        "helper": "installed",
        "extra": "installed",
        "solo": "installed",
    }
    assert_unchanged(snap)


@pytest.mark.parametrize("model", MODELS)
def test_steady_state_install_is_unchanged_and_writes_nothing(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M01c, M02c
    w = _world(team, tmp_path, unique_repo, model)
    check = _frozen(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.bob.runner, "install")
    assert rc == 0
    assert {r["status"] for r in doc["items"]} == {"unchanged"}
    check()


@pytest.mark.parametrize("moved", [False, True], ids=["tags-unmoved", "tags-moved"])
@pytest.mark.parametrize("model", MODELS)
def test_steady_state_bare_update(team, tmp_path, unique_repo, model, moved):
    # research_install_record_cache_test_matrix.md M01d, M02d
    w = _world(team, tmp_path, unique_repo, model)
    if not moved:
        check = _frozen(w, "helper", "solo", "extra")
        rc, doc, _ = _go(w.bob.runner, "update")
        assert rc == 0
        assert {r["action"] for r in doc["items"]} == {"unchanged"}
        check()
        return
    _roll_b(w)
    rc, doc, _ = _go(w.bob.runner, "update")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["action"] == "updated"
    assert rows["solo"]["action"] == "updated"
    assert rows["extra"]["action"] == "unchanged"
    assert "helper v2" in w.text("helper")
    assert "solo v2" in w.text("solo")
    assert _record_pins(w) == _lock_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_steady_state_named_update_moves_the_closure_only(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M01e, M02e
    w = _world(team, tmp_path, unique_repo, model)
    before = _record_pins(w)
    _roll_b(w)
    rc, doc, _ = _go(w.bob.runner, "update", "stack")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["action"] == "updated"
    assert "solo" not in rows or rows["solo"]["action"] == "unchanged"
    assert "solo v1" in w.text("solo")
    assert _record_pins(w)["solo"] == before["solo"]
    assert _record_pins(w)["helper"] != before["helper"]


@pytest.mark.parametrize("model", MODELS)
def test_steady_state_uninstall_deletes_files_and_record(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M01f, M02f
    w = _world(team, tmp_path, unique_repo, model)
    rc, doc, _ = _go(w.bob.runner, "uninstall", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "uninstalled"
    assert not w.skill_dir("solo").exists()
    assert "solo" not in _record_pins(w)
    assert w.skill_dir("helper").is_dir()


@pytest.mark.parametrize("model", MODELS)
def test_steady_state_remove_keeps_files_then_update_prunes(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M01g, M02g
    w = _world(team, tmp_path, unique_repo, model)
    rc, doc, _ = _go(w.bob.runner, "remove", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "removed"
    assert w.skill_dir("solo").is_dir()
    rc, doc, _ = _go(w.bob.runner, "update")
    assert rc == 0
    assert _rows(doc)["solo"]["action"] == "removed"
    assert not w.skill_dir("solo").exists()


@pytest.mark.parametrize("command", ["install", "update"])
@pytest.mark.parametrize("model", MODELS)
def test_steady_state_force_writes_nothing_over_intact_files(team, tmp_path, unique_repo, model, command):
    # research_install_record_cache_test_matrix.md M01h, M02h
    w = _world(team, tmp_path, unique_repo, model)
    check = _frozen(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.bob.runner, command, "--force")
    key = "status" if command == "install" else "action"
    assert rc == 0
    assert {r[key] for r in doc["items"]} == {"unchanged"}
    check()


# ===========================================================================
# Lines 03 and 04: row 2, the lock names another pin, files equal the record. Holds today.
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_status_reads_outdated_without_content_or_write(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M03a, M04a
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    _empty_store(w.bob.runner)
    snap = _snap(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.bob.runner, "status", offline=True)
    states = {r["name"]: r["state"] for r in doc["items"] if r["kind"] == "skill"}
    assert rc == 0
    assert states == {"helper": "outdated", "solo": "outdated", "extra": "installed"}
    assert_unchanged(snap)


@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_install_re_renders_to_the_lock(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M03c, M04c
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    rc, doc, _ = _go(w.bob.runner, "install")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["status"] == "updated"
    assert rows["solo"]["status"] == "updated"
    assert "helper v2" in w.text("helper")
    assert _record_pins(w) == _lock_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_bare_update_re_renders_to_the_carried_pin(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M03d, M04d
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    rc, _, _ = _go(w.bob.runner, "update")
    assert rc == 0
    assert "helper v2" in w.text("helper")
    assert "solo v2" in w.text("solo")
    assert _record_pins(w) == _lock_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_named_update_re_renders_the_closure(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M03e, M04e
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    rc, _, _ = _go(w.bob.runner, "update", "stack")
    assert rc == 0
    assert "helper v2" in w.text("helper")
    assert _record_pins(w)["helper"] == _lock_pins(w)["helper"]


@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_uninstall_deletes_by_the_record(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M03f, M04f
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    rc, doc, _ = _go(w.bob.runner, "uninstall", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "uninstalled"
    assert not w.skill_dir("solo").exists()
    assert "solo" not in _record_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_remove_then_update_prunes(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M03g, M04g
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    rc, doc, _ = _go(w.bob.runner, "remove", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "removed"
    rc, doc, _ = _go(w.bob.runner, "update")
    assert rc == 0
    assert _rows(doc)["solo"]["action"] == "removed"
    assert not w.skill_dir("solo").exists()


@pytest.mark.parametrize("command", ["install", "update"])
@pytest.mark.parametrize("model", MODELS)
def test_lock_ahead_force_re_renders_like_the_plain_run(team, tmp_path, unique_repo, model, command):
    # research_install_record_cache_test_matrix.md M03h, M04h
    w = _world(team, tmp_path, unique_repo, model)
    _row2(w)
    rc, _, _ = _go(w.bob.runner, command, "--force")
    assert rc == 0
    assert "helper v2" in w.text("helper")
    assert _record_pins(w) == _lock_pins(w)


# ===========================================================================
# Lines 05 and 06: row 3, a hand edit with record and lock agreeing. Holds today.
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_status_reads_modified_without_content_or_write(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M05a, M06a
    w = _world(team, tmp_path, unique_repo, model)
    _edit(w, "helper", "solo")
    _empty_store(w.bob.runner)
    snap = _snap(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.bob.runner, "status", offline=True)
    states = {r["name"]: r["state"] for r in doc["items"] if r["kind"] == "skill"}
    assert rc == 0
    assert states == {"helper": "modified", "solo": "modified", "extra": "installed"}
    assert_unchanged(snap)


@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_install_refuses_and_keeps_file_and_record(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M05c, M06c
    w = _world(team, tmp_path, unique_repo, model)
    _edit(w, "helper")
    check = _frozen(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.bob.runner, "install")
    assert rc == 65
    assert doc["error"]["reason"] == "modified"
    assert EDIT in w.text("helper")
    check()


@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_bare_update_refuses_and_keeps_the_dropped_edited_node(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M05d, M06d
    w = _world(team, tmp_path, unique_repo, model)
    _roll_c(w)
    _edit(w, "helper", "extra")
    rc, doc, _ = _go(w.bob.runner, "update")
    rows = _rows(doc)
    assert rc == 65
    assert rows["helper"]["refused"] is True
    assert rows["extra"]["action"] == "kept-modified"
    assert EDIT in w.text("helper")
    assert EDIT in w.text("extra")


@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_named_update_refuses_inside_the_closure_only(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M05e, M06e
    w = _world(team, tmp_path, unique_repo, model)
    _roll_c(w)
    _edit(w, "helper", "solo")
    rc, doc, _ = _go(w.bob.runner, "update", "stack")
    rows = _rows(doc)
    assert rc == 65
    assert rows["helper"]["refused"] is True
    assert "solo" not in rows or rows["solo"]["refused"] is False
    assert EDIT in w.text("solo")
    assert "solo v1" in w.text("solo")


@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_uninstall_deletes_the_files_anyway(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M05f, M06f
    w = _world(team, tmp_path, unique_repo, model)
    _edit(w, "solo")
    rc, doc, _ = _go(w.bob.runner, "uninstall", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "uninstalled"
    assert not w.skill_dir("solo").exists()


@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_remove_then_update_keeps_the_edited_files(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M05g, M06g
    w = _world(team, tmp_path, unique_repo, model)
    _edit(w, "solo")
    rc, doc, _ = _go(w.bob.runner, "remove", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "removed"
    rc, doc, _ = _go(w.bob.runner, "update")
    assert _rows(doc)["solo"]["action"] == "kept-modified"
    assert EDIT in w.text("solo")


@pytest.mark.parametrize("command", ["install", "update"])
@pytest.mark.parametrize("model", MODELS)
def test_hand_edit_force_overwrites_the_edit(team, tmp_path, unique_repo, model, command):
    # research_install_record_cache_test_matrix.md M05h, M06h
    w = _world(team, tmp_path, unique_repo, model)
    _edit(w, "helper")
    rc, _, _ = _go(w.bob.runner, command, "--force")
    assert rc == 0
    assert EDIT not in w.text("helper")
    assert "helper v1" in w.text("helper")


# ===========================================================================
# Lines 07 and 08: row 4 match, record lags files that are a render of the lock's pin.
# Target behaviour; expected to fail today.
# ===========================================================================


@pytest.mark.parametrize("store", ["warm", "cold"])
@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_status_reads_installed_when_the_lock_pin_is_stored(team, tmp_path, unique_repo, model, store):
    # research_install_record_cache_test_matrix.md M07a, M08a
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    runner = w.warm() if store == "warm" else w.cold()
    snap = _snap(w, "helper", "solo", "extra")
    rc, doc, stderr = _go(runner, "status", offline=True)
    states = {r["name"]: r["state"] for r in doc["items"] if r["kind"] == "skill"}
    assert rc == 0
    if store == "warm":
        assert states == {"helper": "installed", "solo": "installed", "extra": "installed"}
        # Part B (M07b): status records the proven pins.
        _assert_recorded_then_silent(w, runner, snap, states)
    else:
        assert_unchanged(snap)
        assert states["helper"] == "modified"
        assert len(_lines_naming(stderr, "grim install")) == 1


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_install_re_pins_and_exits_zero(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M07c, M08c
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    rc, doc, _ = _go(w.warm(), "install")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["status"] == "unchanged"
    assert rows["solo"]["status"] == "unchanged"
    assert "helper v2" in w.text("helper")
    assert _record_pins(w) == _lock_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_bare_update_re_pins_then_rolls_forward(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M07d, M08d
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    _roll_c(w)
    rc, doc, _ = _go(w.warm(), "update")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["action"] == "updated"
    assert rows["extra"]["action"] == "removed"
    assert "helper v3" in w.text("helper")
    assert not (w.skill_dir("helper") / "ref.md").exists()
    assert not w.skill_dir("extra").exists()
    assert _record_pins(w) == _lock_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_named_update_re_pins_the_closure_only(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M07e, M08e
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    solo_before = _record_pins(w)["solo"]
    _roll_c(w)
    rc, doc, _ = _go(w.warm(), "update", "stack")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["action"] == "updated"
    assert rows["extra"]["action"] == "removed"
    assert "solo" not in rows or rows["solo"]["action"] == "unchanged"
    assert _record_pins(w)["solo"] == solo_before


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_uninstall_deletes_files(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M07f, M08f
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    rc, doc, _ = _go(w.warm(), "uninstall", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "uninstalled"
    assert not w.skill_dir("solo").exists()


@pytest.mark.parametrize("store", ["warm", "cold"])
@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_remove_then_update_prunes_only_when_proven(team, tmp_path, unique_repo, model, store):
    # research_install_record_cache_test_matrix.md M07g, M08g
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    runner = w.warm() if store == "warm" else w.cold()
    rc, doc, _ = _go(runner, "remove", "skill", "solo", offline=True)
    assert rc == 0
    assert doc["status"] == "removed"
    # The store is cold at `remove` only; an offline update on it stops at the tag resolve.
    rc, doc, _ = _go(runner, "update", offline=store == "warm")
    if store == "warm":
        assert rc == 0
        assert _rows(doc)["solo"]["action"] == "removed"
        assert not w.skill_dir("solo").exists()
    else:
        assert _rows(doc)["solo"]["action"] == "kept-modified"
        assert w.skill_dir("solo").is_dir()


@pytest.mark.parametrize("command", ["install", "update"])
@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_force_overwrites_identical_bytes(team, tmp_path, unique_repo, model, command):
    # research_install_record_cache_test_matrix.md M07h, M08h
    w = _world(team, tmp_path, unique_repo, model)
    _row4(w)
    rc, _, _ = _go(w.warm(), command, "--force")
    assert rc == 0
    assert "helper v2" in w.text("helper")
    assert _record_pins(w) == _lock_pins(w)


def test_lagging_record_uninstall_removes_a_drifted_mcp_entry(unique_repo, grim_at, project_dir, registry):
    # research_install_record_cache_test_matrix.md M07f (MCP half)
    descriptor = 'description = "t"\n\n[server]\ntransport = "stdio"\ncommand = "{cmd}"\nargs = ["mcp"]\n'
    runner = grim_at(project_dir)
    src = project_dir / "src"
    src.mkdir()
    for version, cmd in (("1.0.0", "grim"), ("1.0.1", "grim2")):
        (src / "d.toml").write_text(descriptor.format(cmd=cmd), encoding="utf-8", newline="\n")
        runner.json("release", str(src / "d.toml"), f"{registry}/{unique_repo}/mcp/m:{version}", "--kind", "mcp")
    write_config(project_dir)
    runner.json("add", "--no-install", f"{registry}/{unique_repo}/mcp/m:1.0.0")
    runner.run("install")
    # The lock moves to 1.0.1 and the config file already holds that entry (a pull).
    config = project_dir / ".mcp.json"
    doc = json.loads(config.read_text(encoding="utf-8"))
    (name,) = doc["mcpServers"]
    doc["mcpServers"][name]["command"] = "grim2"
    config.write_text(json.dumps(doc), encoding="utf-8", newline="\n")
    text = (project_dir / "grimoire.toml").read_text(encoding="utf-8").replace(":1.0.0", ":1.0.1")
    (project_dir / "grimoire.toml").write_text(text, encoding="utf-8", newline="\n")
    runner.run("lock")
    result = runner.run("uninstall", "mcp", name, check=False)
    assert result.returncode == 0
    assert name not in json.loads(config.read_text(encoding="utf-8")).get("mcpServers", {})


# ===========================================================================
# Lines 09 and 10: row 4 no match, files match neither the record nor the lock.
# Target behaviour; expected to fail today.
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_status_reads_modified_and_records_the_proven_nodes(
    team, tmp_path, unique_repo, model
):
    # research_install_record_cache_test_matrix.md M09a, M10a
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "helper")
    snap = _snap(w, "helper", "solo", "extra")
    helper = _record(w, "helper")
    rc, doc, _ = _go(w.warm(), "status", offline=True)
    assert rc == 0
    assert _rows(doc)["helper"]["state"] == "modified"
    assert _rows(doc)["solo"]["state"] == "installed"
    # Part B (M09b): the proven `solo` is recorded; `helper`'s record is left as it was.
    assert _record(w, "helper") == helper
    _assert_recorded_then_silent(w, w.warm(), snap, {"helper": "modified", "solo": "installed", "extra": "installed"})


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_install_refuses_and_keeps_the_record(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M09c, M10c
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "helper")

    def helper_record() -> dict:
        doc = json.loads(w.state_path.read_text(encoding="utf-8"))
        return next(r for r in doc["records"] if r["name"] == "helper")

    before = helper_record()
    outputs = state_snapshot(w.skill_dir("helper") / "SKILL.md")
    rc, doc, _ = _go(w.warm(), "install")
    assert rc == 65
    assert doc["error"]["reason"] == "modified"
    # Only the edited node's record is frozen: a matching sibling re-pins.
    assert helper_record() == before
    assert_unchanged(outputs)


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_bare_update_refuses_shipped_and_keeps_dropped(
    team, tmp_path, unique_repo, model
):
    # research_install_record_cache_test_matrix.md M09d, M10d
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "helper")
    _edit(w, "extra")
    _roll_c(w)
    rc, doc, stderr = _go(w.warm(), "update")
    rows = _rows(doc)
    assert rc == 65
    assert rows["helper"]["refused"] is True
    assert rows["extra"]["action"] == "kept-modified"
    assert w.skill_dir("extra").is_dir()
    assert len(_lines_naming(stderr, "extra")) <= 1


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_named_update_refuses_inside_the_closure(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M09e, M10e
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "helper", "solo")
    _edit(w, "extra")
    _roll_c(w)
    rc, doc, _ = _go(w.warm(), "update", "stack")
    rows = _rows(doc)
    assert rc == 65
    assert rows["helper"]["refused"] is True
    assert rows["extra"]["action"] == "kept-modified"
    assert "solo" not in rows or rows["solo"]["refused"] is False


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_uninstall_deletes_the_files(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M09f, M10f
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "solo")
    rc, doc, _ = _go(w.warm(), "uninstall", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "uninstalled"
    assert not w.skill_dir("solo").exists()


@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_remove_then_update_keeps_them(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M09g, M10g
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "solo")
    rc, doc, _ = _go(w.warm(), "remove", "skill", "solo")
    assert rc == 0
    rc, doc, _ = _go(w.warm(), "update")
    assert rc == 0
    assert _rows(doc)["solo"]["action"] == "kept-modified"
    assert w.skill_dir("solo").is_dir()


@pytest.mark.parametrize("command", ["install", "update"])
@pytest.mark.parametrize("model", MODELS)
def test_lagging_record_with_foreign_files_force_overwrites(team, tmp_path, unique_repo, model, command):
    # research_install_record_cache_test_matrix.md M09h, M10h
    w = _world(team, tmp_path, unique_repo, model)
    _row4_nomatch(w, "helper")
    rc, _, _ = _go(w.warm(), command, "--force")
    assert rc == 0
    assert "helper v2" in w.text("helper")
    assert EDIT not in w.text("helper")


# ===========================================================================
# Lines 11 and 12: row 5 match, no record, files are a render of the lock's pin.
# Target behaviour; expected to fail today where it differs.
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_no_record_status_reads_installed_and_records_every_proven_node(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M11a, M12a
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    snap = _snap(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.warm(), "status", offline=True)
    assert rc == 0
    # Part B (M11b): status adopts every proven render and records it.
    assert {r["state"] for r in doc["items"] if r["kind"] == "skill"} == {"installed"}
    assert _record_pins(w) == _lock_pins(w)
    _assert_recorded_then_silent(w, w.warm(), snap, {"helper": "installed", "solo": "installed", "extra": "installed"})


@pytest.mark.parametrize("model", MODELS)
def test_no_record_install_adopts_matching_files(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M11c, M12c
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    rc, doc, _ = _go(w.warm(), "install")
    assert rc == 0
    assert {r["status"] for r in doc["items"]} == {"unchanged"}
    assert _record_pins(w) == _lock_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_no_record_bare_update_removes_dropped_files_it_proves(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M11d, M12d
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    _roll_c(w)
    rc, doc, _ = _go(w.warm(), "update")
    rows = _rows(doc)
    assert rc == 0
    assert rows["helper"]["action"] == "updated"
    assert rows["extra"]["action"] == "removed"
    assert not w.skill_dir("extra").exists()
    assert not (w.skill_dir("helper") / "ref.md").exists()
    assert "helper v3" in w.text("helper")


@pytest.mark.parametrize("model", MODELS)
def test_no_record_named_update_proves_the_closure_only(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M11e, M12e
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    _roll_c(w)
    rc, doc, _ = _go(w.warm(), "update", "stack")
    rows = _rows(doc)
    assert rc == 0
    assert rows["extra"]["action"] == "removed"
    assert not w.skill_dir("extra").exists()
    assert "solo v2" in w.text("solo")
    assert "solo" not in _record_pins(w)


@pytest.mark.parametrize("store", ["warm", "cold"])
@pytest.mark.parametrize("model", MODELS)
def test_no_record_uninstall_deletes_only_what_it_proves(team, tmp_path, unique_repo, model, store):
    # research_install_record_cache_test_matrix.md M11f, M12f
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    runner = w.warm() if store == "warm" else w.cold()
    rc, doc, stderr = _go(runner, "uninstall", "skill", "solo", offline=True)
    assert rc == 0
    assert doc["status"] == "uninstalled"
    if store == "warm":
        assert not w.skill_dir("solo").exists()
    else:
        assert w.skill_dir("solo").is_dir()
        assert _lines_naming(stderr, "solo")


@pytest.mark.parametrize("store", ["warm", "cold"])
@pytest.mark.parametrize("model", MODELS)
def test_no_record_remove_then_update_prunes_only_what_it_proves(team, tmp_path, unique_repo, model, store):
    # research_install_record_cache_test_matrix.md M11g, M12g
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    runner = w.warm() if store == "warm" else w.cold()
    rc, doc, removed = _go(runner, "remove", "skill", "solo", offline=True)
    assert rc == 0
    assert doc["status"] == "removed"
    # The store is cold at `remove` only; an offline update on it stops at the tag resolve.
    rc, doc, _ = _go(runner, "update", offline=store == "warm")
    if store == "warm":
        assert rc == 0
        assert not w.skill_dir("solo").exists()
    else:
        assert w.skill_dir("solo").is_dir()
        # Unproven at `remove`, solo leaves the lock with no record: `remove` names it.
        assert _lines_naming(removed, "solo")
        (line,) = [line for line in _lines_naming(removed, "'solo'") if "on disk" in line]
        assert "skill" in line
        assert ".claude/skills/solo" in line.replace("\\", "/")
        assert (w.skill_dir("solo") / "SKILL.md").is_file()
        assert not w.state_path.exists() or "solo" not in _record_pins(w)


@pytest.mark.parametrize("case", ["install", "update", "update-unproven"])
@pytest.mark.parametrize("model", MODELS)
def test_no_record_force_adopts_on_install_and_removes_only_proven_drops_on_update(
    team, tmp_path, unique_repo, model, case
):
    # research_install_record_cache_test_matrix.md M11h, M12h
    w = _world(team, tmp_path, unique_repo, model)
    _row5(w)
    if case == "install":
        rc, doc, _ = _go(w.warm(), "install", "--force")
        assert rc == 0
        assert _record_pins(w) == _lock_pins(w)
        return
    if case == "update-unproven":
        _edit(w, "extra")
    _roll_c(w)
    rc, doc, stderr = _go(w.warm(), "update", "--force")
    assert rc == 0
    if case == "update":
        assert not w.skill_dir("extra").exists()
    else:
        assert w.skill_dir("extra").is_dir()
        assert _lines_naming(stderr, "extra")


# ===========================================================================
# Lines 13 and 14: row 5 no match, no record, files are not a render of the lock's pin.
# ===========================================================================


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_status_reads_missing_and_records_the_proven_nodes(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M13a, M14a
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "helper")
    snap = _snap(w, "helper", "solo", "extra")
    rc, doc, _ = _go(w.warm(), "status", offline=True)
    assert rc == 0
    assert _rows(doc)["helper"]["state"] == "missing"
    # Part B (M13b): the proven `solo` and `extra` are recorded; `helper` gets no record.
    assert "helper" not in _record_pins(w)
    _assert_recorded_then_silent(w, w.warm(), snap, {"helper": "missing", "solo": "installed", "extra": "installed"})


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_install_refuses_untracked(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M13c, M14c
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "helper")
    rc, doc, _ = _go(w.warm(), "install")
    assert rc == 65
    assert doc["error"]["reason"] == "untracked-destination"
    assert not w.state_path.exists() or "helper" not in _record_pins(w)


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_bare_update_refuses_shipped_and_names_dropped(
    team, tmp_path, unique_repo, model
):
    # research_install_record_cache_test_matrix.md M13d, M14d
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "helper")
    _edit(w, "extra")
    _roll_c(w)
    rc, doc, stderr = _go(w.warm(), "update")
    assert rc == 65
    assert _rows(doc)["helper"]["refused"] is True
    assert w.skill_dir("extra").is_dir()
    assert _lines_naming(stderr, "extra")


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_named_update_refuses_inside_the_closure(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M13e, M14e
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "helper", "solo")
    _edit(w, "extra")
    _roll_c(w)
    rc, doc, stderr = _go(w.warm(), "update", "stack")
    rows = _rows(doc)
    assert rc == 65
    assert rows["helper"]["refused"] is True
    assert w.skill_dir("extra").is_dir()
    assert _lines_naming(stderr, "extra")
    assert "solo" not in rows or rows["solo"]["refused"] is False


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_uninstall_leaves_them_and_names_them(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M13f, M14f
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "solo")
    rc, doc, stderr = _go(w.warm(), "uninstall", "skill", "solo")
    assert rc == 0
    assert doc["status"] == "uninstalled"
    assert w.skill_dir("solo").is_dir()
    assert _lines_naming(stderr, "solo")


@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_remove_then_update_leaves_them_named(team, tmp_path, unique_repo, model):
    # research_install_record_cache_test_matrix.md M13g, M14g
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "solo")
    rc, _, stderr = _go(w.warm(), "remove", "skill", "solo")
    assert rc == 0
    # Neither lock nor any record holds solo after `remove`, so `remove` names it.
    (line,) = [line for line in _lines_naming(stderr, "'solo'") if "on disk" in line]
    assert "skill" in line
    assert ".claude/skills/solo" in line.replace("\\", "/")
    rc, _, _ = _go(w.warm(), "update")
    assert w.skill_dir("solo").is_dir()
    assert rc == 0
    assert w.text("solo").endswith(EDIT)
    assert not w.state_path.exists() or "solo" not in _record_pins(w)


@pytest.mark.parametrize("command", ["install", "update"])
@pytest.mark.parametrize("model", MODELS)
def test_no_record_with_foreign_files_force_overwrites_on_install_and_keeps_dropped_on_update(
    team, tmp_path, unique_repo, model, command
):
    # research_install_record_cache_test_matrix.md M13h, M14h
    w = _world(team, tmp_path, unique_repo, model)
    _row5_nomatch(w, "helper")
    if command == "install":
        rc, _, _ = _go(w.warm(), "install", "--force")
        assert rc == 0
        assert EDIT not in w.text("helper")
        assert "helper v2" in w.text("helper")
        return
    _edit(w, "extra")
    _roll_c(w)
    rc, _, stderr = _go(w.warm(), "update", "--force")
    assert rc == 0
    assert w.skill_dir("extra").is_dir()
    assert _lines_naming(stderr, "extra")
