# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Untracked-clobber guard acceptance tests.

An install destination that exists on disk without a recorded output for
that client was not written by grim — the installer must refuse to
overwrite it (exit 65) unless ``--force`` is given. Exception: when the
on-disk content is identical to what the install would write, the file is
adopted into the record instead (the "state deleted, files intact"
repair case).
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path

from src.assertions import assert_not_exists, assert_path_exists
from src.helpers import make_artifact, write_config

SKILL_MD = "---\nname: code-review\n---\n# CR\n"

MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


def _setup_skill(project_dir: Path, unique_repo: str):
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": SKILL_MD},
        tag="stable",
    )
    write_config(project_dir, skills={"code-review": sk.fq})
    return sk


def test_install_refuses_untracked_skill_dir(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A pre-existing hand-authored skill dir with no install record is
    never overwritten: exit 65, content preserved, hint at --force."""
    _setup_skill(project_dir, unique_repo)
    hand = project_dir / ".claude/skills/code-review/SKILL.md"
    hand.parent.mkdir(parents=True)
    hand.write_text("# hand-authored, not grim's\n")

    runner = grim_at(project_dir)
    runner.run("lock")
    result = runner.run("install", check=False)

    assert result.returncode == 65, (
        f"untracked clobber must exit 65, got {result.returncode}; "
        f"{result.stderr}"
    )
    assert hand.read_text() == "# hand-authored, not grim's\n", (
        "refusal must leave the hand-authored file untouched"
    )
    assert "--force" in result.stderr, (
        f"refusal must hint --force; stderr: {result.stderr}"
    )


def test_force_overwrites_untracked_and_uninstall_cleans_up(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--force overwrites the untracked destination and records it; a
    subsequent uninstall removes exactly the recorded output."""
    _setup_skill(project_dir, unique_repo)
    hand = project_dir / ".claude/skills/code-review/SKILL.md"
    hand.parent.mkdir(parents=True)
    hand.write_text("# hand-authored, not grim's\n")

    runner = grim_at(project_dir)
    runner.run("lock")
    rows = runner.json("install", "--force")["items"]
    assert {r["status"] for r in rows} == {"installed"}
    assert hand.read_text() == SKILL_MD

    runner.run("uninstall", "skill", "code-review")
    assert_not_exists(project_dir / ".claude/skills/code-review")


def test_install_adopts_identical_untracked_footprint(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Repair case: state deleted but rendered files intact — the install
    adopts the on-disk content (identical footprint) instead of refusing,
    rebuilds the record, and reports 'unchanged'."""
    _setup_skill(project_dir, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock")
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}

    # Simulate a lost state file: files intact, record gone. The config
    # and lock live beside grimoire.toml and survive.
    shutil.rmtree(project_dir / ".grimoire")
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"unchanged"}, rows
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")

    # And the rebuilt record supports a clean uninstall.
    runner.run("uninstall", "skill", "code-review")
    assert_not_exists(project_dir / ".claude/skills/code-review")


def _release_mcp(runner, project_dir: Path, registry: str, unique_repo: str) -> str:
    src = project_dir / "src"
    src.mkdir(parents=True, exist_ok=True)
    descriptor = src / "grim-mcp.toml"
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def test_mcp_install_refuses_untracked_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A pre-existing user-authored MCP config member with the same name
    and a different value is never replaced without --force."""
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)
    (project_dir / ".claude").mkdir()
    (project_dir / ".mcp.json").write_text(
        '{\n  "mcpServers": {\n    "grim-mcp": {"command": "user-owned"}\n  }\n}\n'
    )
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    result = runner.run("install", check=False)
    assert result.returncode == 65, (
        f"untracked MCP member clobber must exit 65, got "
        f"{result.returncode}; {result.stderr}"
    )
    claude = json.loads((project_dir / ".mcp.json").read_text())
    assert claude["mcpServers"]["grim-mcp"]["command"] == "user-owned", (
        "refusal must leave the user's member untouched"
    )

    rows = runner.json("install", "--force")["items"]
    assert {r["status"] for r in rows} == {"installed"}
    claude = json.loads((project_dir / ".mcp.json").read_text())
    assert claude["mcpServers"]["grim-mcp"]["command"] == "grim"


def test_mcp_install_adopts_identical_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Repair case for MCP: record gone but the registered member is
    semantically identical to what the install would write — adopt."""
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)
    (project_dir / ".claude").mkdir()
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}

    shutil.rmtree(project_dir / ".grimoire")
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"unchanged"}, rows
