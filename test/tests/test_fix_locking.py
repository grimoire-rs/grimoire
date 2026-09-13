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


# ---------------------------------------------------------------------------
# Symlinked config (issue #117)
# ---------------------------------------------------------------------------


def _stow(grim_home: Path, dotfiles: Path, name: str, body: str | None = None) -> Path:
    """Put ``<name>`` under ``dotfiles`` and symlink it into ``grim_home``,
    the way GNU stow / chezmoi / yadm manage a dotfile. Returns the link.

    ``body=None`` moves the existing ``grim_home/<name>`` behind the link
    instead of writing a fresh file.
    """
    real = dotfiles / name
    link = grim_home / name
    dotfiles.mkdir(parents=True, exist_ok=True)
    grim_home.mkdir(parents=True, exist_ok=True)
    if body is None:
        link.rename(real)
    else:
        real.write_text(body)
    os.symlink(os.path.relpath(real, grim_home), link)
    assert link.is_symlink()
    return link


@unix_only
def test_symlinked_global_config_is_written_through_and_the_link_survives(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    """The issue #117 shape: ``$GRIM_HOME/grimoire.toml`` (and later the
    lock) are symlinks into a dotfiles repository.

    Before the fix every config-locking command failed with
    ``I/O error: guarded path is a symlink`` (74). Lifting only that guard
    would have been worse: the atomic tmp+rename replaced the *link* with a
    regular file, silently detaching the dotfiles copy. Both files must
    stay links and the managed copies must carry the new content.
    """
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["GRIM_OFFLINE"] = "1"
    dotfiles = tmp_path / "dotfiles"
    toml_link = _stow(grim_home, dotfiles, "grimoire.toml", "[skills]\n")

    result = runner.run("add", "--global", str(_skill(tmp_path, "alpha")), "--no-install", check=False)
    assert result.returncode == 0, f"add through a symlinked config must succeed: {result.stderr}"
    assert toml_link.is_symlink(), "the add must write through the link, not replace it"
    assert "alpha = " in (dotfiles / "grimoire.toml").read_text(), "the dotfiles copy must carry the declaration"

    result = runner.run("install", "--global", check=False)
    assert result.returncode == 0, f"install --global through a symlinked config must succeed: {result.stderr}"

    # Now the lock is stowed too — the next relock must write through it.
    lock_link = _stow(grim_home, dotfiles, "grimoire.lock")
    result = runner.run("add", "--global", str(_skill(tmp_path, "omega")), "--no-install", check=False)
    assert result.returncode == 0, f"add through a symlinked lock must succeed: {result.stderr}"
    assert toml_link.is_symlink() and lock_link.is_symlink(), "both links must survive the relock"
    assert "omega = " in (dotfiles / "grimoire.toml").read_text()
    assert "omega" in (dotfiles / "grimoire.lock").read_text(), "the dotfiles lock copy must carry the new pin"
    assert not (grim_home / "grimoire.toml.lock").exists(), "no sidecar may linger beside the link"


@unix_only
def test_symlinked_config_contends_on_the_real_files_flock(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    """The flock keys on the file the link resolves to, not on the link.

    A sidecar beside the link would let a writer reaching the same real
    file by another path (``--config ~/dotfiles/grimoire.toml``, a second
    link) run concurrently — exactly the lost update the lock exists to
    prevent. Holding the real file's sidecar must therefore refuse a
    mutation issued through the link.
    """
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["GRIM_OFFLINE"] = "1"
    dotfiles = tmp_path / "dotfiles"
    _stow(grim_home, dotfiles, "grimoire.toml", "[skills]\n")

    with held_flock(dotfiles / "grimoire.toml.lock"):
        result = runner.run("add", "--global", str(_skill(tmp_path, "alpha")), "--no-install", check=False)

    assert result.returncode == 75, (
        f"a mutation through the link must contend on the real file's flock (75), got {result.returncode}; {result.stderr}"
    )
    assert not (grim_home / "grimoire.toml.lock").exists(), "the sidecar must not be placed beside the link"
    assert "alpha = " not in (dotfiles / "grimoire.toml").read_text()
