# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim remove hook` then `grim install` disarms the removed hook in a
consented workspace.

`remove` keeps the install record and the payload, so convergence must arm
only what the lock still declares. The workspace is consented through
`grim hook allow`: without a consent record the follow-up install disarms
everything for the wrong reason and hides the bug.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

HOOK_TOML = """\
schema = 1
name = "{name}"
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


def _publish(unique_repo: str, name: str, log: Path):
    guard = f"#!/bin/sh\ncat > /dev/null\necho {name} >> '{log}'\necho '{{}}'\n"
    return make_artifact(
        f"{unique_repo}/{name}",
        "hook",
        {f"{name}/hook.toml": HOOK_TOML.format(name=name), f"{name}/guard.sh": guard},
        tag="1",
    )


def _table(runner) -> Path:
    return Path(runner.grim_home) / "hooks" / "dispatch.json"


def _armed(runner) -> list[str]:
    if not _table(runner).is_file():
        return []
    roots = json.loads(_table(runner).read_text())["roots"].values()
    return sorted(row["artifact"] for root in roots for row in root["hooks"])


def _claude_marked(project_dir: Path) -> bool:
    surface = project_dir / ".claude" / "settings.local.json"
    return surface.is_file() and MARKER in surface.read_text()


def _fire(runner, project_dir: Path, token: str, log: Path) -> list[str]:
    log.write_text("")
    envelope = {
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
        "cwd": str(project_dir),
        "session_id": "s",
    }
    fired = runner.run(
        "hook", "run", "--client", "claude", "--event", "PreToolUse",
        "--table", str(_table(runner)), "--root", token,
        stdin=json.dumps(envelope), check=False,
    )
    assert fired.returncode == 0, fired.stderr
    return sorted(log.read_text().split())


def _declared_hooks(runner) -> tuple[list[str], list[str]]:
    status = sorted(i["name"] for i in runner.json("status")["items"] if i["kind"] == "hook")
    listed = sorted({i["artifact"] for i in runner.json("hook", "list")["items"]})
    return status, listed


@pytest.mark.parametrize("install", [(), ("--trust-hooks",)], ids=["consent", "trust-hooks"])
def test_remove_then_install_disarms_the_removed_hook(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, install: tuple[str, ...]
) -> None:
    log = tmp_path / "fired.log"
    guard = _publish(unique_repo, "shell-guard", log)
    gate = _publish(unique_repo, "fmt-gate", log)
    write_config(project_dir, hooks={"shell-guard": guard.fq, "fmt-gate": gate.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    assert _armed(runner) == ["fmt-gate", "shell-guard"] and _claude_marked(project_dir), "precondition: both arm"
    [token] = json.loads(_table(runner).read_text())["roots"]
    assert _fire(runner, project_dir, token, log) == ["fmt-gate", "shell-guard"], "POSITIVE CONTROL: both fire"

    runner.run("remove", "hook", "shell-guard")
    runner.run("install", *install)

    assert _armed(runner) == ["fmt-gate"], "the removed hook must leave the dispatch table"
    assert _fire(runner, project_dir, token, log) == ["fmt-gate"], "the removed hook must not fire"
    assert _claude_marked(project_dir), "a surviving hook keeps the registration"
    assert _declared_hooks(runner) == (["fmt-gate"], ["fmt-gate"])

    runner.run("remove", "hook", "fmt-gate")
    runner.run("install", *install)

    assert _armed(runner) == []
    assert _fire(runner, project_dir, token, log) == []
    assert not _claude_marked(project_dir), "the last hook gone, the registration is reaped"
    assert _declared_hooks(runner) == ([], [])
