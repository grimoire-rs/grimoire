# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Drivers for the workspace-consent question: a pseudo-terminal run and an older record.

The question needs a terminal on stdin and stderr, so ``on_a_terminal`` hands the
child a pseudo-terminal. They live here so no test module imports a sibling one.
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

from src.runner import GrimRunner

PROMPT = "[y/N]"
TERMINAL_DEADLINE_SECONDS = 60


def on_a_terminal(
    runner: GrimRunner, *args: str, answer: str | None = None
) -> tuple[int, str]:
    """Run grim with stdin and stderr on a pseudo-terminal.

    Returns the exit code and everything printed there. ``answer`` is typed
    once the question appears. With no answer, a question nobody expected hangs
    the child until the deadline, which fails the test.
    """
    pty = pytest.importorskip("pty")
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
    answered = answer is None
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


def workspace_key(workspace: Path) -> str:
    return hashlib.sha256(str(workspace).encode()).hexdigest()


def old_record_path(runner: GrimRunner, workspace: Path) -> Path:
    """Where an older grim kept the record for ``workspace``."""
    return (
        Path(runner.grim_home)
        / "hooks"
        / "consent"
        / f"{workspace_key(workspace)}.json"
    )


def seed_old_record(path: Path, recorded: Path, keys: list[str]) -> None:
    """The older record an earlier grim wrote, by hand: the shape is the shipped contract."""
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
