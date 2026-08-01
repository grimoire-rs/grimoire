# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Config-flock coverage for the first-run (absent-config) state.

An absent ``grimoire.toml`` is the universal first-run state, not a reason
to run a mutation unlocked: two concurrent first-run writers used to race
last-writer-wins with both exiting 0, silently dropping one declaration.
``grim init`` had the same shape one layer down — a ``path.exists()``
check-then-act with a non-atomic write.

The contention tests stand in for a competing ``grim`` process by holding
the advisory lock from the test itself: grim locks a ``<file>.lock``
sidecar with ``flock(2)`` (via fs4), the same lock space Python's
``fcntl.flock`` uses, so the assertion is deterministic instead of timed.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tomllib
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

from src.runner import GrimRunner

unix_only = pytest.mark.skipif(
    sys.platform == "win32", reason="flock(2) contention needs a Unix host"
)

SKILL = "---\nname: {name}\ndescription: Demo skill.\n---\n# Body\n"


@contextmanager
def held_flock(sidecar: Path) -> Iterator[None]:
    """Hold grim's advisory lock on ``sidecar`` for the block's duration."""
    import fcntl

    sidecar.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(sidecar, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(fd)
        sidecar.unlink(missing_ok=True)


def _skill(root: Path, name: str) -> Path:
    d = root / "skills" / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "SKILL.md").write_text(SKILL.format(name=name))
    return d


@unix_only
def test_first_run_global_add_refuses_while_the_config_flock_is_held(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    """A global mutation against an absent ``$GRIM_HOME/grimoire.toml``
    must still take the config flock.

    Before the fix the lock was skipped whenever the config file did not
    exist yet, so this add ran unguarded and exited 0 — the window in which
    two first-run writers lose each other's declarations.
    """
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["GRIM_OFFLINE"] = "1"
    config = grim_home / "grimoire.toml"
    skill = _skill(tmp_path, "alpha")

    with held_flock(grim_home / "grimoire.toml.lock"):
        result = runner.run("add", "--global", str(skill), "--no-install", check=False)

    assert result.returncode == 75, (
        "a first-run global add must contend for the config flock (EX_TEMPFAIL 75), "
        f"got {result.returncode}; {result.stderr}"
    )
    assert not config.exists(), "the refused add must not have written a declaration"


def test_parallel_first_run_global_adds_never_lose_a_declaration(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    """Two first-run global adds started together: whichever exits 0 must
    be able to find its own declaration afterwards.

    The flock is non-blocking, so a genuine overlap makes the loser exit 75
    (retryable) rather than wait. What must never happen is the pre-fix
    outcome — both exit 0, one declaration silently gone.
    """
    grim_home.mkdir(parents=True, exist_ok=True)
    names = ["alpha", "omega"]
    procs = []
    for name in names:
        runner = GrimRunner(grim_binary, grim_home)
        runner.env["GRIM_OFFLINE"] = "1"
        procs.append(
            subprocess.Popen(
                [
                    str(grim_binary),
                    "add",
                    "--global",
                    str(_skill(tmp_path, name)),
                    "--no-install",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                env=runner.env,
            )
        )
    codes = [p.wait() for p in procs]

    config = grim_home / "grimoire.toml"
    declared = config.read_text() if config.exists() else ""
    assert 0 in codes, f"at least one add must succeed; got {codes}"
    for name, code in zip(names, codes, strict=True):
        if code == 0:
            assert f'{name} = ' in declared, (
                f"'{name}' exited 0 but its declaration is missing — a concurrent "
                f"writer overwrote it:\n{declared}"
            )
        else:
            assert code == 75, (
                f"the losing writer must report the retryable lock refusal, got {code}"
            )


@unix_only
def test_init_refuses_while_the_config_flock_is_held(
    grim_at, project_dir: Path
) -> None:
    """``grim init`` takes the config flock before its exists-check.

    Before the fix init was the one config writer that never locked, so two
    racing inits both passed ``exists()`` and the last write won silently.
    """
    runner = grim_at(project_dir)
    with held_flock(project_dir / "grimoire.toml.lock"):
        result = runner.plain("init", check=False)

    assert result.returncode == 75, (
        f"init must contend for the config flock (EX_TEMPFAIL 75), "
        f"got {result.returncode}; {result.stderr}"
    )
    assert not (project_dir / "grimoire.toml").exists(), (
        "a refused init must not write a config"
    )


def test_second_init_refuses_and_leaves_a_parseable_config(
    grim_at, project_dir: Path
) -> None:
    """Re-init still exits 64, and the config the first init wrote is
    complete TOML — the write goes through the atomic seam, so no partial
    file can be left behind for the exists-guard to protect forever."""
    runner = grim_at(project_dir)
    assert runner.plain("init", check=False).returncode == 0

    second = runner.plain("init", check=False)
    assert second.returncode == 64, f"re-init must be EX_USAGE 64; {second.stderr}"

    body = (project_dir / "grimoire.toml").read_bytes()
    parsed = tomllib.loads(body.decode())
    assert "skills" in parsed and "rules" in parsed, parsed
