# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""grim's ignore rule for `.claude/settings.local.json` lands where git reads it.

Git takes `info/exclude` from the common directory, so a rule in a linked
worktree's own directory is inert. One file then serves every checkout: a
linked worktree never drops the line, and a line an older grim left in the
per-worktree file stays inert while the next install writes the real one.
"""
from __future__ import annotations

import os
import shutil
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest

from src.helpers import PublishedArtifact, make_hook, write_config
from src.runner import GrimRunner

pytestmark = [
    pytest.mark.skipif(shutil.which("git") is None, reason="git not on PATH"),
    pytest.mark.skipif(os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"),
]

SETTINGS = ".claude/settings.local.json"
MARKER = "hook-dispatcher"

Git = Callable[..., subprocess.CompletedProcess[str]]


@pytest.fixture()
def git(tmp_path: Path) -> Git:
    """Git blind to the host's own ignore files.

    A developer's global ignore file commonly names this very path, and it
    would answer every `check-ignore` below, so a dead rule would read as
    honoured.
    """
    home = tmp_path / "git-home"
    home.mkdir()
    env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1"}

    def run(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args], cwd=cwd, env=env, check=check, capture_output=True, text=True, encoding="utf-8"
        )

    return run


@pytest.fixture()
def checkouts(tmp_path: Path, git: Git) -> tuple[Path, Path]:
    """One repository: its primary checkout and a linked worktree, both Claude projects."""
    primary = tmp_path / "primary"
    primary.mkdir()
    git(primary, "init", "-q", "-b", "main")
    git(primary, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "seed")
    linked = tmp_path / "linked"
    git(primary, "worktree", "add", "-q", str(linked), "-b", "topic")
    (primary / ".claude").mkdir()
    (linked / ".claude").mkdir()
    return primary, linked


@pytest.fixture()
def hook(unique_repo: str) -> PublishedArtifact:
    return make_hook(f"{unique_repo}/guard", "guard")


def _arm(grim_at: Callable[[Path], GrimRunner], checkout: Path, hook: PublishedArtifact) -> GrimRunner:
    write_config(checkout, hooks={"guard": hook.fq})
    runner = grim_at(checkout)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    assert _armed(checkout), "precondition: the hook armed, so the file the rule covers exists"
    return runner


def _armed(checkout: Path) -> bool:
    return MARKER in (checkout / SETTINGS).read_text(encoding="utf-8")


def _rule_source(git: Git, checkout: Path) -> Path | None:
    """The exclude file git took the rule from; `None` when git does not ignore the file."""
    found = git(checkout, "check-ignore", "-v", SETTINGS, check=False)
    assert found.returncode in (0, 1), found.stderr
    if found.returncode == 1:
        return None
    source = found.stdout.split("\t")[0].rsplit(":", 2)[0]
    return (checkout / source).resolve()


def _common_exclude(primary: Path) -> Path:
    return (primary / ".git" / "info" / "exclude").resolve()


def test_linked_worktree_exclude_is_honoured(
    checkouts: tuple[Path, Path], git: Git, grim_at, hook: PublishedArtifact
) -> None:
    primary, linked = checkouts
    _arm(grim_at, linked, hook)

    assert _rule_source(git, linked) == _common_exclude(primary)
    status = git(linked, "status", "--porcelain", "--untracked-files=all").stdout
    assert "grimoire.toml" in status, "POSITIVE CONTROL: status lists an untracked file"
    assert SETTINGS not in status


def test_primary_checkout_exclude_is_honoured(
    checkouts: tuple[Path, Path], git: Git, grim_at, hook: PublishedArtifact
) -> None:
    primary, _ = checkouts
    _arm(grim_at, primary, hook)

    assert _rule_source(git, primary) == _common_exclude(primary)


def test_primary_checkout_drops_the_rule_with_its_last_hook(
    checkouts: tuple[Path, Path], git: Git, grim_at, hook: PublishedArtifact
) -> None:
    primary, _ = checkouts
    runner = _arm(grim_at, primary, hook)
    assert _rule_source(git, primary) == _common_exclude(primary), "precondition: the rule is live"

    runner.run("remove", "hook", "guard")
    runner.run("install")

    assert _rule_source(git, primary) is None


def test_a_linked_worktree_never_drops_the_shared_rule(
    checkouts: tuple[Path, Path], git: Git, grim_at, hook: PublishedArtifact
) -> None:
    primary, linked = checkouts
    _arm(grim_at, primary, hook)
    runner = _arm(grim_at, linked, hook)
    assert _rule_source(git, primary) == _common_exclude(primary), "precondition: the rule is live"

    runner.run("remove", "hook", "guard")
    runner.run("install")

    assert not _armed(linked), "POSITIVE CONTROL: the worktree did lose its last hook"
    assert _armed(primary)
    assert _rule_source(git, primary) == _common_exclude(primary)


def test_an_old_per_worktree_rule_stays_inert_and_the_next_install_heals(
    checkouts: tuple[Path, Path], git: Git, grim_at, hook: PublishedArtifact
) -> None:
    primary, linked = checkouts
    runner = _arm(grim_at, linked, hook)
    common = _common_exclude(primary)
    common.write_text(common.read_text(encoding="utf-8").replace(f"{SETTINGS}\n", ""), encoding="utf-8")
    old = primary / ".git" / "worktrees" / linked.name / "info" / "exclude"
    old.parent.mkdir(exist_ok=True)
    old.write_text(f"# mine\n{SETTINGS}\n", encoding="utf-8")
    assert _rule_source(git, linked) is None, "precondition: git never reads the per-worktree file"

    runner.run("install")

    assert _rule_source(git, linked) == common
    assert old.read_text(encoding="utf-8") == f"# mine\n{SETTINGS}\n", "the old file is left as it was"
