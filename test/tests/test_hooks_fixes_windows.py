# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Hooks never arm on Windows: grim registers nothing a client would fail to run.

The launcher and every registered command are POSIX shell. Copilot runs its
hook command through PowerShell, where that string does not parse, and a
non-zero `preToolUse` exit denies the tool call. So grim declines to arm on
Windows, reports cause `platform-unsupported`, and writes no registration.
"""
from __future__ import annotations

import os
from pathlib import Path

import pytest

from src.helpers import make_artifact

pytestmark = pytest.mark.skipif(os.name != "nt", reason="the decline is Windows-only; POSIX hosts arm")

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

# Where each hook-capable client's global registration would land.
SURFACES = (".claude/settings.json", ".codex/hooks.json", ".copilot/hooks/grim.json", ".qoder/settings.json")


def test_windows_declines_to_arm_with_platform_unsupported(grim_at, project_dir: Path, unique_repo: str) -> None:
    hook = make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": "#!/bin/sh\ncat > /dev/null\n"},
        tag="1",
    )
    runner = grim_at(project_dir)
    home = Path(runner.home)
    (home / ".claude").mkdir()
    (home / ".copilot" / "skills").mkdir(parents=True)

    runner.run("config", "set", "options.experimental.hooks", "true", "--global")
    added = runner.run("add", "--kind", "hook", hook.fq, "--global")

    written = [s for s in SURFACES if (home / s).is_file() and "grim-hook" in (home / s).read_text()]
    assert written == [], f"a hook registration was written: {written}"
    hooks = Path(runner.grim_home) / "hooks"
    for armable in ("bin/grim-hook", "dispatch.json", "root-key"):
        assert not (hooks / armable).exists(), f"{armable} was written"
    assert added.stderr.count("Linux and macOS only") == 1, added.stderr

    (item,) = runner.json("hook", "list", "--global")["items"]
    causes = {a["client"]: a["cause"] for a in item["arming"]}
    assert item["state"] == "not-armed", item
    assert causes["claude"] == causes["copilot"] == "platform-unsupported", item

    (row,) = [i for i in runner.json("status", "--global")["items"] if i["kind"] == "hook"]
    assert {a["cause"] for a in row["arming"]} == {"platform-unsupported"}, row
