# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim install` of dependent artifacts: the lock's edges must match the pin.

A dependent artifact publishes its requires as a second layer of the pinned
manifest. `install` compares that digest-verified layer with the lock entry's
`requires` and refuses any difference before writing anything: a lock written
by 0.15.1 (no `requires`), a hand-edited lock, an unverified offline carry.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

from src.helpers import make_artifact, make_dependent, write_config

SKILL_DOC = "---\nname: dep\ndescription: d\n---\n# Dep\n"
BASE_DOC = "---\nname: base\ndescription: b\n---\n# Base\n"
REQUIRES_BLOCK = re.compile(r'\n\[\[skill\.requires\]\]\nkind = "[^"]*"\nname = "[^"]*"\nid = "[^"]*"\n')
EDGE = ("skill", "base", "./base:1")


def _lock_entry(kind: str, name: str, id_: str) -> str:
    return f'\n[[skill.requires]]\nkind = "{kind}"\nname = "{name}"\nid = "{id_}"\n'


def _snapshot(*roots: Path) -> dict[str, bytes]:
    """Every file under ``roots`` with its bytes; a missing root is empty."""
    seen: dict[str, bytes] = {}
    for root in roots:
        if root.is_dir():
            for path in sorted(root.rglob("*")):
                if path.is_file() and not path.is_symlink():
                    seen[str(path)] = path.read_bytes()
    return seen


def _stale_error(result) -> dict:
    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert error["exit"] == 65 and error["reason"] == "stale-lock", error
    return error


def _stale(result) -> dict:
    error = _stale_error(result)
    assert (
        "skill 'dep': the lock's dependencies do not match the pinned artifact; run `grim lock`"
        in error["message"]
    ), error
    return error


def test_a_config_install_names_its_config_in_the_stale_remedy(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Followed from the shell's own project, a plain `grim lock` relocks that one."""
    dep = make_dependent(
        f"{unique_repo}/dep", "1", "skill", {"dep/SKILL.md": SKILL_DOC}, [EDGE]
    )
    make_artifact(f"{unique_repo}/base", "skill", {"base/SKILL.md": BASE_DOC}, tag="1")
    config = write_config(project_dir, skills={"dep": dep.fq})
    grim_at(project_dir).run("lock")
    lock = project_dir / "grimoire.lock"
    lock.write_text(REQUIRES_BLOCK.sub("\n", lock.read_text()))
    away = tmp_path / "away"
    away.mkdir()

    result = grim_at(away).run(
        "--format", "json", "--config", str(config), "install", check=False
    )

    message = _stale_error(result)["message"]
    assert f"do not match the pinned artifact; run `grim --config {config} lock`" in message


def test_install_refuses_mismatched_requires(
    grim_at, project_dir: Path, grim_home: Path, unique_repo: str
) -> None:
    dep = make_dependent(
        f"{unique_repo}/dep", "1", "skill", {"dep/SKILL.md": SKILL_DOC}, [EDGE]
    )
    make_artifact(f"{unique_repo}/base", "skill", {"base/SKILL.md": BASE_DOC}, tag="1")
    write_config(project_dir, skills={"dep": dep.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"
    # What 0.15.1 writes for the same project: the entries without their edges.
    zero_fifteen_lock = REQUIRES_BLOCK.sub("\n", lock.read_text())
    assert "requires" not in zero_fifteen_lock, "a 0.15.1-style lock carries no edges"
    state_roots = (project_dir / ".grimoire", grim_home / "state", grim_home / "hooks")
    before = _snapshot(project_dir / ".claude", *state_roots)

    # A lock without the edge the pinned artifact declares.
    lock.write_text(zero_fifteen_lock)
    _stale(runner.run("--format", "json", "install", check=False))

    # A hand-edited lock naming an edge the pinned artifact does not declare.
    lock.write_text(zero_fifteen_lock + _lock_entry("skill", "other", "./other:1"))
    _stale(runner.run("--format", "json", "install", check=False))

    # The right edge with a different id is also a difference.
    lock.write_text(zero_fifteen_lock + _lock_entry(*(EDGE[0], EDGE[1], "./base:2")))
    _stale(runner.run("--format", "json", "install", check=False))

    assert not (project_dir / ".claude/skills/dep").exists()
    assert _snapshot(project_dir / ".claude", *state_roots) == before, (
        "a refused install must write no destination, state or consent file"
    )

    # Positive control: a lock listing exactly the published edge installs,
    # and only the content layer materializes.
    lock.write_text(zero_fifteen_lock + _lock_entry(*EDGE))
    result = runner.run("--format", "json", "install")
    statuses = {i["name"]: i["status"] for i in json.loads(result.stdout)["items"]}
    assert statuses["dep"] == "installed", statuses
    tree = sorted(
        p.relative_to(project_dir / ".claude/skills/dep").as_posix()
        for p in (project_dir / ".claude/skills/dep").rglob("*")
        if p.is_file()
    )
    assert tree == ["SKILL.md"]
