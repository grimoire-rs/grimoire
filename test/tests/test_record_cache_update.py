# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Bare ``grim update`` over a record that lags the files, or no record at all.

The install target and the install state load before the lock is saved, so a
state that cannot be read stops the run with the lock as it was found. A bare
run then proves the files against the previous lock before the save: a record
that lagged a pull re-pins, a clone's files gain a record, and the install and
the prune act on what proved. Files no proof covers stay, named once. A named
run proves only what it may install, prune or force. Update halves of
``research_install_record_cache_test_matrix.md`` S02, S06, S08, S09, S14, S19,
S20, S46, S47, S61 and S63, plus ADR validation cases 5, 8 to 12, 17, 18 and 21.
"""

from __future__ import annotations

import json
import os
import shutil
from collections.abc import Callable
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, write_config
from src.oci_proxy import OciProxy
from src.registry import REGISTRY_HOST, fetch_manifest, retag, tag_digest
from src.runner import GrimRunner
from src.scenarios import (
    Team,
    allow_insecure,
    assert_unchanged,
    cold_runner,
    fresh_runner,
    git,
    move_tag,
    state_snapshot,
)

EDIT = "# local edit\n"

posix_only = pytest.mark.skipif(os.name == "nt", reason="needs POSIX directory modes")
not_root = pytest.mark.skipif(
    os.name != "nt" and os.geteuid() == 0,
    reason="root ignores directory modes",
)


# ---------------------------------------------------------------------------
# Observation
# ---------------------------------------------------------------------------


def _go(runner: GrimRunner, *args: str):
    """Run ``grim --format json`` and return ``(exit, parsed stdout, stderr)``."""
    result = runner.run(*args, format="json", check=False)
    try:
        doc = json.loads(result.stdout)
    except json.JSONDecodeError:
        doc = {}
    return result.returncode, doc, result.stderr


def _actions(doc: dict) -> dict[str, str]:
    return {row["name"]: row["action"] for row in doc.get("items", [])}


def _digest(pinned: str) -> str:
    return pinned.rsplit("@", 1)[1]


def _record_pins(work: Path) -> dict[str, str]:
    doc = json.loads((work / ".grimoire" / "state.json").read_text(encoding="utf-8"))
    return {r["name"]: _digest(r["pinned"]) for r in doc["records"]}


def _lock_pins(work: Path) -> dict[str, str]:
    doc = tomllib.loads((work / "grimoire.lock").read_text(encoding="utf-8"))
    rows = [*doc.get("skill", []), *doc.get("rule", [])]
    return {row["name"]: _digest(row["pinned"]) for row in rows}


def _skill(work: Path, name: str, client: str = "claude") -> Path:
    folder = {"claude": ".claude", "copilot": ".github"}[client]
    return work / folder / "skills" / name


def _text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _left_lines(stderr: str, token: str) -> list[str]:
    """The stderr lines naming ``token`` that say files, or an MCP entry, were left behind."""
    left = ("on disk", "left its entry in")
    return [
        line
        for line in stderr.splitlines()
        if token in line and any(m in line for m in left)
    ]


# ---------------------------------------------------------------------------
# World: Alice declares, installs, commits and pushes; Bob pulls
# ---------------------------------------------------------------------------


def _shared(
    team: Callable[[str], Team], model: str, declare: Callable[[Path], None]
) -> Team:
    """Bob holds Alice's first commit and no install state of his own."""
    t = team(model)
    declare(t.alice.work)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    return t


def _alice_updates(t: Team, branch: str = "main") -> None:
    t.alice.runner.run("update")
    t.alice.commit_all("chore: update")
    git(t.alice.work, "push", "-q", "origin", branch)


def _options(clients: tuple[str, ...]) -> dict[str, str] | None:
    return {"clients": json.dumps(list(clients))} if clients else None


def _kit(version: str) -> dict[str, str]:
    """A skill whose first version alone ships ``ref.md``."""
    files = {"kit/SKILL.md": f"# kit {version}\n"}
    if version == "v1":
        files["kit/ref.md"] = "ref v1\n"
    return files


def _declare_kit(repo: str, clients: tuple[str, ...] = ()) -> Callable[[Path], None]:
    def declare(work: Path) -> None:
        make_artifact(f"{repo}/kit", "skill", _kit("v1"), tag="rolling")
        write_config(
            work,
            skills={"kit": f"{REGISTRY_HOST}/{repo}/kit:rolling"},
            options=_options(clients),
        )

    return declare


def _move_kit(repo: str, version: str) -> None:
    move_tag(f"{repo}/kit", _kit(version), tag="rolling")


BOTH = [("skill", "helper", "helper:v1"), ("skill", "extra", "extra:v1")]
HELPER_ONLY = [("skill", "helper", "helper:v1")]


def _bundle(repo: str, members: list[tuple[str, str, str]], *, move: bool) -> None:
    """Publish ``stack`` with ``members``; a move keeps the manifest it leaves tagged."""
    if move:
        old = tag_digest(f"{repo}/stack", "rolling")
        retag(f"{repo}/stack", f"keep-{old.removeprefix('sha256:')[:12]}", old)
    refs = [
        (kind, name, f"{REGISTRY_HOST}/{repo}/{art}") for kind, name, art in members
    ]
    make_bundle(f"{repo}/stack", refs, tag="rolling")


def _declare_stack(
    repo: str, members: list[tuple[str, str, str]], clients: tuple[str, ...] = ()
) -> Callable[[Path], None]:
    def declare(work: Path) -> None:
        for name in ("helper", "extra"):
            make_artifact(
                f"{repo}/{name}", "skill", {f"{name}/SKILL.md": f"# {name}\n"}, tag="v1"
            )
        make_artifact(f"{repo}/style", "rule", {"style.md": "# style\n"}, tag="v1")
        _bundle(repo, members, move=False)
        write_config(
            work,
            bundles={"stack": f"{REGISTRY_HOST}/{repo}/stack:rolling"},
            options=_options(clients),
        )

    return declare


# ---------------------------------------------------------------------------
# Ordering: nothing is read after the lock is saved
# ---------------------------------------------------------------------------


def test_update_with_an_unreadable_state_keeps_the_lock(
    grim_at, project_dir: Path, unique_repo: str
):
    # adr_output_ownership_without_state.md validation case 17
    make_artifact(
        f"{unique_repo}/kit", "skill", {"kit/SKILL.md": "# kit v1\n"}, tag="rolling"
    )
    write_config(
        project_dir, skills={"kit": f"{REGISTRY_HOST}/{unique_repo}/kit:rolling"}
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    move_tag(f"{unique_repo}/kit", {"kit/SKILL.md": "# kit v2\n"}, tag="rolling")
    (project_dir / ".grimoire" / "state.json").write_text(
        "not json", encoding="utf-8", newline="\n"
    )
    lock = (project_dir / "grimoire.lock").read_bytes()

    result = runner.run("update", check=False)

    assert result.returncode == 74, result.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == lock


def test_update_over_a_lock_with_conflict_markers_writes_nothing(team, unique_repo):
    # research_install_record_cache_test_matrix.md S14
    t = _shared(team, "CO", _declare_kit(unique_repo))
    t.bob.runner.run("install")
    work = t.bob.work
    lock_path = work / "grimoire.lock"
    text = _text(lock_path)
    lock_path.write_text(
        f"<<<<<<< ours\n{text}=======\n{text}>>>>>>> theirs\n",
        encoding="utf-8",
        newline="\n",
    )
    lock = lock_path.read_bytes()
    snap = state_snapshot(work / ".grimoire" / "state.json")

    rc, _, stderr = _go(t.bob.runner, "update")

    assert rc == 78, stderr
    assert lock_path.read_bytes() == lock
    assert_unchanged(snap)


@posix_only
@not_root
def test_an_update_whose_proven_records_cannot_be_saved_keeps_the_lock(
    team, unique_repo
):
    # The proof destroys its evidence at the lock save, so a failed save of
    # what proved stops the run before it.
    t = _shared(team, "CO", _declare_kit(unique_repo))
    work = t.bob.work
    _move_kit(unique_repo, "v2")
    lock = (work / "grimoire.lock").read_bytes()
    state_dir = work / ".grimoire"
    state_dir.mkdir()
    state_dir.chmod(0o500)
    try:
        rc, _, stderr = _go(t.bob.runner, "update")
    finally:
        state_dir.chmod(0o700)

    assert rc == 77, stderr
    assert (work / "grimoire.lock").read_bytes() == lock
    assert "kit v1" in _text(_skill(work, "kit") / "SKILL.md")


@pytest.mark.parametrize("where", ["config", "flag"])
def test_an_update_naming_an_unknown_client_keeps_the_lock(team, unique_repo, where):
    # The config load refuses `[options].clients`; only `--client` reaches the
    # client set the update reads ahead of its lock save.
    t = _shared(team, "CO", _declare_kit(unique_repo))
    work = t.bob.work
    t.bob.runner.run("install")
    _move_kit(unique_repo, "v2")
    flags = ["--client", "nope"] if where == "flag" else []
    if where == "config":
        write_config(
            work,
            skills={"kit": f"{REGISTRY_HOST}/{unique_repo}/kit:rolling"},
            options=_options(("claude", "nope")),
        )
    lock = (work / "grimoire.lock").read_bytes()

    rc, _, stderr = _go(t.bob.runner, "update", *flags)

    assert rc == 78, stderr
    assert "nope" in stderr
    assert (work / "grimoire.lock").read_bytes() == lock


# ---------------------------------------------------------------------------
# Row 4: a record that lags files of the previous lock's pin
# ---------------------------------------------------------------------------


def test_a_pulled_update_then_a_third_version_rolls_forward(team, unique_repo):
    # research_install_record_cache_test_matrix.md S06; ADR validation case 5
    t = _shared(team, "CO", _declare_kit(unique_repo))
    t.bob.runner.run("install")
    _move_kit(unique_repo, "v2")
    _alice_updates(t)
    t.bob.pull()
    work = t.bob.work
    assert _record_pins(work) != _lock_pins(work)
    _move_kit(unique_repo, "v3")

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["kit"] == "updated"
    assert "kit v3" in _text(_skill(work, "kit") / "SKILL.md")
    assert _record_pins(work) == _lock_pins(work)


@pytest.mark.parametrize("model", ["CO", "GO"])
def test_update_after_a_branch_checkout_proves_or_re_renders(team, unique_repo, model):
    # research_install_record_cache_test_matrix.md S61
    t = _shared(team, model, _declare_kit(unique_repo))
    t.bob.runner.run("install")
    git(t.alice.work, "checkout", "-q", "-b", "next")
    _move_kit(unique_repo, "v2")
    _alice_updates(t, "next")
    work = t.bob.work
    git(work, "fetch", "-q", "origin")
    git(work, "checkout", "-q", "next")
    skill_md = _skill(work, "kit") / "SKILL.md"
    snap = state_snapshot(skill_md)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert "kit v2" in _text(skill_md)
    assert not (_skill(work, "kit") / "ref.md").exists()
    assert _record_pins(work) == _lock_pins(work)
    if model == "CO":
        assert _actions(doc)["kit"] == "unchanged"
        assert_unchanged(snap)


# ---------------------------------------------------------------------------
# Rows 1 and 2: records that match their files read no content
# ---------------------------------------------------------------------------


def _pulled_without_extra(team: Callable[[str], Team], repo: str) -> Team:
    """Bob's record holds ``extra``; his pull took it out of the lock and the tree."""
    t = _shared(team, "CO", _declare_stack(repo, BOTH))
    t.bob.runner.run("install")
    _bundle(repo, HELPER_ONLY, move=True)
    _alice_updates(t)
    t.bob.pull()
    assert not _skill(t.bob.work, "extra").exists()
    assert "extra" in _record_pins(t.bob.work)
    return t


def test_a_pulled_bundle_without_a_member_prunes_it_silently(team, unique_repo):
    # research_install_record_cache_test_matrix.md S08 (bare update)
    t = _pulled_without_extra(team, unique_repo)
    work = t.bob.work

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["extra"] == "removed"
    assert "extra" not in stderr
    assert "extra" not in _record_pins(work)


def test_a_named_update_after_that_pull_leaves_the_record_and_names_it_once(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S08 (named update). The
    # previous lock no longer holds the member, so a named run neither proves nor
    # prunes it: the record stays and the orphan warning names it.
    t = _pulled_without_extra(team, unique_repo)
    work = t.bob.work

    rc, doc, stderr = _go(t.bob.runner, "update", "stack")

    assert rc == 0, stderr
    assert "extra" not in _actions(doc)
    (line,) = [line for line in stderr.splitlines() if "extra" in line]
    assert "uninstall" in line
    assert "extra" in _record_pins(work)


def test_a_pulled_remove_prunes_the_record_and_a_clone_leaves_it_unnamed(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md S09
    def declare(work: Path) -> None:
        refs = {}
        for name in ("gone", "kept"):
            make_artifact(
                f"{unique_repo}/{name}",
                "skill",
                {f"{name}/SKILL.md": f"# {name}\n"},
                tag="v1",
            )
            refs[name] = f"{REGISTRY_HOST}/{unique_repo}/{name}:v1"
        write_config(work, skills=refs)

    t = _shared(team, "CO", declare)
    t.bob.runner.run("install")
    t.alice.runner.run("remove", "skill", "gone")
    t.alice.commit_all("chore: remove gone")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    assert _skill(work, "gone").is_dir()

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["gone"] == "removed"
    assert not _skill(work, "gone").exists()

    # The ADR's stated limit: no lock a clone holds names the node any more.
    clone = tmp_path / "carol" / "work"
    clone.parent.mkdir()
    git(tmp_path, "clone", "-q", str(t.origin), str(clone))
    carol = fresh_runner(tmp_path, "carol-machine", cwd=clone)
    rc, doc, stderr = _go(carol, "update")
    assert rc == 0, stderr
    assert _skill(clone, "gone").is_dir()
    assert "gone" not in _actions(doc)
    assert "'gone'" not in stderr


def test_gitignored_outputs_after_a_pull_re_render_on_update(team, unique_repo):
    # research_install_record_cache_test_matrix.md S19 (update half)
    t = _shared(team, "GO", _declare_kit(unique_repo))
    t.bob.runner.run("install")
    _move_kit(unique_repo, "v2")
    _alice_updates(t)
    t.bob.pull()
    work = t.bob.work
    assert "kit v1" in _text(_skill(work, "kit") / "SKILL.md")
    rc, doc, _ = _go(t.bob.runner, "status")
    assert {row["name"]: row["state"] for row in doc["items"]}["kit"] == "outdated"

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert "kit v2" in _text(_skill(work, "kit") / "SKILL.md")
    assert not (_skill(work, "kit") / "ref.md").exists()
    assert _record_pins(work) == _lock_pins(work)
    assert _left_lines(stderr, "'kit'") == []


def test_a_record_that_lags_a_pull_sends_no_proof_request(
    team, unique_repo, oci_proxy, tmp_path
):
    # research_install_record_cache_test_matrix.md S19, S61 (gitignored outputs, row 2)
    proxy: OciProxy = oci_proxy("count")
    t = team("GO")
    for member in (t.alice, t.bob):
        allow_insecure(member.runner, proxy.host)
    make_artifact(f"{unique_repo}/kit", "skill", _kit("v1"), tag="rolling")
    write_config(
        t.alice.work, skills={"kit": f"{proxy.host}/{unique_repo}/kit:rolling"}
    )
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    work = t.bob.work
    _move_kit(unique_repo, "v2")
    _alice_updates(t)
    t.bob.pull()
    # Files and record at v1, the previous lock at v2: a proof would render v2.
    pulled = _lock_pins(work)["kit"]
    assert _record_pins(work)["kit"] != pulled
    assert "kit v1" in _text(_skill(work, "kit") / "SKILL.md")
    pulled_layer = fetch_manifest(f"{unique_repo}/kit", pulled)["layers"][0]["digest"]
    _move_kit(unique_repo, "v3")
    # A store that holds nothing, so a proof would have to fetch v2.
    cold = cold_runner(t.bob.runner, tmp_path, "cold")
    before = proxy.counts

    rc, _, stderr = _go(cold, "update")

    assert rc == 0, stderr
    sent = [path for path, n in proxy.counts.items() if n > before.get(path, 0)]
    assert [path for path in sent if pulled in path or pulled_layer in path] == []
    assert any("/kit/blobs/" in path for path in sent), sent
    assert "kit v3" in _text(_skill(work, "kit") / "SKILL.md")
    assert _record_pins(work) == _lock_pins(work)


def test_gitignored_outputs_re_render_after_each_branch_checkout(team, unique_repo):
    # research_install_record_cache_test_matrix.md S20
    t = _shared(team, "GO", _declare_kit(unique_repo))
    t.bob.runner.run("install")
    git(t.alice.work, "checkout", "-q", "-b", "next")
    _move_kit(unique_repo, "v2")
    _alice_updates(t, "next")
    work = t.bob.work
    git(work, "fetch", "-q", "origin")
    for ref, version in (("next", "v2"), ("main", "v1"), ("next", "v2")):
        git(work, "checkout", "-q", ref)
        rc, _, stderr = _go(t.bob.runner, "install")
        assert rc == 0, stderr
        assert f"kit {version}" in _text(_skill(work, "kit") / "SKILL.md")
        assert _record_pins(work) == _lock_pins(work)


# ---------------------------------------------------------------------------
# Row 5: a clone holds files and no record
# ---------------------------------------------------------------------------


def test_a_clone_update_drops_a_file_inside_a_skill(team, unique_repo):
    # research_install_record_cache_test_matrix.md S02; ADR validation case 8
    t = _shared(team, "CO", _declare_kit(unique_repo))
    work = t.bob.work
    assert not (work / ".grimoire").exists()
    _move_kit(unique_repo, "v2")

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["kit"] == "updated"
    assert not (_skill(work, "kit") / "ref.md").exists()
    assert "kit v2" in _text(_skill(work, "kit") / "SKILL.md")
    assert _record_pins(work) == _lock_pins(work)


def test_a_clone_update_removes_a_dropped_member_and_a_renamed_rule(team, unique_repo):
    # ADR validation case 9
    first = [*BOTH, ("rule", "old-style", "style:v1")]
    then = [*HELPER_ONLY, ("rule", "new-style", "style:v1")]
    t = _shared(team, "CO", _declare_stack(unique_repo, first))
    work = t.bob.work
    rules = work / ".claude" / "rules"
    assert (rules / "old-style.md").is_file()
    _bundle(unique_repo, then, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    actions = _actions(doc)
    assert actions["extra"] == "removed"
    assert actions["old-style"] == "removed"
    assert not _skill(work, "extra").exists()
    assert not (rules / "old-style.md").exists()
    assert (rules / "new-style.md").is_file()
    assert _record_pins(work) == _lock_pins(work)


@pytest.mark.parametrize("absent", [False, True], ids=["both-clients", "one-absent"])
def test_a_clone_update_at_a_carried_pin_writes_only_what_is_missing(
    team, unique_repo, absent
):
    # ADR validation case 10
    t = _shared(team, "CO", _declare_kit(unique_repo, ("claude", "copilot")))
    work = t.bob.work
    copilot = _skill(work, "kit", "copilot")
    assert copilot.is_dir()
    if absent:
        shutil.rmtree(copilot)
    snap = state_snapshot(_skill(work, "kit") / "SKILL.md")

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["kit"] == ("restored" if absent else "unchanged")
    if not absent:
        assert_unchanged(snap)
    assert (copilot / "SKILL.md").is_file()
    assert _record_pins(work) == _lock_pins(work)


def test_a_clone_update_removes_a_dropped_skill_only_where_it_proves(team, unique_repo):
    # ADR validation case 11
    t = _shared(team, "CO", _declare_stack(unique_repo, BOTH, ("claude", "copilot")))
    work = t.bob.work
    edited = _skill(work, "extra", "copilot") / "SKILL.md"
    edited.write_text(_text(edited) + EDIT, encoding="utf-8", newline="\n")
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update")

    assert rc == 0, stderr
    assert _actions(doc)["extra"] == "removed"
    assert not _skill(work, "extra").exists()
    assert _text(edited).endswith(EDIT)
    (line,) = _left_lines(stderr, "'extra'")
    assert "skill" in line
    assert ".github/skills/extra" in line.replace("\\", "/")
    assert ".claude/skills/extra" not in line.replace("\\", "/")
    assert "extra" not in _record_pins(work)


def test_a_forced_clone_update_removes_what_proves_and_names_what_does_not(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S46
    first = [*BOTH, ("rule", "old-style", "style:v1")]
    t = _shared(team, "CO", _declare_stack(unique_repo, first))
    work = t.bob.work
    rule = work / ".claude" / "rules" / "old-style.md"
    shipped = _skill(work, "helper") / "SKILL.md"
    for path in (rule, shipped):
        path.write_text(_text(path) + EDIT, encoding="utf-8", newline="\n")
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update", "--force")

    assert rc == 0, stderr
    assert _actions(doc)["extra"] == "removed"
    assert not _skill(work, "extra").exists()
    assert _text(rule).endswith(EDIT)
    (line,) = _left_lines(stderr, "'old-style'")
    assert "rule" in line
    assert ".claude/rules/old-style.md" in line.replace("\\", "/")
    assert EDIT not in _text(shipped)
    assert "old-style" not in _record_pins(work)


# ---------------------------------------------------------------------------
# Named runs: the proof stops where the run's reach stops
# ---------------------------------------------------------------------------


def test_a_named_clone_update_proves_its_closure_alone(team, unique_repo, oci_proxy):
    # research_install_record_cache_test_matrix.md S47
    proxy: OciProxy = oci_proxy("count")
    declare_stack = _declare_stack(unique_repo, BOTH)

    def declare(work: Path) -> None:
        declare_stack(work)
        make_artifact(
            f"{unique_repo}/solo", "skill", {"solo/SKILL.md": "# solo\n"}, tag="v1"
        )
        write_config(
            work,
            bundles={"stack": f"{REGISTRY_HOST}/{unique_repo}/stack:rolling"},
            skills={"solo": f"{proxy.host}/{unique_repo}/solo:v1"},
        )

    t = team("CO")
    for member in (t.alice, t.bob):
        allow_insecure(member.runner, proxy.host)
    declare(t.alice.work)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    _bundle(unique_repo, HELPER_ONLY, move=True)

    def solo_blob_reads() -> int:
        return sum(n for path, n in proxy.counts.items() if "/solo/blobs/" in path)

    before = solo_blob_reads()

    rc, doc, stderr = _go(t.bob.runner, "update", "stack")

    assert rc == 0, stderr
    assert _actions(doc)["extra"] == "removed"
    assert solo_blob_reads() == before
    assert "solo" not in _record_pins(work)
    assert "helper" in _record_pins(work)

    # The same files prove on a bare run, which reads solo's content to do it.
    rc, _, stderr = _go(t.bob.runner, "update")
    assert rc == 0, stderr
    assert solo_blob_reads() > before
    assert "solo" in _record_pins(work)


@pytest.mark.parametrize("recorded", [True, False], ids=["recorded", "clone"])
def test_a_named_update_names_an_edited_dropped_member_once(
    team, unique_repo, recorded
):
    # adr_output_ownership_without_state.md validation case 12
    t = _shared(team, "CO", _declare_stack(unique_repo, BOTH))
    if recorded:
        t.bob.runner.run("install")
    work = t.bob.work
    for name in ("helper", "extra"):
        path = _skill(work, name) / "SKILL.md"
        path.write_text(_text(path) + EDIT, encoding="utf-8", newline="\n")
    _bundle(unique_repo, HELPER_ONLY, move=True)

    rc, doc, stderr = _go(t.bob.runner, "update", "stack")

    assert rc == 65, stderr
    rows = {row["name"]: row for row in doc["items"]}
    assert rows["helper"]["refused"] is True
    assert _text(_skill(work, "extra") / "SKILL.md").endswith(EDIT)
    assert _text(_skill(work, "helper") / "SKILL.md").endswith(EDIT)
    lines = stderr.splitlines()
    assert len([line for line in lines if "extra" in line]) == 1, stderr
    assert len([line for line in lines if "helper" in line]) == 1, stderr
    assert not [line for line in lines if "outside the named closure" in line]
    if recorded:
        assert rows["extra"]["action"] == "kept-modified"
    else:
        (line,) = _left_lines(stderr, "'extra'")
        assert ".claude/skills/extra" in line.replace("\\", "/")


# ---------------------------------------------------------------------------
# What a run reads, and what it keeps when it fails
# ---------------------------------------------------------------------------


def test_an_update_over_cache_hits_sends_no_proof_request(
    team, unique_repo, oci_proxy, tmp_path
):
    # adr_output_ownership_without_state.md validation case 18
    proxy: OciProxy = oci_proxy("count")
    t = team("CO")
    for member in (t.alice, t.bob):
        allow_insecure(member.runner, proxy.host)
    refs = {}
    for name in ("kit", "solo"):
        make_artifact(
            f"{unique_repo}/{name}",
            "skill",
            {f"{name}/SKILL.md": f"# {name}\n"},
            tag="v1",
        )
        refs[name] = f"{proxy.host}/{unique_repo}/{name}:v1"
    write_config(t.alice.work, skills=refs)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    work = t.bob.work
    edited = _skill(work, "solo") / "SKILL.md"
    edited.write_text(_text(edited) + EDIT, encoding="utf-8", newline="\n")
    # Rows 1 and 3 on a machine whose store holds nothing: a proof would fetch.
    cold = cold_runner(t.bob.runner, tmp_path, "cold")
    state = (work / ".grimoire" / "state.json").read_bytes()
    before = proxy.counts

    rc, doc, stderr = _go(cold, "update")

    assert rc == 65, stderr
    assert {row["name"]: row["refused"] for row in doc["items"]} == {
        "kit": False,
        "solo": True,
    }
    reads = {
        path: n - before.get(path, 0)
        for path, n in proxy.counts.items()
        if "/blobs/" in path and n > before.get(path, 0)
    }
    assert reads == {}
    assert (work / ".grimoire" / "state.json").read_bytes() == state


def test_an_update_that_fails_after_the_proof_keeps_the_proven_records(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md S63
    t = _shared(team, "CO", _declare_kit(unique_repo))
    t.bob.runner.run("install")
    _move_kit(unique_repo, "v2")
    _alice_updates(t)
    t.bob.pull()
    work = t.bob.work
    pulled = _lock_pins(work)["kit"]
    assert _record_pins(work)["kit"] != pulled
    _move_kit(unique_repo, "v3")
    # Alice's machine learns v3; a copy of it then loses v3's content alone.
    t.alice.runner.run("update")
    third = _lock_pins(t.alice.work)["kit"]
    home = tmp_path / "half-warm" / "grim-home"
    shutil.copytree(t.alice.runner.grim_home, home)
    layer = fetch_manifest(f"{unique_repo}/kit", "rolling")["layers"][0]["digest"]
    hex_digest = layer.removeprefix("sha256:")
    (home / "blobs" / "sha256" / hex_digest[:2] / hex_digest).unlink()
    bob = GrimRunner(t.alice.runner.binary, home, cwd=work)

    rc, _, stderr = _go(bob, "--offline", "update")

    assert rc == 81, stderr
    assert _lock_pins(work)["kit"] == third
    assert _record_pins(work)["kit"] == pulled

    rc, doc, stderr = _go(bob, "install")
    assert rc == 0, stderr
    assert {row["name"]: row["status"] for row in doc["items"]}["kit"] == "updated"
    assert "kit v3" in _text(_skill(work, "kit") / "SKILL.md")
    assert _record_pins(work) == _lock_pins(work)


# ---------------------------------------------------------------------------
# An MCP entry grim never wrote, and one artifact at two versions
# ---------------------------------------------------------------------------

MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


@pytest.mark.parametrize("force", [False, True], ids=["plain", "force"])
def test_a_clone_update_keeps_a_dropped_mcp_entry_unless_forced(
    team, unique_repo, tmp_path, force
):
    # adr_output_ownership_without_state.md validation case 21
    t = team("CO")
    descriptor = tmp_path / "srv.toml"
    descriptor.write_text(MCP_DESCRIPTOR, encoding="utf-8", newline="\n")
    ref = f"{REGISTRY_HOST}/{unique_repo}/mcp/srv:1.0.0"
    t.alice.runner.json("release", str(descriptor), ref, "--kind", "mcp")
    make_artifact(f"{unique_repo}/kit", "skill", {"kit/SKILL.md": "# kit\n"}, tag="v1")
    head = f'[options]\nclients = ["claude"]\n\n[skills]\nkit = "{REGISTRY_HOST}/{unique_repo}/kit:v1"\n'
    (t.alice.work / "grimoire.toml").write_text(
        f'{head}\n[mcp]\nsrv = "{ref}"\n', encoding="utf-8", newline="\n"
    )
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    config = work / ".mcp.json"
    assert "srv" in json.loads(_text(config))["mcpServers"]
    (work / "grimoire.toml").write_text(head, encoding="utf-8", newline="\n")

    rc, doc, stderr = _go(t.bob.runner, "update", *(["--force"] if force else []))

    assert rc == 0, stderr
    row = {row["name"]: row for row in doc["items"]}["srv"]
    assert row["action"] == "removed"
    servers = json.loads(_text(config)).get("mcpServers", {})
    if force:
        assert "srv" not in servers
        assert row["abandoned_entries"] == []
    else:
        assert "srv" in servers
        [entry] = row["abandoned_entries"]
        assert entry["pointer"] == "/mcpServers/srv"
        assert Path(entry["path"]).resolve() == config.resolve()
    assert _left_lines(stderr, "'srv'") == []


@pytest.mark.parametrize("copilot", ["old-pin", "edited"])
def test_an_update_rolls_one_artifact_at_two_versions_forward(
    team, unique_repo, tmp_path, copilot
):
    # One client's files at the record's pin, the other's at the previous lock's.
    t = _shared(team, "CO", _declare_kit(unique_repo, ("claude", "copilot")))
    t.bob.runner.run("install")
    work = t.bob.work
    own = _skill(work, "kit", "copilot")
    kept = tmp_path / "copilot-kit"
    shutil.copytree(own, kept)
    _move_kit(unique_repo, "v2")
    _alice_updates(t)
    t.bob.pull()
    shutil.rmtree(own)
    shutil.copytree(kept, own)
    if copilot == "edited":
        path = own / "SKILL.md"
        path.write_text(_text(path) + EDIT, encoding="utf-8", newline="\n")
    _move_kit(unique_repo, "v3")

    rc, doc, stderr = _go(t.bob.runner, "update")

    if copilot == "edited":
        assert rc == 65, stderr
        assert {row["name"]: row["refused"] for row in doc["items"]}["kit"] is True
        assert _text(own / "SKILL.md").endswith(EDIT)
        return
    assert rc == 0, stderr
    assert _actions(doc)["kit"] == "updated"
    for client in ("claude", "copilot"):
        assert "kit v3" in _text(_skill(work, "kit", client) / "SKILL.md")
        assert not (_skill(work, "kit", client) / "ref.md").exists()
    assert _record_pins(work) == _lock_pins(work)
