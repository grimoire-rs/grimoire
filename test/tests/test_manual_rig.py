# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Structural integrity of the manual rolling-release rig.

The rig scripts under ``test/manual/scripts/`` are documented as
*directly invocable* (each carries a ``#   test/manual/scripts/<name>.sh``
usage header). If one is committed without its execute bit the documented
rolling-release reproduction silently no-ops with exit 126 ("permission
denied") — ``grim release`` never runs, the registry keeps the old
cascade, and the rolling-release feature *appears* broken when it is not.

Regression guard for that mode-bit defect: every shebang-carrying driver
script in the rig must be executable (git mode ``100755``).
"""
from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from src.helpers import PROJECT_ROOT

_RIG_SCRIPTS_DIR = PROJECT_ROOT / "test" / "manual" / "scripts"

# Driver scripts a user (or the documented repro) invokes directly. ``env.sh``
# is sourced, not executed, so it is exempt from the execute-bit contract.
_DRIVER_SCRIPTS = ("bootstrap.sh", "release-update.sh", "teardown.sh")


@pytest.mark.parametrize("name", _DRIVER_SCRIPTS)
def test_rig_driver_script_is_executable_on_disk(name: str) -> None:
    script = _RIG_SCRIPTS_DIR / name
    assert script.is_file(), f"missing rig script {script}"
    assert os.access(script, os.X_OK), (
        f"{script} is not executable; the documented reproduction invokes "
        f"it directly and would fail with exit 126 (permission denied), "
        f"making the rolling-release feature appear broken"
    )


@pytest.mark.parametrize("name", _DRIVER_SCRIPTS)
def test_rig_driver_script_is_executable_in_git(name: str) -> None:
    rel = f"test/manual/scripts/{name}"
    out = subprocess.run(
        ["git", "ls-files", "-s", rel],
        cwd=PROJECT_ROOT,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0 or not out.stdout.strip():
        pytest.skip(f"{rel} not tracked by git in this checkout")
    mode = out.stdout.split()[0]
    assert mode == "100755", (
        f"{rel} is committed with git mode {mode}; rig driver scripts "
        f"must be committed executable (100755) so a fresh checkout can "
        f"run the documented rolling-release reproduction"
    )
