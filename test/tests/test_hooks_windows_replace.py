# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Replacing the launcher on Windows while it is in use (C-N5, PD-9).

Windows cannot overwrite or delete a running image, but can rename it. The
launcher on disk differs from the embedded one (a staged upgrade), a hook is in
flight, and `grim install` must still replace it: the old image moves aside, the
new one takes its name, the in-flight hook finishes untouched, and the next
install reaps the aside copy.

A handle held without share-delete makes the rename fail with
`ERROR_SHARING_VIOLATION`, which is what an antivirus scan looks like to grim.
Short holds are retried; a hold that outlasts the retries refuses to arm and
leaves the old launcher.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from src.helpers import make_artifact
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(os.name != "nt", reason="Windows rename semantics")

SESSION_START = json.dumps({"hook_event_name": "SessionStart", "cwd": "C:/repo", "session_id": "s-1"})


def _hook_toml() -> str:
    python = Path(sys.executable).as_posix()
    return f"""\
schema = 1
name = "slow-guard"
description = "Observes session starts, slowly."

[[hooks]]
id = "guard"
event = "SessionStart"
tier = "observer"
argv = ["{python}", "${{GRIM_HOOK_DIR}}/guard.py"]
timeout = 30
"""


def _slow_guard(started: Path, finished: Path) -> str:
    return (
        "import sys, time\n"
        "sys.stdin.read()\n"
        f"open({str(started)!r}, 'w').write('x')\n"
        "time.sleep(6)\n"
        f"open({str(finished)!r}, 'w').write('x')\n"
        "sys.stdout.write('{}')\n"
    )


@pytest.fixture()
def armed(grim: GrimRunner, tmp_path: Path):
    """An armed global Claude hook whose payload runs for six seconds."""
    grim.env["LOCALAPPDATA"] = os.environ["LOCALAPPDATA"]
    started, finished = tmp_path / "started", tmp_path / "finished"
    hook = make_artifact(
        f"grim-test/replace-{os.getpid()}-{time.monotonic_ns()}",
        "hook",
        {"slow-guard/hook.toml": _hook_toml(), "slow-guard/guard.py": _slow_guard(started, finished)},
        tag="1",
    )
    (grim.home / ".claude").mkdir(parents=True, exist_ok=True)
    (grim.grim_home / "grimoire.toml").write_text(
        f'[hooks]\nslow-guard = "{hook.fq}"\n\n[options.experimental]\nhooks = true\n'
    )
    grim.run("lock", "--global")
    grim.run("install", "--global", "--trust-hooks")
    launcher = grim.grim_home / "hooks" / "grim-hook.exe"
    assert launcher.is_file()
    return launcher, started, finished


def _registered_argv(grim: GrimRunner) -> list[str]:
    settings = json.loads((grim.home / ".claude" / "settings.json").read_text(encoding="utf-8"))
    commands: list[str] = []

    def walk(node) -> None:
        if isinstance(node, dict):
            if "grim-hook" in str(node.get("command", "")):
                commands.append(node["command"])
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    walk(settings)
    # The home is bare (no space, no quote), so the shell form splits on blanks.
    return commands[0].split(" ")


def _asides(launcher: Path) -> list[Path]:
    return sorted(launcher.parent.glob(".grim-hook.old.*.exe"))


def _launcher_report(grim: GrimRunner) -> dict:
    return grim.json("hook", "list", "--global")["launcher"]


def test_a_running_launcher_is_replaced_by_rename_aside(grim: GrimRunner, armed) -> None:
    launcher, started, finished = armed
    embedded = launcher.read_bytes()
    launcher.write_bytes(embedded + b"\0")  # stale: not the embedded bytes, still a valid image
    assert hashlib.sha256(launcher.read_bytes()).hexdigest() != hashlib.sha256(embedded).hexdigest()

    running = subprocess.Popen(
        _registered_argv(grim), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    assert running.stdin
    running.stdin.write(SESSION_START)
    running.stdin.close()
    deadline = time.monotonic() + 20
    while not started.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    assert started.exists(), "POSITIVE CONTROL: the slow payload never started, so nothing was in flight"

    installed = grim.run("install", "--global", "--trust-hooks")

    assert "hooks not armed for claude" not in installed.stderr, installed.stderr
    assert launcher.read_bytes() == embedded, "the running launcher was not replaced"
    assert _launcher_report(grim)["state"] == "current"
    assert _asides(launcher), "the running image should have been renamed aside, not deleted"
    assert not finished.exists(), "install returned only after the hook finished, so nothing overlapped"

    out, err = running.communicate(timeout=60)
    assert running.returncode == 0, (out, err)
    assert finished.exists(), "the in-flight hook did not run to completion"

    grim.run("install", "--global", "--trust-hooks")
    assert _asides(launcher) == [], "the aside copy should be reaped once its process has exited"
    assert sorted(p.name for p in launcher.parent.glob("*.exe")) == ["grim-hook.exe"]


def _hold_open(path: Path, seconds: float) -> threading.Thread:
    """Hold `path` open without share-delete, as a scanner would."""
    held = threading.Event()

    def hold() -> None:
        with open(path, "rb"):
            held.set()
            time.sleep(seconds)

    thread = threading.Thread(target=hold, daemon=True)
    thread.start()
    assert held.wait(10)
    return thread


def test_a_sharing_violation_shorter_than_the_retries_still_replaces(grim: GrimRunner, armed) -> None:
    launcher, _, _ = armed
    embedded = launcher.read_bytes()
    launcher.write_bytes(embedded + b"\0")

    hold = _hold_open(launcher, 0.7)
    installed = grim.run("install", "--global", "--trust-hooks")
    hold.join()

    assert "hooks not armed for claude" not in installed.stderr, installed.stderr
    assert launcher.read_bytes() == embedded
    assert _launcher_report(grim)["state"] == "current"


def test_a_sharing_violation_that_outlasts_the_retries_refuses_and_keeps_the_old_launcher(
    grim: GrimRunner, armed
) -> None:
    launcher, _, _ = armed
    embedded = launcher.read_bytes()
    stale = embedded + b"\0"
    launcher.write_bytes(stale)

    hold = _hold_open(launcher, 8)
    refused = grim.run("install", "--global", "--trust-hooks")
    assert "hooks not armed for claude" in refused.stderr, refused.stderr
    assert launcher.read_bytes() == stale, "a refused replace must leave the old launcher alone"
    hold.join()

    grim.run("install", "--global", "--trust-hooks")
    assert launcher.read_bytes() == embedded
    assert "grim-hook" in (grim.home / ".claude" / "settings.json").read_text(encoding="utf-8")
