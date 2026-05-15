# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`--global` scope acceptance tests.

The global scope operates on ``$GRIM_HOME/grimoire.toml`` and its own
lock, fully independent of any project config (the two are never merged).
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact
from src.runner import GrimRunner


def test_global_scope_is_independent_of_project(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/global-rule"
    ru = make_artifact(
        repo,
        "rule",
        {"global-rule.md": "---\npaths: ['**']\n---\n# global\n"},
        tag="v1",
    )
    # Global config under $GRIM_HOME, no project config anywhere.
    (grim_home / "grimoire.toml").write_text(
        f'[rules]\nglobal-rule = "{ru.fq}"\n'
    )
    runner = GrimRunner(grim_binary, grim_home)

    lock_rows = runner.json("lock", "--global")
    assert lock_rows[0]["name"] == "global-rule"
    assert (grim_home / "grimoire.lock").is_file()
    assert "@sha256:" in (grim_home / "grimoire.lock").read_text()

    install_rows = runner.json("install", "--global")
    assert install_rows[0]["status"] == "installed"
    # Global artifacts materialize under $GRIM_HOME/.claude.
    assert (grim_home / ".claude/rules/global-rule.md").is_file()

    status_rows = runner.json("status", "--global")
    assert status_rows[0]["state"] == "installed"


def test_global_install_without_lock_exits_79(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/r"
    ru = make_artifact(repo, "rule", {"r.md": "# r\n"}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[rules]\nr = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)

    result = runner.run("install", "--global", check=False)
    assert result.returncode == 79, (
        f"global install without a lock must exit 79, got "
        f"{result.returncode}; {result.stderr}"
    )
