# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Consent record v2: one record at ``$GRIM_HOME/consent/``, no kind in its keys.

The record an older grim left at ``$GRIM_HOME/hooks/consent/`` is never
honoured, and grim deletes the one that names the resolved workspace whenever
it writes or revokes. Every "nothing happened" assertion sits beside a run where
the same thing does happen.
"""

from __future__ import annotations

import hashlib
import json
import os
import select
import subprocess
import time
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_settings, settings_sandbox, write_config
from src.registry import REGISTRY_HOST

pytestmark = pytest.mark.skipif(
    os.name == "nt",
    reason="the hook launcher and its registered command are POSIX-only in v1",
)

pty = pytest.importorskip("pty")

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

GUARD_SH = "#!/bin/sh\ncat > /dev/null\necho '{}'\n"
DOCUMENT = {"description": "Team defaults", "claude": {"model": "sonnet"}}
PROMPT = "[y/N]"
TERMINAL_DEADLINE_SECONDS = 60


def _publish_hook(unique_repo: str):
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def _key(workspace: Path) -> str:
    return hashlib.sha256(str(workspace).encode()).hexdigest()


def _new_path(runner, workspace: Path) -> Path:
    return Path(runner.grim_home) / "consent" / f"{_key(workspace)}.json"


def _old_path(runner, workspace: Path) -> Path:
    return Path(runner.grim_home) / "hooks" / "consent" / f"{_key(workspace)}.json"


def _seed_old(runner, path: Path, recorded: Path, keys: list[str]) -> None:
    """The v1 record an older grim wrote, by hand: the shape is the shipped contract."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "v": 1,
                "workspace": str(recorded),
                "hooks": keys,
                "consented_at": "2026-08-28T10:00:00Z",
            }
        ),
        encoding="utf-8",
        newline="\n",
    )


def _rows(runner) -> list[dict]:
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    table = json.loads(path.read_text(encoding="utf-8"))
    return [row for root in table["roots"].values() for row in root["hooks"]]


def _hook_row(runner) -> dict:
    out = json.loads(runner.run("status", "--format", "json").stdout)
    [row] = [i for i in out["items"] if i["kind"] == "hook"]
    return row


def _declare_hook(grim_at, project_dir: Path, unique_repo: str):
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    return runner, f"shell-guard@{REGISTRY_HOST}/{unique_repo}/shell-guard"


def _on_a_terminal(runner, *args: str, answer: str) -> tuple[int, str]:
    """Run grim with stdin and stderr on a pseudo-terminal and type ``answer`` at the question."""
    master, slave = pty.openpty()
    child = subprocess.Popen(
        [str(runner.binary), *args],
        cwd=runner.cwd,
        env=runner.env,
        stdin=slave,
        stdout=subprocess.DEVNULL,
        stderr=slave,
    )
    os.close(slave)
    transcript = ""
    answered = False
    deadline = time.monotonic() + TERMINAL_DEADLINE_SECONDS
    try:
        while True:
            remaining = deadline - time.monotonic()
            assert remaining > 0, f"grim never finished on its terminal:\n{transcript}"
            if not select.select([master], [], [], remaining)[0]:
                continue
            try:
                chunk = os.read(master, 4096)
            except OSError:  # Linux reports the closed far side as EIO.
                break
            if not chunk:
                break
            transcript += chunk.decode(errors="replace")
            if not answered and PROMPT in transcript:
                os.write(master, f"{answer}\n".encode())
                answered = True
    finally:
        os.close(master)
        if child.poll() is None:
            child.kill()
    return child.wait(timeout=TERMINAL_DEADLINE_SECONDS), transcript


def test_old_record_is_not_honoured(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, key = _declare_hook(grim_at, project_dir, unique_repo)
    old = _old_path(runner, project_dir)
    _seed_old(runner, old, project_dir, [key])

    result = runner.run("install")

    assert result.returncode == 0, result.stderr
    assert _rows(runner) == [], "an older grim's record arms nothing"
    assert _hook_row(runner)["state"] == "gated"
    assert {a["cause"] for a in _hook_row(runner)["arming"]} == {
        "workspace-not-consented"
    }
    assert old.is_file(), "a read-only run leaves the old record alone"
    assert not _new_path(runner, project_dir).exists()

    # Positive control: a record at the new path, with the same entry, arms.
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1


def test_allow_deletes_the_old_record_of_this_workspace(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, key = _declare_hook(grim_at, project_dir, unique_repo)
    old = _old_path(runner, project_dir)
    _seed_old(runner, old, project_dir, [key])

    runner.run("hook", "allow")

    record = json.loads(_new_path(runner, project_dir).read_text(encoding="utf-8"))
    assert record["v"] == 2 and record["workspace"] == str(project_dir)
    assert key in record["sources"], "the entry carries no kind prefix"
    assert not old.exists(), "the write took the old record with it"


def test_old_record_of_another_workspace_stays(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, key = _declare_hook(grim_at, project_dir, unique_repo)
    elsewhere = tmp_path / "elsewhere"
    # Filed under this workspace's key but naming another directory: not this one's.
    foreign = _old_path(runner, project_dir)
    _seed_old(runner, foreign, elsewhere, [key])
    # Another workspace's own record, under its own key.
    other_own = _old_path(runner, elsewhere)
    _seed_old(runner, other_own, elsewhere, [key])

    runner.run("hook", "allow")
    assert _new_path(runner, project_dir).is_file(), (
        "positive control: the write landed"
    )
    assert foreign.is_file(), "a record naming another workspace stays"
    assert other_own.is_file(), "another workspace's record stays"

    runner.run("hook", "revoke")
    assert not _new_path(runner, project_dir).exists(), (
        "positive control: revoke removed the new record"
    )
    assert foreign.is_file() and other_own.is_file()


def test_revoke_deletes_the_old_record(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, key = _declare_hook(grim_at, project_dir, unique_repo)
    runner.run("hook", "allow")
    old = _old_path(runner, project_dir)
    _seed_old(runner, old, project_dir, [key])

    result = runner.run("hook", "revoke", "--format", "json")

    assert json.loads(result.stdout)["action"] == "revoked"
    assert not _new_path(runner, project_dir).exists()
    assert not old.exists(), "a revoke reaches an older grim"


def test_prompt_names_each_kind_and_marks_flag_off(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    hook = _publish_hook(unique_repo)
    package = make_settings(f"{unique_repo}/team", "1", DOCUMENT)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    hook_key = f"shell-guard@{REGISTRY_HOST}/{unique_repo}/shell-guard"
    settings_key = f"team@{REGISTRY_HOST}/{unique_repo}/team"

    # Hooks only: today's wording, no list of kinds.
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    code, plain = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, plain
    assert PROMPT in plain and "Allow hooks from" in plain
    assert "Declared here:" not in plain

    # Settings declared, its flag off: the entry is listed under its kind and marked.
    write_config(
        project_dir, hooks={"shell-guard": hook.fq}, settings={"team": package.fq}
    )
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    code, marked = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, marked
    assert "Declared here:" in marked
    assert f"hook: {hook_key}" in marked
    assert f"settings: {settings_key} (feature off" in marked
    assert f"hook: {hook_key} (feature off" not in marked
    assert "Allow hooks from" in marked

    # Both flags on: both kinds named in the question, neither marked.
    runner.run("config", "set", "options.experimental.settings", "true")
    code, both = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, both
    assert "Allow hooks and settings from" in both
    assert f"hook: {hook_key}" in both and f"settings: {settings_key}" in both
    assert "feature off" not in both
    assert not _new_path(runner, project_dir).exists(), "a decline records nothing"
