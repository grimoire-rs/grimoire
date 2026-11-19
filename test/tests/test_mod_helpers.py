# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-test of the mod fixtures: a declared mod with the feature flag off
locks, records its clients, and writes nothing to any client root."""

from __future__ import annotations

from pathlib import Path

import tomllib

from src.helpers import make_artifact, make_mod, mod_sandbox, write_config


def test_declared_mod_with_the_flag_off_locks_and_writes_nothing(
    grim_at, project_dir: Path, unique_repo: str, tmp_path: Path
) -> None:
    mod = make_mod(f"{unique_repo}/probe-mod", "1")
    skill = make_artifact(
        f"{unique_repo}/probe-skill",
        "skill",
        {"probe-skill/SKILL.md": "# probe\n"},
        tag="1",
    )
    write_config(
        project_dir,
        skills={"probe-skill": skill.fq},
        mods={"probe-mod": mod.fq},
        experimental_mods=False,
    )
    env, roots = mod_sandbox(tmp_path / "sandbox")
    runner = grim_at(project_dir)
    runner.env.update(env)

    runner.run("lock")
    result = runner.run("install")
    assert result.returncode == 0

    lock = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    rows = lock["mod"]
    assert len(rows) == 1
    assert rows[0]["clients"] == ["claude", "opencode"]

    # Positive control: the skill declared in the same run is written.
    assert any(project_dir.glob(".claude/skills/probe-skill/SKILL.md"))

    # Nothing of the mod under either client root, in the project or the sandbox.
    scanned = [project_dir / ".claude", project_dir / ".opencode", *roots.values()]
    for root in scanned:
        if root.is_dir():
            leaked = [
                p
                for p in root.rglob("*")
                if "probe-mod" in p.as_posix() or p.name in ("index.js", "index.ts")
            ]
            assert leaked == [], f"mod files written under {root}: {leaked}"
