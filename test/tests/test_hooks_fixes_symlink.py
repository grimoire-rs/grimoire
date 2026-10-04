# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A dotfiles-linked client settings file survives hook arming and disarming.

Issue #117 for the hook registration: `~/.claude/settings.json` managed by
stow/yadm is a symlink, and grim must write the registration into the real
file rather than replace the link with a regular file.
"""
from __future__ import annotations

import os
import shutil
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

pytestmark = pytest.mark.skipif(os.name == "nt", reason="symlinked dotfiles and the hook launcher are POSIX-only")

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

MARKER = "hook-dispatcher"


def test_global_hook_arms_and_disarms_through_a_symlinked_settings_file(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    hook = make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": "#!/bin/sh\ncat > /dev/null\n"},
        tag="1",
    )
    runner = grim_at(project_dir)
    real = Path(runner.home) / "dotfiles" / "claude-settings.json"
    real.parent.mkdir(parents=True)
    real.write_text('{"permissions": {}}\n')
    link = Path(runner.home) / ".claude" / "settings.json"
    link.parent.mkdir(parents=True)
    link.symlink_to(real)

    runner.run("config", "set", "options.experimental.hooks", "true", "--global")
    runner.run("add", "--kind", "hook", hook.fq, "--global")
    assert link.is_symlink(), "arming must keep the dotfiles link"
    assert MARKER in real.read_text(), "the registration lands in the real file"

    runner.run("uninstall", "--global", "hook", "shell-guard")
    assert link.is_symlink(), "disarming must keep the dotfiles link"
    assert MARKER not in real.read_text(), "the registration is reaped from the real file"
    assert '"permissions"' in real.read_text()


def test_a_project_claude_dir_linked_out_of_the_workspace_is_never_written(
    grim_at, project_dir: Path
) -> None:
    """T3: a cloned repo commits `.claude` as a link to another project's
    directory. Even a hook-free install must not reap through it."""
    other = project_dir.parent / f"{project_dir.name}-other" / ".claude"
    other.mkdir(parents=True)
    settings = other / "settings.local.json"
    owned = (
        '{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", '
        f'"command": "x", "com.grimoire.managed": "{MARKER}"}}]}}]}}}}\n'
    )
    settings.write_text(owned)
    shutil.rmtree(project_dir / ".claude", ignore_errors=True)
    (project_dir / ".claude").symlink_to(other)
    write_config(project_dir)

    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")

    assert settings.read_text() == owned, "the other project's registrations survive"
