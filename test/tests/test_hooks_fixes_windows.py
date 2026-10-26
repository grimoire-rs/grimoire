# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A client outside `HOOKS_WINDOWS_VERIFIED` never arms on Windows.

Qoder, Copilot and Codex have no observed Windows launch path (the last two
passed only emulated launches), so they keep `platform-unsupported` while
Claude, the one admitted client, arms on the same machine. grim registers nothing a
client might fail to run: a non-zero `preToolUse` exit denies the tool call.
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

ARMED = ".claude/settings.json"
DECLINED = {
    "qoder": ".qoder/settings.json",
    "copilot": ".copilot/hooks/grim.json",
    "codex": ".codex/hooks.json",
}


def test_windows_declines_a_client_that_has_not_passed_acceptance(grim_at, project_dir: Path, unique_repo: str) -> None:
    hook = make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": "#!/bin/sh\ncat > /dev/null\n"},
        tag="1",
    )
    runner = grim_at(project_dir)
    home = Path(runner.home)
    (home / ".claude").mkdir()
    (home / ".qoder").mkdir()
    (home / ".copilot" / "skills").mkdir(parents=True)
    (home / ".codex").mkdir()
    # The runner's env holds only USERPROFILE; a local GRIM_HOME must sit under the profile or LOCALAPPDATA.
    runner.env["LOCALAPPDATA"] = os.environ["LOCALAPPDATA"]

    runner.run("config", "set", "options.experimental.hooks", "true", "--global")
    added = runner.run("add", "--kind", "hook", hook.fq, "--global", "--trust-hooks")

    assert "grim-hook" in (home / ARMED).read_text(encoding="utf-8"), "the admitted client did not arm"
    for client, surface in DECLINED.items():
        declined = home / surface
        assert not declined.is_file() or "grim-hook" not in declined.read_text(encoding="utf-8"), f"{client} was registered"

    (item,) = runner.json("hook", "list", "--global")["items"]
    causes = {a["client"]: a["cause"] for a in item["arming"]}
    for client in DECLINED:
        assert causes[client] == "platform-unsupported", item
    assert "claude" not in causes, item

    (row,) = [i for i in runner.json("status", "--global")["items"] if i["kind"] == "hook"]
    status_causes = {a["client"]: a["cause"] for a in row["arming"]}
    for client in DECLINED:
        assert status_causes[client] == "platform-unsupported", row
        message = f"hooks not armed for {client}: grim has not verified hooks for this client on Windows"
        assert added.stderr.count(message) == 1, added.stderr
    assert "hooks not armed for claude" not in added.stderr, added.stderr
