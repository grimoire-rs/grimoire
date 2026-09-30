# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
# doc: guide-agent-hooks
"""The "Control agent commands with hooks" guide, run from the page itself.

Every fence the page binds with ``<!-- doc: guide-agent-hooks -->`` is read
out of the markdown at test time: the ``toml`` and ``python`` fences become
the hook's files, and each ``bash-run`` fence is executed. So a reader copying
the page runs what this test ran, and an edit to the page is an edit to the
test.

The page teaches using a hook before authoring one, but using needs a
published hook, so the "Author your own hook" fences run first. Both of the
page's references, the team's ``ghcr.io/acme`` one and the reader's
``localhost:5000`` one, are swapped for one repository on the suite's own
registry.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
from pathlib import Path

import pytest

from src.helpers import PROJECT_ROOT
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    os.name == "nt",
    reason="the registered launcher is a POSIX shell script (v1 launcher scope)",
)

PAGE = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "guides" / "agent-hooks.md"
KEY = "guide-agent-hooks"
PAGE_REFS = ("ghcr.io/acme/hooks/approve-safe-commands", "localhost:5000/hooks/approve-safe-commands")
AUTHOR_HEADING = "## Author your own hook"
REASON = "approve-safe-commands: on this project's safe list"

_BOUND_FENCE = re.compile(
    rf"^[ \t]*<!-- doc: {re.escape(KEY)} -->\n[ \t]*```(?P<lang>[\w-]+)\n(?P<body>.*?)\n[ \t]*```",
    re.MULTILINE | re.DOTALL,
)


def _bound_fences(text: str) -> list[tuple[str, str]]:
    """``(lang, body)`` for every fence ``text`` binds to this test, in order.

    List-item fences are indented; the indent is stripped so the files land
    on disk exactly as a reader who copies them gets them.
    """
    fences = []
    for m in _BOUND_FENCE.finditer(text):
        lines = m.group("body").splitlines()
        indent = min((len(x) - len(x.lstrip()) for x in lines if x.strip()), default=0)
        fences.append((m.group("lang"), "\n".join(x[indent:] for x in lines) + "\n"))
    return fences


def _fire(project_dir: Path, runner: GrimRunner, command: str) -> subprocess.CompletedProcess[str]:
    """Run Claude's registered PreToolUse command the way Claude runs it."""
    settings = json.loads((project_dir / ".claude" / "settings.local.json").read_text())
    (registered,) = [
        h["command"]
        for group in settings["hooks"]["PreToolUse"]
        if group["matcher"] == "Bash"
        for h in group["hooks"]
    ]
    envelope = json.dumps(
        {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "cwd": str(project_dir),
            "session_id": "guide-1",
        }
    )
    return subprocess.run(
        ["sh", "-c", registered],
        input=envelope,
        capture_output=True,
        text=True,
        env=runner.env,
        cwd=project_dir,
        check=False,
    )


def test_the_guide_runs_end_to_end(grim_at, project_dir: Path, registry: str, unique_repo: str) -> None:
    use_text, author_text = PAGE.read_text(encoding="utf-8").split(AUTHOR_HEADING, 1)
    author, use = _bound_fences(author_text), _bound_fences(use_text)
    files = {lang: body for lang, body in author if lang in ("toml", "python")}
    commands = [body.strip() for lang, body in author + use if lang == "bash-run"]
    assert set(files) == {"toml", "python"}, f"the page lost a hook file fence: {sorted(files)}"
    assert len(commands) == 8, f"the page gained or lost a bound command: {commands}"

    source = project_dir / "approve-safe-commands"
    source.mkdir()
    (source / "hook.toml").write_text(files["toml"])
    (source / "approve.py").write_text(files["python"])

    runner = grim_at(project_dir)
    runner.run("init")
    # A per-test repository on the suite's registry, so parallel runs never
    # share a tag.
    test_ref = f"{registry}/{unique_repo}/hooks/approve-safe-commands"

    for command in commands:
        if command.startswith("grim "):
            for page_ref in PAGE_REFS:
                command = command.replace(page_ref, test_ref)
            result = runner.run(*shlex.split(command)[1:])
        else:
            result = subprocess.run(
                ["sh", "-c", command],
                capture_output=True,
                text=True,
                env=runner.env,
                cwd=project_dir,
                check=False,
            )
        assert result.returncode == 0, f"{command}\nstderr: {result.stderr}"

        if '"task verify"' in command:
            assert json.loads(result.stdout) == {"decision": "allow", "reason": REASON}
        elif "approve.py" in command:
            assert json.loads(result.stdout) == {}
        elif command == "grim hook list":
            listed = runner.json("hook", "list")["items"]
            assert [i["state"] for i in listed] == ["installed"], listed
            assert all(a["client"] != "claude" for a in listed[0]["arming"]), listed

            # "Watch the prompt disappear": the approval reaches Claude
            # in Claude's shape, through the launcher the install registered.
            approved = _fire(project_dir, runner, "task verify")
            assert approved.returncode == 0, approved.stderr
            assert json.loads(approved.stdout) == {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": REASON,
                }
            }, approved.stdout
            unlisted = _fire(project_dir, runner, "task verify && git push")
            assert unlisted.returncode == 0, unlisted.stderr
            assert "permissionDecision" not in unlisted.stdout, unlisted.stdout
        elif command.startswith("grim uninstall"):
            assert runner.json("hook", "list")["items"] == []
            settings = json.loads((project_dir / ".claude" / "settings.local.json").read_text())
            assert settings.get("hooks", {}) == {}, settings
