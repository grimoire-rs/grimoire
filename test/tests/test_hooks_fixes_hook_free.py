# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A hook-free project with the flag off stays free of hook output and hook
writes after another workspace on the machine has armed a hook.

The `grim-home-in-workspace` case is a dotfiles repository at `$HOME`, where
`GRIM_HOME=~/.grimoire` sits inside the workspace: that workspace may never arm
a hook, but it has none to arm, so it must not be told so.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    os.name == "nt",
    reason="the hook launcher and its registered command are POSIX-only in v1",
)

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

SKILL_MD = "---\nname: greeter\ndescription: Says hello.\n---\nSay hello.\n"


def _snapshot(root: Path) -> dict[str, tuple[int, bytes]]:
    """Every entry under `root`, directories included: a lock sidecar created
    and unlinked again still moves its directory's mtime."""
    return {
        p.relative_to(root).as_posix(): (
            p.stat().st_mtime_ns,
            p.read_bytes() if p.is_file() else b"",
        )
        for p in [root, *root.rglob("*")]
    }


@pytest.mark.parametrize(
    "nested", [True, False], ids=["grim-home-in-workspace", "plain"]
)
def test_a_hook_free_project_never_hears_of_another_workspaces_hooks(
    grim_binary: Path, tmp_path: Path, unique_repo: str, nested: bool
) -> None:
    hook_free = tmp_path / "dotfiles"
    armed = tmp_path / "armed"
    for workspace in (hook_free, armed):
        (workspace / ".claude").mkdir(parents=True)
    grim_home = hook_free / ".grimoire" if nested else tmp_path / "grim-home"
    grim_home.mkdir(parents=True)

    hook = make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {
            "shell-guard/hook.toml": HOOK_TOML,
            "shell-guard/guard.sh": "#!/bin/sh\ncat > /dev/null\necho '{}'\n",
        },
        tag="1",
    )
    write_config(armed, hooks={"shell-guard": hook.fq})
    other = GrimRunner(grim_binary, grim_home, cwd=armed)
    other.run("config", "set", "options.experimental.hooks", "true")
    other.run("lock")
    other.run("hook", "allow")
    other.run("install")
    table = grim_home / "hooks" / "dispatch.json"
    assert json.loads(table.read_text())["roots"], (
        "precondition: another workspace armed a hook"
    )

    skill = make_artifact(
        f"{unique_repo}/greeter", "skill", {"greeter/SKILL.md": SKILL_MD}, tag="1"
    )
    write_config(hook_free, skills={"greeter": skill.fq})
    runner = GrimRunner(grim_binary, grim_home, cwd=hook_free)
    runner.run("lock")
    before = _snapshot(grim_home / "hooks")

    for command in ("install", "update", "status"):
        result = runner.run(command)
        # The test's own tmp path spells "hook".
        output = (result.stdout + result.stderr).replace(str(tmp_path), "<tmp>")
        assert "hook" not in output.lower(), (
            f"grim {command} in a hook-free project:\n{output}"
        )
        assert json.loads(runner.run(command, format="json").stdout), command

    assert (hook_free / ".claude" / "skills" / "greeter" / "SKILL.md").is_file(), (
        "POSITIVE CONTROL: the skill installed"
    )
    assert _snapshot(grim_home / "hooks") == before, (
        "a hook-free project must write nothing hook-related"
    )
    assert not (hook_free / ".claude" / "settings.local.json").exists()
