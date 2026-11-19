# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The binding gate: a bad mod name refuses its row before any path is minted."""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_mod, mod_sandbox, write_config

SKILL = "---\nname: control\ndescription: d\n---\n# control\n"
OPENCODE_V2 = {"opencode": ['plugin_api = "v2"']}
BOTH = ("--client", "claude", "--client", "opencode")
BAD = ["nul", "con.x", "Upper", "a@b", "a/b", "a\nb"]


def _setup(project_dir: Path, tmp_path: Path, repo: str, mods: dict, grim_at):
    skill = make_artifact(f"{repo}/control", "skill", {"control/SKILL.md": SKILL}, "1")
    write_config(
        project_dir,
        skills={"control": skill.fq},
        mods={name: m.fq for name, m in mods.items()},
        experimental_mods=True,
        vendors=OPENCODE_V2,
    )
    runner = grim_at(project_dir)
    runner.env.update(mod_sandbox(tmp_path / "sandbox")[0])
    runner.run("lock")
    return runner


@pytest.mark.parametrize("bad", BAD)
def test_bad_binding_refuses_before_any_path(
    bad: str, grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    good = make_mod(f"{unique_repo}/good", "1")
    other = make_mod(f"{unique_repo}/other", "1")
    runner = _setup(
        project_dir, tmp_path, unique_repo, {"good": good, "other": other}, grim_at
    )
    # The row's name is hand-edited in the lock: the one declaring path every
    # source (a `[mods]` key, a bundle member, a `requires` edge) ends in.
    lock = project_dir / "grimoire.lock"
    text = lock.read_bytes().decode("utf-8")
    edited = re.sub(
        r'(\[\[mod\]\]\nname = )"other"', lambda m: m.group(1) + json.dumps(bad), text
    )
    assert edited != text
    lock.write_bytes(edited.encode("utf-8"))

    result = runner.run(
        "install", "--trust-workspace", *BOTH, format="json", check=False
    )

    assert result.returncode == 65, result.stderr
    assert json.loads(result.stdout)["error"]["reason"] == "invalid-mod"
    for tree in (
        project_dir / ".claude" / "mods",
        project_dir / ".opencode" / "plugins",
    ):
        assert (
            not [p for p in tree.rglob("*") if bad in p.parts]
            if tree.exists()
            else True
        )
    assert not list(project_dir.rglob("*Upper*")) and not (project_dir / "a").exists()
    assert (project_dir / ".claude" / "mods" / "good" / "src").is_dir(), (
        "positive control: the other row converged in the same run"
    )


@pytest.mark.parametrize("reserved", ["claude-tools", "anthropic", "cc-plugin-x"])
def test_claude_reserved_binding_declines_claude_only(
    reserved: str, grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/{reserved}", "1")
    runner = _setup(project_dir, tmp_path, unique_repo, {reserved: mod}, grim_at)

    result = runner.run("install", "--trust-workspace", *BOTH)

    lines = [l for l in result.stderr.splitlines() if "not installed for claude" in l]
    assert len(lines) == 1, result.stderr
    assert not (project_dir / ".claude" / "mods").exists()
    assert (project_dir / ".opencode" / "plugins" / reserved / "index.ts").is_file(), (
        "OpenCode gets its files"
    )
