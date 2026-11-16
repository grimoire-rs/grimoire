# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim install`` over copied and moved workspaces, and over differences that are not edits.

A copied or renamed workspace keeps its records, because they are anchored at
the workspace. A render copied in by hand is adopted only when its bytes are
what this lock renders. Mode bits and mtimes are not part of a footprint; CRLF
line endings, an added newline and a symbolic link are differences no proof
accepts. A committed record grants nothing by itself: the files decide. Install
halves of ``research_install_record_cache_test_matrix.md`` S23, S25, S29, E01,
E02, E03, E06, E07, E08, E17, E18 and E23.

World: a skill ``moving`` and a rule ``style`` whose tags move from v1 to v2,
and a skill ``steady`` whose tag never moves.
"""

from __future__ import annotations

import json
import os
import shutil
import stat
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.assertions import assert_not_exists, assert_symlink_exists
from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_HOST, retag
from src.runner import GrimRunner
from src.scenarios import (
    Team,
    assert_unchanged,
    cold_runner,
    fresh_runner,
    git,
    move_tag,
    render_at,
    state_snapshot,
)

NAMES = ("moving", "style", "steady")
# A past instant no install can stamp, so a kept mtime proves no rewrite.
PAST = 1_000_000_000

no_links = pytest.mark.skipif(
    sys.platform == "win32",
    reason="symbolic links need a privilege Windows runners lack",
)
no_modes = pytest.mark.skipif(
    sys.platform == "win32", reason="Windows has no POSIX mode bits to change"
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


def _declare(root: Path, repo: str) -> None:
    write_config(
        root,
        skills={
            "moving": f"{REGISTRY_HOST}/{repo}/moving:rolling",
            "steady": f"{REGISTRY_HOST}/{repo}/steady:stable",
        },
        rules={"style": f"{REGISTRY_HOST}/{repo}/style:rolling"},
    )


# ---------------------------------------------------------------------------
# A project-scope workspace and what grim keeps in it
# ---------------------------------------------------------------------------


@dataclass
class Place:
    runner: GrimRunner

    @property
    def root(self) -> Path:
        assert self.runner.cwd is not None
        return self.runner.cwd

    @property
    def lock(self) -> Path:
        return self.root / "grimoire.lock"

    @property
    def state(self) -> Path:
        return self.root / ".grimoire" / "state.json"

    def run(self, *args: str) -> tuple[int, dict]:
        """Run ``grim --format json`` and return ``(exit, parsed stdout)``."""
        result = self.runner.run(*args, format="json", check=False)
        try:
            doc = json.loads(result.stdout)
        except json.JSONDecodeError:
            doc = {}
        return result.returncode, doc

    def skill(self, name: str) -> Path:
        return self.root / ".claude" / "skills" / name

    def rule(self, name: str) -> Path:
        return self.root / ".claude" / "rules" / f"{name}.md"

    def outputs(self) -> list[Path]:
        files = [
            p
            for name in ("moving", "steady")
            for p in sorted(self.skill(name).rglob("*"))
            if p.is_file()
        ]
        return [*files, self.rule("style")]

    def contents(self) -> dict[Path, bytes]:
        return {path: path.read_bytes() for path in self.outputs()}


def _digest(pinned: str) -> str:
    return pinned.rsplit("@", 1)[1]


def _records(place: Place) -> dict[str, dict]:
    doc = json.loads(place.state.read_text(encoding="utf-8"))
    return {r["name"]: r for r in doc["records"]}


def _record_pins(place: Place) -> dict[str, str]:
    return {name: _digest(r["pinned"]) for name, r in _records(place).items()}


def _lock_rows(place: Place) -> dict[str, str]:
    doc = tomllib.loads(place.lock.read_text(encoding="utf-8"))
    return {
        row["name"]: row["pinned"]
        for row in [*doc.get("skill", []), *doc.get("rule", [])]
    }


def _lock_pins(place: Place) -> dict[str, str]:
    return {name: _digest(pinned) for name, pinned in _lock_rows(place).items()}


def _rows(doc: dict) -> dict[str, str]:
    return {row["name"]: row["status"] for row in doc.get("items", [])}


def _frozen(place: Place):
    """mtime and inode of every client output: a rewrite moves at least one."""
    paths = place.outputs()
    return state_snapshot(paths[0], *paths[1:])


def _assert_kept(place: Place, rc: int, doc: dict) -> None:
    """Exit 0, every row ``unchanged``, and the record at the lock's pin."""
    assert rc == 0, doc
    assert _rows(doc) == {name: "unchanged" for name in NAMES}
    assert _record_pins(place) == _lock_pins(place)


def _assert_refused(
    place: Place, rc: int, doc: dict, reason: str, before: dict[str, dict], *names: str
) -> None:
    assert rc == 65, doc
    assert doc["error"]["reason"] == reason
    after = _records(place) if place.state.exists() else {}
    assert {n: after.get(n) for n in names} == {n: before.get(n) for n in names}


def _machine(tmp_path: Path, repo: str) -> Place:
    """One machine, installed at v1."""
    work = tmp_path / "work"
    (work / ".claude").mkdir(parents=True)
    place = Place(fresh_runner(tmp_path, "machine", cwd=work))
    _publish_v1(repo)
    _declare(work, repo)
    assert place.run("lock")[0] == 0
    assert place.run("install")[0] == 0
    return place


def _alice_v1(t: Team, repo: str) -> None:
    _publish_v1(repo)
    _declare(t.alice.work, repo)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()


def _alice_v2(t: Team, repo: str) -> None:
    _publish_v2(repo)
    t.alice.runner.run("update")
    t.alice.commit_all("chore: v2")
    t.alice.push()


def _pulled(
    team: Callable[[str], Team],
    repo: str,
    before_pull: Callable[[Team], None] = lambda _: None,
) -> Team:
    """Bob installed v1; his pull brought Alice's v2 lock and outputs."""
    t = team("CO")
    _alice_v1(t, repo)
    t.bob.pull()
    t.bob.runner.run("install")
    _alice_v2(t, repo)
    before_pull(t)
    t.bob.pull()
    return t


# ---------------------------------------------------------------------------
# A workspace copied or moved with its state
# ---------------------------------------------------------------------------


def test_install_in_a_copy_of_the_workspace_and_its_state_writes_nothing(
    tmp_path, unique_repo
):
    # research_install_record_cache_test_matrix.md S23
    original = _machine(tmp_path, unique_repo)
    state = original.state.read_bytes()
    untouched = state_snapshot(original.state, *original.outputs())
    copy = tmp_path / "copy"
    shutil.copytree(original.root, copy, symlinks=True)
    place = Place(
        GrimRunner(original.runner.binary, original.runner.grim_home, cwd=copy)
    )
    outputs = _frozen(place)
    rc, doc = place.run("install")
    _assert_kept(place, rc, doc)
    assert_unchanged(outputs)
    # Records are anchored at the workspace, so the copy's state needs no rewrite.
    assert place.state.read_bytes() == state
    assert_unchanged(untouched)


def test_install_after_renaming_the_workspace_writes_nothing(tmp_path, unique_repo):
    # research_install_record_cache_test_matrix.md S25
    before = _machine(tmp_path, unique_repo)
    state = before.state.read_bytes()
    renamed = tmp_path / "renamed"
    before.root.rename(renamed)
    place = Place(
        GrimRunner(before.runner.binary, before.runner.grim_home, cwd=renamed)
    )
    outputs = _frozen(place)
    rc, doc = place.run("install")
    _assert_kept(place, rc, doc)
    assert_unchanged(outputs)
    assert place.state.read_bytes() == state


# ---------------------------------------------------------------------------
# A render copied in by hand from another project
# ---------------------------------------------------------------------------


def _skill_files(root: Path) -> dict[str, bytes]:
    return {
        p.relative_to(root).as_posix(): p.read_bytes()
        for p in sorted(root.rglob("*"))
        if p.is_file()
        and ".grimoire" not in p.parts
        and p.suffix != ".lock"
        and p.name != "grimoire.toml"
    }


@pytest.mark.parametrize(
    ("into_copilot", "moved", "partial", "expected"),
    [
        pytest.param(False, False, None, 0, id="same-pin"),
        pytest.param(True, False, None, 0, id="same-pin-in-another-clients-folder"),
        pytest.param(False, True, None, 65, id="other-pin"),
        pytest.param(
            False, False, [".claude/skills/moving/SKILL.md"], 65, id="partial"
        ),
    ],
)
def test_install_over_a_render_copied_in_by_hand(
    grim_at, project_dir, unique_repo, into_copilot, moved, partial, expected
):
    # research_install_record_cache_test_matrix.md S29
    repo = f"{unique_repo}/moving"
    v1 = make_artifact(repo, "skill", _moving("v1"), tag="rolling")
    render_at(grim_at, v1, project_dir, partial=partial)
    clients = ("claude",)
    if into_copilot:
        (project_dir / ".github").mkdir()
        shutil.move(
            project_dir / ".claude" / "skills", project_dir / ".github" / "skills"
        )
        clients = ("copilot",)
    if moved:
        move_tag(repo, _moving("v2"), tag="rolling", kind="skill")
    write_config(
        project_dir,
        skills={"moving": f"{REGISTRY_HOST}/{repo}:rolling"},
        options={"clients": json.dumps(list(clients))},
    )
    place = Place(grim_at(project_dir))
    assert place.run("lock")[0] == 0
    contents = _skill_files(project_dir)
    rc, doc = place.run("install")
    if expected == 0:
        assert rc == 0, doc
        assert _rows(doc) == {"moving": "unchanged"}
        assert _record_pins(place) == _lock_pins(place)
    else:
        assert rc == 65, doc
        assert doc["error"]["reason"] == "untracked-destination"
    assert _skill_files(project_dir) == contents


def test_install_over_a_render_of_the_same_digest_at_another_repository(
    grim_at, project_dir, unique_repo
):
    # research_install_record_cache_test_matrix.md S29: a verbatim skill matches,
    # a render that names its repository does not.
    upstream = {
        "moving": make_artifact(
            f"{unique_repo}/a/moving", "skill", _moving("v1"), tag="1"
        ),
        "style": make_artifact(f"{unique_repo}/a/style", "rule", _style("v1"), tag="1"),
    }
    mirror = {
        "moving": make_artifact(
            f"{unique_repo}/b/moving", "skill", _moving("v1"), tag="1"
        ),
        "style": make_artifact(f"{unique_repo}/b/style", "rule", _style("v1"), tag="1"),
    }
    assert {k: a.digest for k, a in upstream.items()} == {
        k: a.digest for k, a in mirror.items()
    }
    for artifact in upstream.values():
        render_at(grim_at, artifact, project_dir, clients=["opencode"])
    write_config(
        project_dir,
        skills={"moving": mirror["moving"].fq},
        rules={"style": mirror["style"].fq},
        options={"clients": json.dumps(["opencode"])},
    )
    place = Place(grim_at(project_dir))
    assert place.run("lock")[0] == 0
    rule = project_dir / ".opencode" / "rules" / "style.md"
    rendered = rule.read_bytes()
    rc, doc = place.run("install")
    assert rc == 65, doc
    assert doc["error"]["reason"] == "untracked-destination"
    assert rule.read_bytes() == rendered
    records = _records(place)
    assert "style" not in records
    assert _digest(records["moving"]["pinned"]) == mirror["moving"].digest
    assert f"{unique_repo}/b/moving" in records["moving"]["pinned"]


# ---------------------------------------------------------------------------
# Differences that are an edit: CRLF line endings, one more newline
# ---------------------------------------------------------------------------


def _autocrlf(t: Team) -> None:
    git(t.bob.work, "config", "core.autocrlf", "true")


def test_install_after_a_crlf_pull_refuses_and_keeps_the_record(team, unique_repo):
    # research_install_record_cache_test_matrix.md E01 (pull): no proof accepts CRLF.
    t = _pulled(team, unique_repo, _autocrlf)
    bob = Place(t.bob.runner)
    skill = bob.skill("moving") / "SKILL.md"
    assert b"moving v2\r\n" in skill.read_bytes()
    before = _records(bob)
    contents = bob.contents()
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, "modified", before, "moving", "style")
    assert bob.contents() == contents


def test_install_in_a_crlf_clone_refuses_the_untracked_files(team, unique_repo):
    # research_install_record_cache_test_matrix.md E01 (clone)
    t = team("CO")
    _autocrlf(t)
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = Place(t.bob.runner)
    assert b"moving v1\r\n" in (bob.skill("moving") / "SKILL.md").read_bytes()
    contents = bob.contents()
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, "untracked-destination", {}, "moving")
    assert bob.contents() == contents


def test_a_newline_an_editor_adds_after_a_pull_refuses(team, unique_repo):
    # research_install_record_cache_test_matrix.md E06: one byte is an edit.
    bob = Place(_pulled(team, unique_repo).bob.runner)
    rule = bob.rule("style")
    rule.write_bytes(rule.read_bytes() + b"\n")
    before = _records(bob)
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, "modified", before, "style")
    assert rule.read_bytes().endswith(b"v2\n\n")


# ---------------------------------------------------------------------------
# Differences that are not an edit: mode bits and mtimes
# ---------------------------------------------------------------------------


def _bob_after_a_pull(team, tmp_path, repo) -> Place:
    return Place(_pulled(team, repo).bob.runner)


def _installed_machine(team, tmp_path, repo) -> Place:
    return _machine(tmp_path, repo)


WORLDS = pytest.mark.parametrize(
    "world",
    [_installed_machine, _bob_after_a_pull],
    ids=["same-pin", "after-a-pull"],
)


@no_modes
@WORLDS
def test_an_exec_bit_on_an_output_is_not_an_edit(team, tmp_path, unique_repo, world):
    # research_install_record_cache_test_matrix.md E02
    place = world(team, tmp_path, unique_repo)
    script = place.skill("moving") / "ref.md"
    script.chmod(0o755)
    outputs = _frozen(place)
    rc, doc = place.run("install")
    _assert_kept(place, rc, doc)
    assert_unchanged(outputs)
    assert stat.S_IMODE(script.stat().st_mode) == 0o755


@WORLDS
def test_an_mtime_change_on_every_output_is_not_an_edit(
    team, tmp_path, unique_repo, world
):
    # research_install_record_cache_test_matrix.md E03
    place = world(team, tmp_path, unique_repo)
    for path in place.outputs():
        os.utime(path, (PAST, PAST))
    outputs = _frozen(place)
    rc, doc = place.run("install")
    _assert_kept(place, rc, doc)
    assert_unchanged(outputs)


# ---------------------------------------------------------------------------
# A symbolic link at the destination
# ---------------------------------------------------------------------------


def _link_rule(place: Place, target: Path, contents: bytes) -> None:
    """Replace ``style.md`` with a link to ``target``, which holds ``contents``."""
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(contents)
    place.rule("style").unlink()
    place.rule("style").symlink_to(target)


@no_links
def test_install_adopts_a_live_link_to_a_file_with_identical_bytes(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md E07: the gate adopts by bytes.
    t = team("CO")
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = Place(t.bob.runner)
    shared = tmp_path / "dotfiles" / "style.md"
    _link_rule(bob, shared, bob.rule("style").read_bytes())
    assert_symlink_exists(bob.rule("style"))
    rendered = shared.read_bytes()
    rc, doc = bob.run("install")
    _assert_kept(bob, rc, doc)
    assert bob.rule("style").read_bytes() == rendered
    assert shared.read_bytes() == rendered


@no_links
@pytest.mark.parametrize(
    ("inside", "reason"),
    [(True, "modified"), (False, "anchor-escape")],
    ids=["target-in-the-workspace", "target-outside-it"],
)
def test_a_link_to_the_pulled_bytes_is_never_proven(
    team, unique_repo, tmp_path, inside, reason
):
    # research_install_record_cache_test_matrix.md E07: the proof never proves a link.
    # A target outside the workspace is refused earlier, by the gate's containment.
    bob = Place(_pulled(team, unique_repo).bob.runner)
    shared = (bob.root if inside else tmp_path) / "dotfiles" / "style.md"
    _link_rule(bob, shared, bob.rule("style").read_bytes())
    pulled = shared.read_bytes()
    before = _records(bob)
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, reason, before, "style")
    assert_symlink_exists(bob.rule("style"))
    assert shared.read_bytes() == pulled


@no_links
def test_install_refuses_a_link_to_a_folder_and_keeps_its_target(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md E08
    t = team("CO")
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = Place(t.bob.runner)
    elsewhere = tmp_path / "elsewhere" / "moving"
    shutil.move(bob.skill("moving"), elsewhere)
    bob.skill("moving").symlink_to(elsewhere, target_is_directory=True)
    kept = _skill_files(elsewhere)
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, "untracked-destination", {}, "moving")
    assert_symlink_exists(bob.skill("moving"))
    assert _skill_files(elsewhere) == kept


@no_links
def test_install_refuses_a_dangling_link_and_writes_nothing_through_it(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md E08
    t = team("CO")
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = Place(t.bob.runner)
    nowhere = tmp_path / "nowhere" / "style.md"
    bob.rule("style").unlink()
    bob.rule("style").symlink_to(nowhere)
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, "untracked-destination", {}, "style")
    assert_symlink_exists(bob.rule("style"))
    assert_not_exists(nowhere)


@no_links
def test_a_recorded_folder_replaced_by_a_link_out_of_the_workspace_refuses(
    tmp_path, unique_repo
):
    # research_install_record_cache_test_matrix.md E08, over a record at the lock's pin
    place = _machine(tmp_path, unique_repo)
    elsewhere = tmp_path / "elsewhere" / "moving"
    shutil.move(place.skill("moving"), elsewhere)
    place.skill("moving").symlink_to(elsewhere, target_is_directory=True)
    kept = _skill_files(elsewhere)
    before = _records(place)
    rc, doc = place.run("install")
    _assert_refused(place, rc, doc, "anchor-escape", before, *NAMES)
    assert_symlink_exists(place.skill("moving"))
    assert _skill_files(elsewhere) == kept


# ---------------------------------------------------------------------------
# A pin that moved only in its tag, registry or repository
# ---------------------------------------------------------------------------


def test_a_tag_only_lock_change_installs_nothing(tmp_path, unique_repo):
    # research_install_record_cache_test_matrix.md E17: the tag is not part of a pin.
    place = _machine(tmp_path, unique_repo)
    steady = f"{unique_repo}/steady"
    retag(steady, "alias", _lock_pins(place)["steady"])
    toml = place.root / "grimoire.toml"
    toml.write_text(
        toml.read_text(encoding="utf-8").replace("steady:stable", "steady:alias"),
        encoding="utf-8",
        newline="\n",
    )
    assert place.run("lock")[0] == 0
    assert ":alias@" in _lock_rows(place)["steady"]
    outputs = _frozen(place)
    rc, doc = place.run("install")
    _assert_kept(place, rc, doc)
    assert_unchanged(outputs)
    # No render: a machine with an empty store installs it offline.
    cold = Place(cold_runner(place.runner, tmp_path, "cold"))
    rc, doc = cold.run("--offline", "install")
    _assert_kept(cold, rc, doc)
    assert_unchanged(outputs)


def test_the_same_digest_at_a_mirror_re_renders_only_its_provenance(
    tmp_path, unique_repo
):
    # research_install_record_cache_test_matrix.md E18
    work = tmp_path / "work"
    (work / ".claude").mkdir(parents=True)
    place = Place(fresh_runner(tmp_path, "machine", cwd=work))
    clients = json.dumps(["claude", "opencode"])
    pins = {}
    for side in ("a", "b"):
        pins[side] = {
            "moving": make_artifact(
                f"{unique_repo}/{side}/moving", "skill", _moving("v1"), tag="1"
            ),
            "style": make_artifact(
                f"{unique_repo}/{side}/style", "rule", _style("v1"), tag="1"
            ),
        }
    for side in ("a", "b"):
        write_config(
            work,
            skills={"moving": pins[side]["moving"].fq},
            rules={"style": pins[side]["style"].fq},
            options={"clients": clients},
        )
        assert place.run("lock")[0] == 0
        if side == "a":
            assert place.run("install")[0] == 0
            skill = (place.skill("moving") / "SKILL.md").read_bytes()
    rule = work / ".opencode" / "rules" / "style.md"
    assert f"{unique_repo}/a/style" in rule.read_text(encoding="utf-8")
    rc, doc = place.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"moving": "updated", "style": "updated"}
    assert f"{unique_repo}/b/style" in rule.read_text(encoding="utf-8")
    assert f"{unique_repo}/a/style" not in rule.read_text(encoding="utf-8")
    assert (place.skill("moving") / "SKILL.md").read_bytes() == skill
    records = _records(place)
    assert all(f"{unique_repo}/b/" in records[n]["pinned"] for n in ("moving", "style"))


# ---------------------------------------------------------------------------
# A committed state file whose hashes match nothing on disk
# ---------------------------------------------------------------------------

BOGUS = "sha256:" + "0" * 64


def _commit_hostile_state(t: Team, state: bytes) -> None:
    """Alice commits ``state`` with every recorded hash replaced, beside her outputs."""
    doc = json.loads(state)
    for record in doc["records"]:
        for output in record["outputs"]:
            output["content_hash"] = BOGUS
    path = t.alice.work / ".grimoire" / "state.json"
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8", newline="\n")
    git(t.alice.work, "add", "-f", ".grimoire/state.json")
    t.alice.commit_all("chore: commit the state file")
    t.alice.push()


@pytest.mark.parametrize(
    ("older", "extra_file", "re_pinned"),
    [
        pytest.param(False, False, (), id="at-the-lock-pin"),
        pytest.param(True, False, ("moving", "style"), id="at-an-older-pin"),
        pytest.param(True, True, ("style",), id="at-an-older-pin-beside-a-local-file"),
    ],
)
def test_a_committed_record_whose_hashes_match_nothing_grants_no_deletion(
    team, unique_repo, older, extra_file, re_pinned
):
    # research_install_record_cache_test_matrix.md E23: the files decide each node.
    # A node at the record's pin refuses at once; one at another pin re-pins only
    # where the files are the lock's render. Nothing is written either way.
    t = team("CO")
    _alice_v1(t, unique_repo)
    state = (t.alice.work / ".grimoire" / "state.json").read_bytes()
    if older:
        _alice_v2(t, unique_repo)
    _commit_hostile_state(t, state)
    t.bob.pull()
    bob = Place(t.bob.runner)
    assert {
        o["content_hash"] for r in _records(bob).values() for o in r["outputs"]
    } == {BOGUS}
    mine = bob.skill("moving") / "mine.md"
    if extra_file:
        mine.write_text("mine\n", encoding="utf-8", newline="\n")
    before = _records(bob)
    contents = bob.contents()
    outputs = _frozen(bob)
    rc, doc = bob.run("install")
    kept = [name for name in NAMES if name not in re_pinned]
    _assert_refused(bob, rc, doc, "modified", before, *kept)
    after = _records(bob)
    for name in re_pinned:
        assert _digest(after[name]["pinned"]) == _lock_pins(bob)[name]
        assert BOGUS not in {o["content_hash"] for o in after[name]["outputs"]}
    assert bob.contents() == contents
    assert_unchanged(outputs)
    assert mine.is_file() is extra_file
