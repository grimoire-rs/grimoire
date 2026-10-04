# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim build` refuses a hook `timeout` below one second.

The dispatcher derives each tool call's time budget from the entries'
timeouts, so a zero would leave no time to start any hook. `hook.toml` freezes
once published, which is why the floor is a build refusal and not a runtime
clamp.
"""

from __future__ import annotations

from pathlib import Path

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "python3 guard.py"
timeout = {timeout}
"""


def _hook_dir(root: Path, timeout: int) -> Path:
    hook = root / "shell-guard"
    hook.mkdir()
    (hook / "hook.toml").write_text(HOOK_TOML.format(timeout=timeout))
    (hook / "guard.py").write_text("import sys\nsys.stdin.read()\nprint('{}')\n")
    return hook


def test_build_refuses_a_zero_timeout(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)

    refused = runner.run("build", str(_hook_dir(project_dir, 0)), check=False)

    assert refused.returncode == 65, refused.stderr
    assert "timeout" in refused.stderr and "guard" in refused.stderr, refused.stderr


def test_build_accepts_the_one_second_floor(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)

    assert runner.json("build", str(_hook_dir(project_dir, 1)))["kind"] == "hook"


def test_build_refuses_a_timeout_above_the_ceiling(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)

    refused = runner.run("build", str(_hook_dir(project_dir, 9223372036854775807)), check=False)

    assert refused.returncode == 65, refused.stderr
    assert "timeout" in refused.stderr and "3600" in refused.stderr, refused.stderr
