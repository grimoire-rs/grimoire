# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``uninstall``, ``remove``, ``lock`` and ``add`` over files a clone holds with no record.

Before its edit, each command proves the nodes it drops against the lock as
found, from the local store alone, so the proof sends no request. A proven
node gains a record: ``uninstall`` deletes its files, and after ``remove``,
``lock`` or ``add`` a later bare ``grim update`` prunes it. A node no proof
covers keeps its files and is named once; ``--force`` proves nothing. Halves of
``research_install_record_cache_test_matrix.md`` S37, S43 and S44, plus ADR
validation cases 19 and 20. The matrix cells M05f, M07f, M09f, M11f and M11g
live in ``test_record_cache_matrix.py``.
"""

from __future__ import annotations

import json
import os
from collections.abc import Callable
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, write_config
from src.oci_proxy import OciProxy
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.scenarios import Team, allow_insecure

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


def _left_lines(stderr: str, token: str) -> list[str]:
    """The stderr lines naming ``token`` that say files, or an MCP entry, were left behind."""
    left = ("on disk", "left its entry in")
    return [
        line
        for line in stderr.splitlines()
        if token in line and any(m in line for m in left)
    ]


def _skill(work: Path, name: str) -> Path:
    return work / ".claude" / "skills" / name


def _records(work: Path) -> set[str]:
    path = work / ".grimoire" / "state.json"
    if not path.exists():
        return set()
    return {r["name"] for r in json.loads(path.read_text(encoding="utf-8"))["records"]}


def _locked(work: Path) -> set[str]:
    doc = tomllib.loads((work / "grimoire.lock").read_text(encoding="utf-8"))
    return {
        row["name"] for rows in doc.values() if isinstance(rows, list) for row in rows
    }


def _actions(doc: dict) -> dict[str, str]:
    return {row["name"]: row["action"] for row in doc.get("items", [])}


def _edit(work: Path, name: str) -> None:
    path = _skill(work, name) / "SKILL.md"
    path.write_text(
        path.read_text(encoding="utf-8") + EDIT, encoding="utf-8", newline="\n"
    )


# ---------------------------------------------------------------------------
# World: Alice declares, installs, commits and pushes; Bob pulls the files
# ---------------------------------------------------------------------------


def _publish(host: str, repo: str, *names: str) -> dict[str, str]:
    refs = {}
    for name in names:
        make_artifact(
            f"{repo}/{name}", "skill", {f"{name}/SKILL.md": f"# {name}\n"}, tag="v1"
        )
        refs[name] = f"{host}/{repo}/{name}:v1"
    return refs


def _clone(team: Callable[[str], Team], declare: Callable[[Team], None]) -> Team:
    """Bob holds Alice's committed files and lock, and no install state."""
    t = team("CO")
    declare(t)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    assert not (t.bob.work / ".grimoire").exists()
    return t


def _skills(refs: dict[str, str]) -> Callable[[Team], None]:
    return lambda t: write_config(t.alice.work, skills=refs)


def _warm(t: Team) -> GrimRunner:
    """Bob's workspace on Alice's machine, whose store holds every pin she installed."""
    a = t.alice.runner
    return GrimRunner(a.binary, a.grim_home, cwd=t.bob.work)


# ---------------------------------------------------------------------------
# uninstall
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("case", ["warm", "cold", "cold-force"])
def test_an_uninstall_in_a_clone_deletes_only_what_the_store_proves(
    team, unique_repo, case
):
    # adr_output_ownership_without_state.md validation case 19
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    runner = _warm(t) if case == "warm" else t.bob.runner
    flags = ["--force"] if case == "cold-force" else []

    rc, doc, stderr = _go(runner, "uninstall", *flags, "skill", "solo")

    assert rc == 0, stderr
    assert doc["status"] == "uninstalled"
    lines = _left_lines(stderr, "'solo'")
    if case == "warm":
        assert not _skill(work, "solo").exists()
        assert lines == []
    else:
        assert (_skill(work, "solo") / "SKILL.md").is_file()
        (line,) = lines
        assert "skill 'solo'" in line
        assert ".claude/skills/solo" in line.replace("\\", "/")
    assert "solo" not in _records(work)
    assert "solo" not in _locked(work)
    assert (_skill(work, "kit") / "SKILL.md").is_file()


def test_an_uninstall_proves_no_member_a_bundle_holds(team, unique_repo):
    # adr_output_ownership_without_state.md A8: a node a bundle holds is not proven
    make_artifact(
        f"{unique_repo}/helper", "skill", {"helper/SKILL.md": "# helper\n"}, tag="v1"
    )
    make_bundle(
        f"{unique_repo}/stack",
        [("skill", "helper", f"{REGISTRY_HOST}/{unique_repo}/helper:v1")],
        tag="v1",
    )
    refs = _publish(REGISTRY_HOST, unique_repo, "solo")
    t = _clone(
        team,
        lambda t: write_config(
            t.alice.work,
            skills=refs,
            bundles={"stack": f"{REGISTRY_HOST}/{unique_repo}/stack:v1"},
        ),
    )
    work = t.bob.work

    rc, doc, stderr = _go(_warm(t), "uninstall", "skill", "helper")

    assert rc == 0, stderr
    assert doc["status"] == "kept-by-bundle"
    assert (_skill(work, "helper") / "SKILL.md").is_file()
    assert _records(work) == set()
    assert _left_lines(stderr, "'helper'") == []

    # A sibling's uninstall proves and deletes the sibling alone.
    rc, doc, stderr = _go(_warm(t), "uninstall", "skill", "solo")

    assert rc == 0, stderr
    assert doc["status"] == "uninstalled"
    assert not _skill(work, "solo").exists()
    assert (_skill(work, "helper") / "SKILL.md").is_file()
    assert _records(work) == set()
    assert _left_lines(stderr, "'helper'") == []


MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


@pytest.mark.parametrize("force", [False, True], ids=["plain", "force"])
def test_an_uninstall_keeps_an_mcp_entry_a_clone_proved_unless_forced(
    team, unique_repo, tmp_path, force
):
    # research_install_record_cache_test_matrix.md S37
    descriptor = tmp_path / "srv.toml"
    descriptor.write_text(MCP_DESCRIPTOR, encoding="utf-8", newline="\n")
    ref = f"{REGISTRY_HOST}/{unique_repo}/mcp/srv:1.0.0"

    def declare(t: Team) -> None:
        t.alice.runner.json("release", str(descriptor), ref, "--kind", "mcp")
        text = f'[options]\nclients = ["claude"]\n\n[mcp]\nsrv = "{ref}"\n'
        (t.alice.work / "grimoire.toml").write_text(
            text, encoding="utf-8", newline="\n"
        )

    t = _clone(team, declare)
    config = t.bob.work / ".mcp.json"
    assert "srv" in json.loads(config.read_text(encoding="utf-8"))["mcpServers"]

    rc, doc, stderr = _go(
        _warm(t), "uninstall", *(["--force"] if force else []), "mcp", "srv"
    )

    assert rc == 0, stderr
    assert doc["status"] == "uninstalled"
    servers = json.loads(config.read_text(encoding="utf-8")).get("mcpServers", {})
    if force:
        assert "srv" not in servers
        assert doc["abandoned_entries"] == []
    else:
        assert "srv" in servers
        [entry] = doc["abandoned_entries"]
        assert entry["pointer"] == "/mcpServers/srv"
        assert Path(entry["path"]).resolve() == config.resolve()
        assert any(
            "/mcpServers/srv" in line and "--force" in line
            for line in stderr.splitlines()
        )
    assert _left_lines(stderr, "'srv'") == []


# ---------------------------------------------------------------------------
# remove
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("store", ["warm", "cold"])
def test_a_remove_in_a_clone_proves_before_the_lock_save(team, unique_repo, store):
    # adr_output_ownership_without_state.md validation case 20
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    runner = _warm(t) if store == "warm" else t.bob.runner

    rc, doc, stderr = _go(runner, "remove", "skill", "solo")

    assert rc == 0, stderr
    assert doc["status"] == "removed"
    assert "solo" not in _locked(work)
    assert (_skill(work, "solo") / "SKILL.md").is_file()
    lines = _left_lines(stderr, "'solo'")
    if store == "warm":
        assert "solo" in _records(work)
        assert lines == []
    else:
        assert "solo" not in _records(work)
        (line,) = lines
        assert ".claude/skills/solo" in line.replace("\\", "/")

    rc, doc, stderr = _go(_warm(t), "update")

    assert rc == 0, stderr
    if store == "warm":
        assert _actions(doc)["solo"] == "removed"
        assert not _skill(work, "solo").exists()
    else:
        assert "solo" not in _actions(doc)
        assert (_skill(work, "solo") / "SKILL.md").is_file()


def test_a_remove_over_an_unreadable_state_warns_and_keeps_its_exit(team, unique_repo):
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    (work / ".grimoire").mkdir()
    (work / ".grimoire" / "state.json").write_text(
        "not json\n", encoding="utf-8", newline="\n"
    )

    rc, doc, stderr = _go(_warm(t), "remove", "skill", "solo")

    assert rc == 0, stderr
    assert doc["status"] == "removed"
    assert "solo" not in _locked(work)
    assert any("state.json" in line for line in stderr.splitlines()), stderr


@posix_only
@not_root
def test_a_remove_whose_proven_records_are_not_saved_names_their_files(
    team, unique_repo
):
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    state_dir = work / ".grimoire"
    state_dir.mkdir()
    state_dir.chmod(0o500)
    try:
        rc, doc, stderr = _go(_warm(t), "remove", "skill", "solo")
    finally:
        state_dir.chmod(0o700)

    assert rc == 0, stderr
    assert doc["status"] == "removed"
    assert "solo" not in _locked(work)
    assert "solo" not in _records(work)
    assert (_skill(work, "solo") / "SKILL.md").is_file()
    (line,) = _left_lines(stderr, "'solo'")
    assert "skill 'solo'" in line
    assert ".claude/skills/solo" in line.replace("\\", "/")
    # The files proved, so the reason is the lost record, not an unreadable file.
    assert "could not be saved" in line
    assert "unreadable" not in line
    assert "by hand" in line


@posix_only
@not_root
def test_an_uninstall_whose_proven_records_are_not_saved_edits_nothing(
    team, unique_repo
):
    # The proven records are saved before the config and lock edit, or nothing is edited.
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    lock = (work / "grimoire.lock").read_bytes()
    config = (work / "grimoire.toml").read_bytes()
    state_dir = work / ".grimoire"
    state_dir.mkdir()
    state_dir.chmod(0o500)
    try:
        rc, _, stderr = _go(_warm(t), "uninstall", "skill", "solo")
    finally:
        state_dir.chmod(0o700)

    assert rc == 77, stderr
    assert (work / "grimoire.lock").read_bytes() == lock
    assert (work / "grimoire.toml").read_bytes() == config
    assert (_skill(work, "solo") / "SKILL.md").is_file()


def test_neither_uninstall_nor_remove_sends_a_request_to_prove(
    team, unique_repo, oci_proxy
):
    # adr_output_ownership_without_state.md A6: the uninstall and remove pre-passes read the store only
    proxy: OciProxy = oci_proxy("count")
    t = team("CO")
    for member in (t.alice, t.bob):
        allow_insecure(member.runner, proxy.host)
    refs = _publish(proxy.host, unique_repo, "solo", "kit", "extra")
    write_config(t.alice.work, skills=refs)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    before = proxy.counts

    # A cold store: a proof that reached for the registry would fetch each layer.
    rc, _, uninstalled = _go(t.bob.runner, "uninstall", "--no-relock", "skill", "solo")
    assert rc == 0, uninstalled
    rc, _, removed = _go(t.bob.runner, "remove", "--no-relock", "skill", "kit")
    assert rc == 0, removed

    assert proxy.counts == before
    assert len(_left_lines(uninstalled, "'solo'")) == 1
    assert len(_left_lines(removed, "'kit'")) == 1
    assert (_skill(work, "solo") / "SKILL.md").is_file()
    assert (_skill(work, "kit") / "SKILL.md").is_file()


# ---------------------------------------------------------------------------
# lock and add: a lock write that drops a pin
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("files", ["proven", "edited"])
def test_a_lock_after_a_hand_removed_declaration_proves_what_it_drops(
    team, unique_repo, files
):
    # research_install_record_cache_test_matrix.md S43
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    if files == "edited":
        _edit(work, "solo")
    write_config(work, skills={"kit": refs["kit"]})

    rc, doc, stderr = _go(_warm(t), "lock")

    assert rc == 0, stderr
    assert _actions(doc)["solo"] == "dropped"
    lines = _left_lines(stderr, "'solo'")
    if files == "proven":
        assert "solo" in _records(work)
        assert lines == []
    else:
        assert "solo" not in _records(work)
        (line,) = lines
        assert ".claude/skills/solo" in line.replace("\\", "/")

    rc, doc, stderr = _go(_warm(t), "update")

    assert rc == 0, stderr
    if files == "proven":
        assert _actions(doc)["solo"] == "removed"
        assert not _skill(work, "solo").exists()
    else:
        assert "solo" not in _actions(doc)
        assert (
            (_skill(work, "solo") / "SKILL.md")
            .read_text(encoding="utf-8")
            .endswith(EDIT)
        )


@pytest.mark.parametrize("files", ["proven", "edited"])
def test_an_add_whose_relock_drops_a_pin_proves_it(team, unique_repo, files):
    # research_install_record_cache_test_matrix.md S44
    refs = _publish(REGISTRY_HOST, unique_repo, "solo", "kit", "extra")
    extra = refs.pop("extra")
    t = _clone(team, _skills(refs))
    work = t.bob.work
    if files == "edited":
        _edit(work, "solo")
    write_config(work, skills={"kit": refs["kit"]})

    rc, doc, stderr = _go(_warm(t), "add", extra)

    assert rc == 0, stderr
    assert "solo" not in _locked(work)
    assert (_skill(work, "extra") / "SKILL.md").is_file()
    lines = _left_lines(stderr, "'solo'")
    if files == "proven":
        assert "solo" in _records(work)
        assert lines == []
    else:
        assert "solo" not in _records(work)
        (line,) = lines
        assert ".claude/skills/solo" in line.replace("\\", "/")

    rc, doc, stderr = _go(_warm(t), "update")

    assert rc == 0, stderr
    if files == "proven":
        assert _actions(doc)["solo"] == "removed"
        assert not _skill(work, "solo").exists()
    else:
        assert "solo" not in _actions(doc)
        assert (_skill(work, "solo") / "SKILL.md").is_file()
