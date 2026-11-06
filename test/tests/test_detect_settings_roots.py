# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Global autodetect from a harness's settings directory alone.

A home can hold a harness's settings directory and nothing else: the
Antigravity CLI writes ``~/.gemini/antigravity-cli`` and Warp on Linux writes
``$XDG_CONFIG_HOME/warp-terminal``. Either must select its client at global
scope, or a settings package for it reads ``gated`` with no client named. The
unit tests cannot fabricate ``$HOME``; the runner's isolated home can.
"""

from __future__ import annotations

import sys
from pathlib import Path

from src.runner import GrimRunner


def _detected(runner: GrimRunner) -> list[str]:
    return runner.json("context", "--global")["clients"]


def test_a_cli_only_antigravity_home_detects_antigravity(
    grim_binary: Path, grim_home: Path
) -> None:
    runner = GrimRunner(grim_binary, grim_home)
    assert "antigravity" not in _detected(runner)

    (runner.home / ".gemini" / "antigravity-cli").mkdir(parents=True)
    assert not (runner.home / ".gemini" / "config").exists()
    assert "antigravity" in _detected(runner)


def test_the_xdg_warp_terminal_dir_detects_warp_on_linux_only(
    grim_binary: Path, grim_home: Path
) -> None:
    """Linux is the one platform whose Warp settings live under XDG, so the
    directory selects Warp there and nowhere else."""
    runner = GrimRunner(grim_binary, grim_home)
    assert "warp" not in _detected(runner)

    (Path(runner.env["XDG_CONFIG_HOME"]) / "warp-terminal").mkdir(parents=True)
    assert not (runner.home / ".warp").exists()
    assert ("warp" in _detected(runner)) == sys.platform.startswith("linux")
