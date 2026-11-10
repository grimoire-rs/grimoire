# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim update <names>` acts on the named closure and nothing else (issue #269).

Installing, gating, pruning and reaping all stop at the closure of the named
nodes. Bare `grim update` keeps the whole lock as its scope.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from functools import partial
from pathlib import Path

import pytest

from src.assertions import assert_not_exists, assert_path_exists
from src.helpers import (
    make_artifact,
    make_bundle,
    make_dependent,
    make_hook,
    write_config,
)
from src.registry import PublishedArtifact, retag, tag_digest
from src.runner import GrimRunner

_EDIT = "locally edited\n"
# Strips `:tag` from `pinned` only: a registry port is followed by `/`, never
# by `@sha256:`.
_REQUIRES_BLOCK = re.compile(
    r'\n\[\[skill\.requires\]\]\nkind = "[^"]*"\nname = "[^"]*"\nid = "[^"]*"\n'
)
_BARE_REFUSAL = (
    "Update refused a locally modified artifact; rerunning with --force overwrites "
    "it and also authorizes update's prune and reap deletions"
)


def _files(name: str, body: str = "") -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"
    }


def _skill(unique_repo: str, name: str, body: str = "v1") -> PublishedArtifact:
    return make_artifact(f"{unique_repo}/{name}", "skill", _files(name, body), tag="1")


def _move_tag(unique_repo: str, name: str, body: str = "v2") -> PublishedArtifact:
    """Publish new bytes under `:1`; `keep` stops zot dropping the old manifest."""
    repo = f"{unique_repo}/{name}"
    old = tag_digest(repo, "1")
    retag(repo, "keep", old)
    moved = make_artifact(repo, "skill", _files(name, body), tag="1")
    assert moved.digest != old, "a moved tag must change the digest"
    return moved


def _rows(result: subprocess.CompletedProcess[str]) -> dict[str, dict]:
    items = json.loads(result.stdout)["items"]
    rows = {row["name"]: row for row in items}
    assert len(items) == len(rows), f"a name reported twice: {items}"
    return rows


@dataclass(slots=True)
class Scope:
    """One install scope, addressed the same way by every test below."""

    runner: GrimRunner
    config_dir: Path
    skills: Path
    flags: tuple[str, ...]
    mcp_file: Path

    def run(self, *args: str, **kwargs) -> subprocess.CompletedProcess[str]:
        return self.runner.run(*self.flags, *args, **kwargs)


@pytest.fixture(params=["project", "global"])
def scope(request, grim_at, grim, project_dir: Path, grim_home: Path) -> Scope:
    if request.param == "project":
        return Scope(
            grim_at(project_dir),
            project_dir,
            project_dir / ".claude/skills",
            (),
            project_dir / ".mcp.json",
        )
    # The marker makes Claude a detected global client, as `.claude` does in a project.
    (grim.home / ".claude").mkdir()
    return Scope(
        grim,
        grim_home,
        grim.home / ".claude/skills",
        ("--global",),
        grim.home / ".claude.json",
    )


_MCP = 'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "{command}"\n'
_AGENT = "---\nname: beta\ndescription: d\n---\nbody\n"


def _release_mcp(scope: Scope, src: Path, ref: str, command: str) -> None:
    src.mkdir()
    (src / "server.toml").write_text(_MCP.format(command=command), encoding="utf-8", newline="\n")
    scope.runner.json("release", str(src / "server.toml"), ref, "--kind", "mcp")


@dataclass(slots=True)
class Pair:
    """`alpha`, which the run names, beside `beta` of one kind, which it does not."""

    beta: Callable[[], object]
    delete_beta: Callable[[], None]
    edit_beta: Callable[[], None]
    move_alpha: Callable[[], None]
    alpha_moved: Callable[[], bool]


def _mcp_pair(scope: Scope, tmp_path: Path, registry: str, unique_repo: str) -> Pair:
    """Issue #269's own repro: two MCP bindings at one server, one config file."""
    repo = f"{unique_repo}/server"
    ref = f"{registry}/{repo}:1"
    _release_mcp(scope, tmp_path / "v1", ref, "srv")
    _release_mcp(scope, tmp_path / "v2", f"{registry}/{repo}:next", "srv2")
    write_config(scope.config_dir)
    with (scope.config_dir / "grimoire.toml").open("a") as config:
        config.write(f'[mcp]\nalpha = "{ref}"\nbeta = "{ref}"\n')

    def entry(name: str):
        return json.loads(scope.mcp_file.read_text(encoding="utf-8"))["mcpServers"].get(name)

    def edit(change) -> None:
        doc = json.loads(scope.mcp_file.read_text(encoding="utf-8"))
        change(doc["mcpServers"])
        scope.mcp_file.write_text(json.dumps(doc), encoding="utf-8", newline="\n")

    def move() -> None:
        retag(repo, "keep", tag_digest(repo, "1"))
        retag(repo, "1", tag_digest(repo, "next"))

    return Pair(
        beta=lambda: entry("beta"),
        delete_beta=lambda: edit(lambda servers: servers.pop("beta")),
        edit_beta=lambda: edit(lambda servers: servers["beta"].update(env={"E": "1"})),
        move_alpha=move,
        alpha_moved=lambda: entry("alpha")["command"] == "srv2",
    )


def _file_pair(scope: Scope, kind: str, unique_repo: str) -> Pair:
    root = scope.skills.parent
    alpha = _skill(unique_repo, "alpha")
    tables: dict[str, dict[str, str]] = {"skills": {"alpha": alpha.fq}}
    if kind == "skill":
        tables["skills"]["beta"] = _skill(unique_repo, "beta").fq
        path, footprint = root / "skills/beta/SKILL.md", root / "skills/beta"
        delete = partial(shutil.rmtree, footprint)
    else:
        body = _AGENT if kind == "agent" else "# beta\n"
        beta = make_artifact(f"{unique_repo}/beta", kind, {"beta.md": body}, tag="1")
        tables[f"{kind}s"] = {"beta": beta.fq}
        path = footprint = root / f"{kind}s/beta.md"
        delete = footprint.unlink
    write_config(scope.config_dir, **tables)
    return Pair(
        beta=lambda: path.read_bytes() if footprint.exists() else None,
        delete_beta=delete,
        edit_beta=lambda: path.write_text(_EDIT, encoding="utf-8", newline="\n"),
        move_alpha=lambda: _move_tag(unique_repo, "alpha"),
        alpha_moved=lambda: "alpha v2" in (root / "skills/alpha/SKILL.md").read_text(encoding="utf-8"),
    )


def _hook_pair(scope: Scope, unique_repo: str) -> Pair:
    """`beta` is an armed hook: its dispatch rows, and its registration in Claude's file."""
    root = scope.skills.parent
    alpha = _skill(unique_repo, "alpha")
    beta = make_hook(f"{unique_repo}/beta", "beta", tag="1", matcher="Bash")
    write_config(
        scope.config_dir,
        skills={"alpha": alpha.fq},
        hooks={"beta": beta.fq},
        options={"experimental.hooks": "true"},
    )
    table = scope.runner.grim_home / "hooks" / "dispatch.json"
    surface = _hook_surface(scope)

    def state():
        if "beta" not in _armed(scope.runner):
            return None
        return table.read_bytes(), _bytes_or_none(surface)

    def delete() -> None:
        _drop_dispatch_rows(table, "beta")
        _drop_matcher_group(surface, "Bash")

    def edit() -> None:
        doc = json.loads(surface.read_text(encoding="utf-8"))
        for group in doc["hooks"]["PreToolUse"]:
            group["mine"] = 1
        surface.write_text(json.dumps(doc, indent=2), encoding="utf-8", newline="\n")

    return Pair(
        beta=state,
        delete_beta=delete,
        edit_beta=edit,
        move_alpha=lambda: _move_tag(unique_repo, "alpha"),
        alpha_moved=lambda: "alpha v2" in (root / "skills/alpha/SKILL.md").read_text(encoding="utf-8"),
    )


def _installed_pair(
    scope: Scope, kind: str, tmp_path: Path, registry: str, unique_repo: str
) -> Pair:
    if kind == "mcp":
        pair = _mcp_pair(scope, tmp_path, registry, unique_repo)
    elif kind == "hook":
        pair = _hook_pair(scope, unique_repo)
    else:
        pair = _file_pair(scope, kind, unique_repo)
    scope.run("lock")
    if kind == "hook" and not scope.flags:
        scope.run("hook", "allow")
    scope.run("install")
    assert pair.beta() is not None, "beta must start installed"
    return pair


_KINDS = [
    "skill",
    "rule",
    "agent",
    "mcp",
    pytest.param(
        "hook",
        marks=pytest.mark.skipif(
            os.name == "nt", reason="hook arming is POSIX-only here"
        ),
    ),
]


@pytest.mark.parametrize("kind", _KINDS)
def test_case_a(
    scope: Scope, kind: str, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """Issue #269 case A: an unnamed output the user deleted stays deleted."""
    pair = _installed_pair(scope, kind, tmp_path, registry, unique_repo)
    pair.delete_beta()
    pair.move_alpha()

    result = scope.run("update", "--", "alpha", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert pair.beta() is None
    assert pair.alpha_moved()
    assert set(_rows(result)) == {"alpha"}


@pytest.mark.parametrize("kind", _KINDS)
def test_case_b(
    scope: Scope, kind: str, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """Issue #269 case B: an unnamed edit neither blocks the run nor is touched."""
    pair = _installed_pair(scope, kind, tmp_path, registry, unique_repo)
    pair.edit_beta()
    edited = pair.beta()
    pair.move_alpha()

    result = scope.run("update", "--", "alpha", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert pair.beta() == edited
    assert pair.alpha_moved()
    assert set(_rows(result)) == {"alpha"}


def test_named_update_adds_no_managed_entry_outside_its_closure(
    scope: Scope, registry: str, unique_repo: str
) -> None:
    """An unnamed rule's hand-removed exclusion stays removed until a bare run."""
    alpha = _skill(unique_repo, "alpha")
    files = {"guide.md": "# guide\n", "guide/more.md": "# more\n"}
    guide = make_artifact(f"{unique_repo}/guide", "rule", files, tag="1")
    write_config(
        scope.config_dir, skills={"alpha": alpha.fq}, rules={"guide": guide.fq}
    )
    scope.run("lock")
    scope.run("install")
    settings = scope.skills.parent / "settings.json"
    excluded = json.loads(settings.read_text(encoding="utf-8"))["claudeMdExcludes"]
    settings.write_text(json.dumps({"mine": 1}), encoding="utf-8", newline="\n")
    written = settings.read_bytes()
    _move_tag(unique_repo, "alpha")

    result = scope.run("update", "--", "alpha", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert settings.read_bytes() == written
    assert "outside the named closure" not in result.stderr, result.stderr

    scope.run("update")

    assert json.loads(settings.read_text(encoding="utf-8"))["claudeMdExcludes"] == excluded


def test_uninstall_adds_no_managed_entry_of_another_rule(
    scope: Scope, registry: str, unique_repo: str
) -> None:
    """Removing `alpha` leaves another rule's hand-removed exclusion removed."""
    alpha = _skill(unique_repo, "alpha")
    files = {"guide.md": "# guide\n", "guide/more.md": "# more\n"}
    guide = make_artifact(f"{unique_repo}/guide", "rule", files, tag="1")
    write_config(
        scope.config_dir, skills={"alpha": alpha.fq}, rules={"guide": guide.fq}
    )
    scope.run("lock")
    scope.run("install")
    settings = scope.skills.parent / "settings.json"
    settings.write_text(json.dumps({"mine": 1}), encoding="utf-8", newline="\n")
    written = settings.read_bytes()

    result = scope.run("uninstall", "skill", "alpha", check=False)

    assert result.returncode == 0, result.stderr
    assert not (scope.skills / "alpha").exists()
    assert settings.read_bytes() == written


def _dependent_pair(grim_at, project_dir: Path, unique_repo: str) -> GrimRunner:
    """Install `parent` → `leaf`, plus `bystander`, which the closure never reaches."""
    _skill(unique_repo, "leaf")
    bystander = _skill(unique_repo, "bystander")
    parent = make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        _files("parent"),
        [("skill", "leaf", "./leaf:1")],
    )
    write_config(project_dir, skills={"parent": parent.fq, "bystander": bystander.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    return runner


def test_modified_dependency_in_the_closure_exits_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Scoping spares unnamed bindings, not a dependency of the named one."""
    runner = _dependent_pair(grim_at, project_dir, unique_repo)
    edited = project_dir / ".claude/skills/leaf/SKILL.md"
    edited.write_text(_EDIT, encoding="utf-8", newline="\n")

    result = runner.run("update", "--", "parent", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _rows(result)["leaf"]["refused"] is True
    assert edited.read_text(encoding="utf-8") == _EDIT


def test_force_overrides_the_closure_only(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--force` overwrites inside the closure and stops there."""
    runner = _dependent_pair(grim_at, project_dir, unique_repo)
    inside = project_dir / ".claude/skills/leaf/SKILL.md"
    outside = project_dir / ".claude/skills/bystander/SKILL.md"
    inside.write_text(_EDIT, encoding="utf-8", newline="\n")
    outside.write_text(_EDIT, encoding="utf-8", newline="\n")

    result = runner.run("update", "--force", "--", "parent", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert inside.read_text(encoding="utf-8") != _EDIT
    assert outside.read_text(encoding="utf-8") == _EDIT
    assert set(_rows(result)) == {"parent", "leaf"}


def test_force_on_a_named_update_leaves_an_unnamed_edit(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A refusal promises what `--force` does on this run, and it does."""
    runner = _dependent_pair(grim_at, project_dir, unique_repo)
    inside = project_dir / ".claude/skills/leaf/SKILL.md"
    outside = project_dir / ".claude/skills/bystander/SKILL.md"
    inside.write_text(_EDIT, encoding="utf-8", newline="\n")
    outside.write_text(_EDIT, encoding="utf-8", newline="\n")

    refused = runner.run("update", "--", "parent", check=False)

    assert refused.returncode == 65, refused.stderr
    assert "closure" in refused.stderr and "prune" in refused.stderr, refused.stderr
    assert "reap" not in refused.stderr, refused.stderr

    forced = runner.run("update", "--force", "--", "parent", check=False)

    assert forced.returncode == 0, forced.stderr
    assert inside.read_text(encoding="utf-8") != _EDIT
    assert outside.read_text(encoding="utf-8") == _EDIT

    bare = runner.run("update", check=False)

    assert bare.returncode == 65, bare.stderr
    assert _BARE_REFUSAL in bare.stderr, bare.stderr


def test_named_update_reports_a_moved_pin_outside_its_closure(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A pin moved outside the closure gets a row; its files wait for `install`.

    The lock records no edge and the pin names a tag `parent` does not ask for, so
    nothing answers `leaf` and it re-resolves although no name reaches it.
    """
    runner = _dependent_pair(grim_at, project_dir, unique_repo)
    lock = project_dir / "grimoire.lock"
    lock.write_text(
        _REQUIRES_BLOCK.sub("\n", lock.read_text(encoding="utf-8")).replace("/leaf:1@", "/leaf:0@"), encoding="utf-8", newline="\n"
    )
    assert "/leaf:0@" in lock.read_text(encoding="utf-8")
    old = tag_digest(f"{unique_repo}/leaf", "1")
    moved = _move_tag(unique_repo, "leaf")
    installed = project_dir / ".claude/skills/leaf/SKILL.md"

    result = runner.run("update", "--", "bystander", format="json", check=False)

    assert result.returncode == 0, result.stderr
    rows = _rows(result)
    assert set(rows) == {"bystander", "leaf"}, rows
    leaf = rows["leaf"]
    assert (leaf["action"], leaf["old"], leaf["new"]) == ("updated", old, moved.digest)
    assert "leaf v1" in installed.read_text(encoding="utf-8")
    assert "outside the named closure" in result.stderr, result.stderr

    runner.run("install")

    assert "leaf v2" in installed.read_text(encoding="utf-8")


def test_named_update_installs_what_its_closure_gained(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The closure is the new lock's, so an edge the move brought lands."""
    gained = _skill(unique_repo, "gained")
    parent = _skill(unique_repo, "parent")
    write_config(project_dir, skills={"parent": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    assert_not_exists(project_dir / ".claude/skills/gained")
    retag(f"{unique_repo}/parent", "keep", parent.digest)
    make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        _files("parent", "v2"),
        [("skill", "gained", "./gained:1")],
    )

    result = runner.run("update", "--", "parent", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert_path_exists(project_dir / ".claude/skills/gained")
    row = _rows(result)["gained"]
    assert (row["action"], row["old"], row["new"]) == ("updated", None, gained.digest)


def test_update_names_without_a_lock_prunes_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """With no previous lock a named run has no closure to prune from."""
    named = _skill(unique_repo, "named")
    stray = _skill(unique_repo, "stray")
    write_config(project_dir, skills={"stray": stray.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    runner.run("remove", "skill", "stray")
    write_config(project_dir, skills={"named": named.fq})
    (project_dir / "grimoire.lock").unlink()

    result = runner.run("update", "--", "named", check=False)

    assert result.returncode == 0, result.stderr
    assert_path_exists(project_dir / ".claude/skills/stray")


def _pack_with_a_stray(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, PublishedArtifact]:
    """Install `pack` (`stays`, `dropped`) beside `stray`, an orphan it never held.

    Returns the runner and the `stays` artifact. `grim remove` leaves the files
    and the record, so `stray` is an orphan outside the bundle's closure.
    """
    stays = _skill(unique_repo, "stays")
    dropped = _skill(unique_repo, "dropped")
    stray = _skill(unique_repo, "stray")
    pack = make_bundle(
        f"{unique_repo}/pack",
        [("skill", "stays", stays.fq), ("skill", "dropped", dropped.fq)],
        tag="1",
    )
    write_config(project_dir, skills={"stray": stray.fq}, bundles={"pack": pack.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    runner.run("remove", "skill", "stray")
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    return runner, stays


def test_scoped_prune_removes_a_dropped_member_and_nothing_else(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A named bundle prunes what it dropped, never an unrelated orphan."""
    runner, stays = _pack_with_a_stray(grim_at, project_dir, unique_repo)
    make_bundle(f"{unique_repo}/pack", [("skill", "stays", stays.fq)], tag="1")

    result = runner.run("update", "--", "pack", format="json", check=False)

    assert result.returncode == 0, result.stderr
    skills = project_dir / ".claude/skills"
    assert_not_exists(skills / "dropped")
    assert_path_exists(skills / "stays")
    assert_path_exists(skills / "stray")
    rows = _rows(result)
    assert rows["dropped"]["action"] == "removed", rows
    assert "stray" not in rows


def test_scoped_prune_keeps_a_modified_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A member the user edited is kept, reported, and its remedy named.

    Kept for the finding behind it: the kept member has left the lock, so a
    rerun of the same named update with `--force` shows no row and keeps the
    file. Only the warning tells the user what removes it.
    """
    runner, stays = _pack_with_a_stray(grim_at, project_dir, unique_repo)
    edited = project_dir / ".claude/skills/dropped/SKILL.md"
    edited.write_text(_EDIT, encoding="utf-8", newline="\n")
    make_bundle(f"{unique_repo}/pack", [("skill", "stays", stays.fq)], tag="1")

    result = runner.run("update", "--", "pack", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert edited.read_text(encoding="utf-8") == _EDIT
    assert _rows(result)["dropped"]["action"] == "kept-modified"
    assert "bare `grim update --force`" in result.stderr


def test_force_on_a_named_update_prunes_only_inside_its_closure(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--force` deletes a modified member, never a modified orphan outside."""
    runner, stays = _pack_with_a_stray(grim_at, project_dir, unique_repo)
    skills = project_dir / ".claude/skills"
    stray = skills / "stray/SKILL.md"
    (skills / "dropped/SKILL.md").write_text(_EDIT, encoding="utf-8", newline="\n")
    stray.write_text(_EDIT, encoding="utf-8", newline="\n")
    make_bundle(f"{unique_repo}/pack", [("skill", "stays", stays.fq)], tag="1")

    result = runner.run("update", "--force", "--", "pack", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert_not_exists(skills / "dropped")
    assert_path_exists(stray)
    assert stray.read_text(encoding="utf-8") == _EDIT
    rows = _rows(result)
    assert rows["dropped"]["action"] == "removed", rows
    assert "stray" not in rows


def test_unscoped_update_reports_restored(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An output rewritten at the same pin is `restored`, not `unchanged`."""
    pinned = _skill(unique_repo, "pinned")
    write_config(project_dir, skills={"pinned": pinned.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    shutil.rmtree(project_dir / ".claude/skills/pinned")

    row = _rows(runner.run("update", format="json"))["pinned"]

    assert row["action"] == "restored", row
    assert (row["old"], row["new"]) == (pinned.digest, pinned.digest), row
    assert "pinned v1" in (project_dir / ".claude/skills/pinned/SKILL.md").read_text(encoding="utf-8")


def test_named_update_never_reaps(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A dropped client's outputs wait for a bare `grim update`."""
    named = make_artifact(
        f"{unique_repo}/named", "rule", {"named.md": "# n\n"}, tag="1"
    )
    rules = {"named": named.fq}
    write_config(project_dir, rules=rules, options={"clients": '["claude", "copilot"]'})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    copilot = project_dir / ".github/instructions/named.instructions.md"
    assert_path_exists(copilot)
    write_config(project_dir, rules=rules, options={"clients": '["claude"]'})

    rows = _rows(runner.run("update", "--", "named", format="json"))

    assert_path_exists(copilot)
    assert rows["named"]["reaped_clients"] == [], rows

    rows = _rows(runner.run("update", format="json"))

    assert_not_exists(copilot)
    assert rows["named"]["reaped_clients"] == ["copilot"], rows


def test_update_names_without_a_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """With no lock everything locks, the closure installs, nothing prunes."""
    named = _skill(unique_repo, "named")
    bystander = _skill(unique_repo, "bystander")
    stray = _skill(unique_repo, "stray")
    write_config(project_dir, skills={"stray": stray.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    runner.run("remove", "skill", "stray")
    write_config(project_dir, skills={"named": named.fq, "bystander": bystander.fq})
    (project_dir / "grimoire.lock").unlink()

    result = runner.run("update", "--", "named", check=False)

    assert result.returncode == 0, result.stderr
    lock_text = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert named.digest in lock_text and bystander.digest in lock_text
    skills = project_dir / ".claude/skills"
    assert_path_exists(skills / "named")
    assert_not_exists(skills / "bystander")
    assert_path_exists(skills / "stray")
    assert "no lock to update" in result.stderr, result.stderr
    assert "outside the named closure" not in result.stderr, "one cause, one warning"


def test_update_unknown_name_without_a_lock_warns(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """With no lock an unknown name warns and still exits 0."""
    named = _skill(unique_repo, "named")
    write_config(project_dir, skills={"named": named.fq})
    runner = grim_at(project_dir)

    result = runner.run("update", "--", "nonesuch", check=False)

    assert result.returncode == 0, result.stderr
    assert "nonesuch" in result.stderr, "the unknown name must not pass in silence"


@pytest.mark.parametrize("names", [(), ("--", "alpha")], ids=["bare", "named"])
def test_update_refuses_an_unreadable_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str, names: tuple[str, ...]
) -> None:
    """An unreadable lock stops the run; reading it as no lock moves every pin."""
    alpha = _skill(unique_repo, "alpha")
    write_config(project_dir, skills={"alpha": alpha.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"
    lock.write_text("not a lock\n", encoding="utf-8", newline="\n")

    result = runner.run("update", *names, check=False)

    assert result.returncode == 78, result.stderr
    assert "restore it from git" in result.stderr
    assert lock.read_text(encoding="utf-8") == "not a lock\n"


@pytest.mark.parametrize(
    "command",
    [("lock",), ("update",), ("install",), ("status",)],
    ids=lambda command: command[0],
)
def test_every_command_words_an_unreadable_lock_alike(
    grim_at, project_dir: Path, registry: str, unique_repo: str, command: tuple[str, ...]
) -> None:
    """Each reader of the lock names the one recovery that keeps the pins."""
    alpha = _skill(unique_repo, "alpha")
    write_config(project_dir, skills={"alpha": alpha.fq})
    (project_dir / "grimoire.lock").write_text("not a lock\n", encoding="utf-8", newline="\n")

    result = grim_at(project_dir).run(*command, check=False)

    assert result.returncode == 78, result.stderr
    assert (
        "the lock cannot be read; restore it from git or repair it, "
        "because deleting the lock moves every pin"
    ) in result.stderr


def test_remove_words_an_unreadable_lock_alike(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The edit stands and the warning names the same recovery as every refusal."""
    alpha = _skill(unique_repo, "alpha")
    write_config(project_dir, skills={"alpha": alpha.fq})
    (project_dir / "grimoire.lock").write_text("not a lock\n", encoding="utf-8", newline="\n")

    result = grim_at(project_dir).run("remove", "skill", "alpha", check=False)

    assert result.returncode == 0, result.stderr
    assert "restore it from git or repair it" in result.stderr


def test_a_quoted_lock_reaches_the_terminal_escaped(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An error quoting a lock nobody here wrote cannot drive the terminal."""
    alpha = _skill(unique_repo, "alpha")
    write_config(project_dir, skills={"alpha": alpha.fq})
    (project_dir / "grimoire.lock").write_text('[metadata]\nlock_version = "\x1b[2J"\n', encoding="utf-8", newline="\n")

    result = grim_at(project_dir).run("update", check=False)

    assert result.returncode == 78, result.stderr
    assert "\x1b" not in result.stderr
    assert "\\u{1b}[2J" in result.stderr


def test_a_declared_name_cannot_forge_a_log_line(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A key a cloned repository chose reaches a warning with its line break spelled out."""
    hook = make_hook(f"{unique_repo}/known-guard", "known-guard", tag="1")
    (project_dir / "grimoire.toml").write_text(
        f'[hooks]\n"bad\\n ERROR grim: forged" = "{hook.fq}"\n', encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)
    runner.run("lock")

    result = runner.run("install", check=False)

    assert "bad\\n ERROR grim: forged" in result.stderr, result.stderr
    assert not any(
        line.startswith(" ERROR grim: forged") for line in result.stderr.splitlines()
    ), result.stderr


def _armed(runner: GrimRunner) -> set[str]:
    path = runner.grim_home / "hooks" / "dispatch.json"
    if not path.is_file():
        return set()
    table = json.loads(path.read_text(encoding="utf-8"))
    return {
        row["artifact"] for root in table["roots"].values() for row in root["hooks"]
    }


def test_named_update_keeps_an_unnamed_config_registration(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Vendor sync adds nothing outside the closure, and `retired` holds only what the run moved."""
    guide = make_artifact(
        f"{unique_repo}/guide",
        "rule",
        {"guide.md": "# guide\n", "guide/more.md": "# more\n"},
        tag="1",
    )
    carrier = _skill(unique_repo, "carrier")
    write_config(project_dir, skills={"carrier": carrier.fq}, rules={"guide": guide.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    settings = project_dir / ".claude/settings.json"
    excludes = json.loads(settings.read_text(encoding="utf-8"))["claudeMdExcludes"]
    assert excludes, "the support directory must register an exclusion"
    _move_tag(unique_repo, "carrier")

    result = runner.run("update", "--", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    assert json.loads(settings.read_text(encoding="utf-8"))["claudeMdExcludes"] == excludes


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
def test_named_update_keeps_an_unnamed_hook_armed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A hook outside the closure keeps the dispatch row it had."""
    known = make_hook(f"{unique_repo}/known-guard", "known-guard", tag="1")
    carrier = _skill(unique_repo, "carrier")
    runner = grim_at(project_dir)
    runner.run("init")
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("add", "--kind", "hook", known.fq)
    runner.run("add", "--kind", "skill", carrier.fq)
    assert _armed(runner) == {"known-guard"}
    _move_tag(unique_repo, "carrier")

    result = runner.run("update", "--", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    assert _armed(runner) == {"known-guard"}


def _hook_surface(scope: Scope) -> Path:
    """Where Claude's hook registrations live at this scope."""
    if scope.flags:
        return scope.skills.parent / "settings.json"
    return scope.config_dir / ".claude" / "settings.local.json"


def _drop_matcher_group(surface: Path, matcher: str) -> None:
    doc = json.loads(surface.read_text(encoding="utf-8"))
    groups = doc["hooks"]["PreToolUse"]
    doc["hooks"]["PreToolUse"] = [g for g in groups if g.get("matcher") != matcher]
    surface.write_text(json.dumps(doc, indent=2), encoding="utf-8", newline="\n")


def _drop_dispatch_rows(table: Path, artifact: str) -> None:
    doc = json.loads(table.read_text(encoding="utf-8"))
    for root in doc["roots"].values():
        root["hooks"] = [row for row in root["hooks"] if row["artifact"] != artifact]
    table.write_text(json.dumps(doc), encoding="utf-8", newline="\n")


def _bytes_or_none(path: Path) -> bytes | None:
    return path.read_bytes() if path.is_file() else None


def _tree_bytes(root: Path) -> dict[str, bytes]:
    return {
        str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()
    }


def _two_armed_hooks(
    scope: Scope, unique_repo: str, *, gamma_tier: str = "gatekeeper"
) -> tuple[Path, Path]:
    """Arm `beta` at Bash and `gamma` at Write beside skill `carrier`.

    Returns the claude registration file and the dispatch table.
    """
    carrier = _skill(unique_repo, "carrier")
    beta = make_hook(f"{unique_repo}/beta", "beta", tag="1", matcher="Bash")
    gamma = make_hook(f"{unique_repo}/gamma", "gamma", tag="1", matcher="Write", tier=gamma_tier)
    (scope.config_dir / "grimoire.toml").write_text(
        "[options.experimental]\nhooks = true\n\n"
        f'[skills]\ncarrier = "{carrier.fq}"\n\n'
        f'[hooks]\nbeta = "{beta.fq}"\ngamma = "{gamma.fq}"\n', encoding="utf-8", newline="\n"
    )
    scope.run("lock")
    if not scope.flags:
        scope.run("hook", "allow")
    scope.run("install", "--client", "claude")
    surface = _hook_surface(scope)
    assert {"beta", "gamma"} <= _armed(scope.runner), "precondition: both hooks arm"
    assert surface.is_file(), "precondition: both hooks register"
    return surface, scope.runner.grim_home / "hooks" / "dispatch.json"


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
@pytest.mark.parametrize(
    "damage", ["surface-deleted", "registration-removed", "rows-removed", "both-removed"]
)
def test_named_update_leaves_an_unnamed_hooks_arming_as_it_found_it(
    scope: Scope, registry: str, unique_repo: str, damage: str
) -> None:
    """A named run writes no unnamed hook's registration, dispatch row or payload.

    The registration file, `beta`'s registration element and `beta`'s dispatch
    rows stay as the user left them, byte for byte, while the run updates the
    skill it names.
    """
    surface, table = _two_armed_hooks(scope, unique_repo)
    if damage == "surface-deleted":
        surface.unlink()
    if damage in ("registration-removed", "both-removed"):
        _drop_matcher_group(surface, "Bash")
    if damage in ("rows-removed", "both-removed"):
        _drop_dispatch_rows(table, "beta")
    hooks = scope.runner.grim_home / "hooks"
    payload = next(p.parent for p in hooks.rglob("hook.toml") if p.parent.name == "beta")
    before = (_bytes_or_none(surface), table.read_bytes(), _tree_bytes(payload))
    _move_tag(unique_repo, "carrier")

    result = scope.run("update", "--client", "claude", "--", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    after = (_bytes_or_none(surface), table.read_bytes(), _tree_bytes(payload))
    assert after[0] == before[0], f"the registration file changed\n{result.stderr}"
    assert after[1] == before[1], f"the dispatch table changed\n{result.stderr}"
    assert after[2] == before[2], "the unnamed hook's payload changed"
    assert "hook registrations" not in result.stderr, result.stderr


_UNREGISTERED = "no client registration"


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
def test_named_update_warns_for_an_unregistered_gatekeeper_outside_its_closure(
    scope: Scope, registry: str, unique_repo: str
) -> None:
    """A gatekeeper with a row and no registration is one no client asks, so stderr says it.

    The run still writes nothing of its own: `beta`'s registration stays gone,
    byte for byte, and it exits 0. Observer `gamma` is in the same state and gets
    no line, since `grim status` reads the table alone and a disable is the
    user's to make.
    """
    surface, table = _two_armed_hooks(scope, unique_repo, gamma_tier="observer")
    intact = scope.run("update", "--client", "claude", "--", "carrier", check=False)
    assert _UNREGISTERED not in intact.stderr, intact.stderr
    _drop_matcher_group(surface, "Bash")
    _drop_matcher_group(surface, "Write")
    hooks = scope.runner.grim_home / "hooks"
    payload = next(p.parent for p in hooks.rglob("hook.toml") if p.parent.name == "beta")
    before = (surface.read_bytes(), table.read_bytes(), _tree_bytes(payload))
    _move_tag(unique_repo, "carrier")

    result = scope.run("update", "--client", "claude", "--", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    assert (surface.read_bytes(), table.read_bytes(), _tree_bytes(payload)) == before, result.stderr
    warning = [line for line in result.stderr.splitlines() if _UNREGISTERED in line]
    assert len(warning) == 1, result.stderr
    assert "hook 'beta'" in warning[0], warning[0]
    assert "gamma" not in warning[0], warning[0]
    assert f"`{' '.join(('grim', *scope.flags))} install`" in warning[0], warning[0]
    if not scope.flags:
        config = scope.config_dir / "grimoire.toml"
        named = scope.run("--config", str(config), "update", "--client", "claude", "--", "carrier", check=False)
        assert f"`grim --config {config} install`" in named.stderr, named.stderr

    scope.run("install", "--client", "claude")
    healed = scope.run("update", "--client", "claude", "--", "carrier", check=False)

    assert _UNREGISTERED not in healed.stderr, healed.stderr


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
@pytest.mark.parametrize("damage", ["registration-removed", "rows-removed"])
def test_uninstall_leaves_another_hooks_arming_as_it_found_it(
    scope: Scope, registry: str, unique_repo: str, damage: str
) -> None:
    """A delete removes its own hook's row and element and writes nothing for another."""
    surface, table = _two_armed_hooks(scope, unique_repo)
    if damage == "registration-removed":
        _drop_matcher_group(surface, "Bash")
    else:
        _drop_dispatch_rows(table, "beta")

    result = scope.run("uninstall", "hook", "gamma", check=False)

    assert result.returncode == 0, result.stderr
    groups = json.loads(surface.read_text(encoding="utf-8")).get("hooks", {}).get("PreToolUse", [])
    left = ({g.get("matcher") for g in groups}, _armed(scope.runner))
    assert left == (({"Bash"}, set()) if damage == "rows-removed" else (set(), {"beta"})), surface.read_text(encoding="utf-8")


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
def test_uninstall_warns_for_an_unregistered_gatekeeper(
    scope: Scope, registry: str, unique_repo: str
) -> None:
    """A delete writes no other hook's registration back, so stderr names a gatekeeper left without one.

    A client never runs such a gatekeeper while `grim status` reads it installed.
    Observer `gamma` in the same state gets no line, as on a named update.
    """
    surface, _ = _two_armed_hooks(scope, unique_repo, gamma_tier="observer")
    _drop_matcher_group(surface, "Bash")
    _drop_matcher_group(surface, "Write")

    result = scope.run("uninstall", "skill", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    warning = [line for line in result.stderr.splitlines() if _UNREGISTERED in line]
    assert len(warning) == 1, result.stderr
    assert "hook 'beta'" in warning[0], warning[0]
    assert "gamma" not in warning[0], warning[0]
    assert f"`{' '.join(('grim', *scope.flags))} install`" in warning[0], warning[0]


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
@pytest.mark.parametrize(
    "command", [("update",), ("install",), ("install", "--force")], ids=["update", "install", "install-force"]
)
def test_a_run_with_no_names_converges_what_a_named_run_left(
    scope: Scope, registry: str, unique_repo: str, command: tuple[str, ...]
) -> None:
    """The closure scopes a named run alone: any other run heals every hook."""
    surface, table = _two_armed_hooks(scope, unique_repo)
    surface.unlink()
    _drop_dispatch_rows(table, "beta")
    _move_tag(unique_repo, "carrier")
    scope.run("update", "--client", "claude", "--", "carrier")
    assert not surface.exists(), "precondition: the named run wrote no registration"
    assert "beta" not in _armed(scope.runner), "precondition: the named run armed no row"

    result = scope.run(*command, "--client", "claude", check=False)

    assert result.returncode == 0, result.stderr
    assert {"beta", "gamma"} <= _armed(scope.runner), result.stderr
    groups = json.loads(surface.read_text(encoding="utf-8"))["hooks"]["PreToolUse"]
    assert {g["matcher"] for g in groups} == {"Bash", "Write"}, groups


@pytest.mark.skipif(os.name == "nt", reason="hook arming is POSIX-only here")
def test_named_update_with_a_new_hook_gates_every_hook(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A hook outside the closure loses its arming, and only stderr can say so."""
    known = make_hook(f"{unique_repo}/known-guard", "known-guard", tag="1")
    make_hook(f"{unique_repo}/new-guard", "new-guard", tag="1")
    carrier = _skill(unique_repo, "carrier")
    runner = grim_at(project_dir)
    runner.run("init")
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("add", "--kind", "hook", known.fq)
    runner.run("add", "--kind", "skill", carrier.fq)
    assert _armed(runner) == {"known-guard"}
    retag(f"{unique_repo}/carrier", "keep", carrier.digest)
    make_dependent(
        f"{unique_repo}/carrier",
        "1",
        "skill",
        _files("carrier", "v2"),
        [("hook", "new-guard", "./new-guard:1")],
    )

    result = runner.run("update", "--", "carrier", check=False)

    assert result.returncode == 0, result.stderr
    assert _armed(runner) == set(), "consent drift gates every hook in the workspace"
    assert "known-guard" in result.stderr, result.stderr
    status = _rows(runner.run("status", format="json", check=False))
    assert status["known-guard"]["arming"][0]["cause"] == "consent-drifted", status
