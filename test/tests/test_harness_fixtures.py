# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Smoke tests for the hooks-work test-harness prerequisites.

These exercise the three additions WP-C makes to the shared test harness
(`.agents/plans/plan_hooks_artifact_kind.md` § WP-C) so they are proven to
work end to end rather than merely defined:

- `GrimRunner.run(..., stdin=...)` (`src/runner.py`)
- the `two_projects` fixture (two workspace roots sharing one `$GRIM_HOME`)
- the `hostile_hook_clone` fixture factory (planted dispatch table + sentinel)

None of these tests assert anything about the hooks feature itself — that
lands in later waves (WP-I onward) and its own `test_hooks_*.py` suite
(WP-O). This file only proves the harness plumbing those tests will stand
on is correct today.
"""
from __future__ import annotations

import base64
import json
import subprocess
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING

from src.runner import GrimRunner

if TYPE_CHECKING:
    from conftest import HostileHookClone

# ---------------------------------------------------------------------------
# GrimRunner.run(stdin=...)
# ---------------------------------------------------------------------------


def test_run_stdin_reaches_the_binary(grim: GrimRunner, tmp_path: Path) -> None:
    """`stdin=` on `GrimRunner.run()` is actually piped to the child process.

    `grim login --password-stdin` is the one place in the CLI that reads a
    secret from stdin, so it is the natural probe: if `stdin` were not
    wired through (e.g. silently dropped, or left as the default
    `subprocess.DEVNULL`), `--password-stdin` would read EOF immediately
    and grim would reject an empty password — a `--no-verify` /
    `--allow-insecure-store` run that still comes back non-zero is exactly
    that failure mode.
    """
    docker_config = tmp_path / "docker"
    docker_config.mkdir()
    grim.env["DOCKER_CONFIG"] = str(docker_config)

    result = grim.run(
        "login",
        "-u", "smoketest",
        "--password-stdin",
        "--allow-insecure-store",
        "--no-verify",
        "ghcr.io",
        check=False,
        stdin="hunter2\n",
    )
    assert result.returncode == 0, result.stderr

    cfg = json.loads((docker_config / "config.json").read_text())
    entry = cfg["auths"]["ghcr.io"]["auth"]
    assert base64.b64decode(entry).decode() == "smoketest:hunter2"


def test_run_omits_stdin_by_default(grim: GrimRunner, tmp_path: Path) -> None:
    """No `stdin=` given ⇒ the child's stdin is `DEVNULL`, not inherited.

    `--password-stdin` with nothing piped in must fail fast (empty
    password) rather than block waiting on a TTY that never arrives in a
    test run.
    """
    docker_config = tmp_path / "docker"
    docker_config.mkdir()
    grim.env["DOCKER_CONFIG"] = str(docker_config)

    result = grim.run(
        "login",
        "-u", "smoketest",
        "--password-stdin",
        "--allow-insecure-store",
        "ghcr.io",
        check=False,
    )
    assert result.returncode != 0, "empty/absent stdin must not succeed as a real login"


# ---------------------------------------------------------------------------
# two_projects
# ---------------------------------------------------------------------------


def test_two_projects_share_one_grim_home_but_have_independent_state(
    two_projects: tuple[GrimRunner, GrimRunner],
) -> None:
    runner_a, runner_b = two_projects

    assert runner_a.grim_home == runner_b.grim_home, "fixture must share one GRIM_HOME"
    assert runner_a.cwd != runner_b.cwd, "the two workspace roots must differ"

    runner_a.run("init", check=False)
    runner_b.run("init", check=False)

    config_a = runner_a.cwd / "grimoire.toml"
    config_b = runner_b.cwd / "grimoire.toml"
    assert config_a.is_file()
    assert config_b.is_file()
    assert config_a != config_b, "each workspace root gets its own config"


# ---------------------------------------------------------------------------
# hostile_hook_clone
# ---------------------------------------------------------------------------


def test_hostile_hook_clone_plants_dispatch_table_and_payload(
    hostile_hook_clone: Callable[..., HostileHookClone],
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "hostile-clone"
    clone = hostile_hook_clone(workspace)

    assert clone.workspace.is_dir()
    assert clone.dispatch_table.is_file()
    assert clone.payload.is_file()
    assert not clone.sentinel.exists(), "planting must not itself execute the payload"

    table = json.loads(clone.dispatch_table.read_text())
    assert str(clone.workspace) in table

    payload_source = clone.payload.read_text()
    assert str(clone.sentinel) in payload_source


def test_hostile_hook_clone_payload_would_fire_if_actually_executed(
    hostile_hook_clone: Callable[..., HostileHookClone],
    tmp_path: Path,
) -> None:
    """Self-check on the fixture's own mechanics, independent of grim: prove
    the planted payload is a real, executable sentinel-toucher — not a
    no-op that would make a future ``not sentinel.exists()`` assertion
    trivially true regardless of what grim does."""
    workspace = tmp_path / "hostile-clone-direct"
    clone = hostile_hook_clone(workspace)

    subprocess.run(["/bin/sh", str(clone.payload)], check=True)

    assert clone.sentinel.exists(), "the planted payload must be a real executable sentinel"


def test_hostile_clone_today_grim_status_never_touches_the_planted_payload(
    hostile_hook_clone: Callable[..., HostileHookClone],
    grim_at: Callable[[Path], GrimRunner],
    tmp_path: Path,
) -> None:
    """Baseline for S-011 ahead of the hooks feature landing: a `grim` build
    with no hook support at all must not go anywhere near a repo-planted
    dispatch table. WP-O extends this once `grim hook run` exists."""
    workspace = tmp_path / "hostile-clone-status"
    clone = hostile_hook_clone(workspace)

    runner = grim_at(clone.workspace)
    runner.run("init", check=False)
    runner.run("status", check=False)

    assert not clone.sentinel.exists()
