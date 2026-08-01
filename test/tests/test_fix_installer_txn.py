# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Install-transaction regression tests.

An install pass touches several clients per artifact and several artifacts
per run. Each of these covers a way a partial pass used to leave the
workspace in a state no later command could repair without ``--force``:

- a client whose materialization fails must not discard the work of the
  clients that already succeeded (the record has to describe what is on
  disk, or the next run refuses on drift the user never caused);
- an MCP registration refused for one client must not have been spliced
  into an earlier client's config already;
- a destination whose bytes drifted from the record is refused, never
  silently deleted — and ``--force`` always clears the refusal.
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

SKILL_V1 = "---\nname: code-review\n---\n# CR v1\n"
SKILL_V2 = "---\nname: code-review\n---\n# CR v2\n"

MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

# Permission-based failure injection cannot work for a superuser, and
# `chmod` is not a write barrier on Windows.
needs_posix_permissions = pytest.mark.skipif(
    sys.platform == "win32" or (hasattr(os, "geteuid") and os.geteuid() == 0),
    reason="needs POSIX permissions enforced against a non-root user",
)


def _publish_two_versions(unique_repo: str):
    repo = f"{unique_repo}/code-review"
    v1 = make_artifact(
        repo, "skill", {"code-review/SKILL.md": SKILL_V1}, tag="1.0.0"
    )
    v2 = make_artifact(
        repo, "skill", {"code-review/SKILL.md": SKILL_V2}, tag="2.0.0"
    )
    return v1, v2


def _release_mcp(runner, project_dir: Path, registry: str, unique_repo: str) -> str:
    src = project_dir / "src"
    src.mkdir(parents=True, exist_ok=True)
    descriptor = src / "grim-mcp.toml"
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


@needs_posix_permissions
def test_failed_client_keeps_sibling_client_recoverable(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A hard error on the second client must not wedge the first.

    Claude and Cursor both hold the artifact at v1. The pin rolls to v2
    with Cursor's skills directory frozen: Claude is re-materialized, then
    Cursor fails. The run must still exit non-zero, but the recorded state
    must describe what is now on disk, so that re-running the install once
    the directory is writable again converges without ``--force``.
    """
    v1, v2 = _publish_two_versions(unique_repo)
    (project_dir / ".cursor").mkdir()
    write_config(project_dir, skills={"code-review": v1.fq})

    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    claude_index = project_dir / ".claude/skills/code-review/SKILL.md"
    cursor_index = project_dir / ".cursor/skills/code-review/SKILL.md"
    assert claude_index.read_text() == SKILL_V1
    assert cursor_index.read_text() == SKILL_V1

    write_config(project_dir, skills={"code-review": v2.fq})
    runner.run("lock")
    cursor_skills = project_dir / ".cursor/skills"
    cursor_skills.chmod(0o500)
    try:
        failed = runner.run("install", check=False)
    finally:
        cursor_skills.chmod(0o700)

    assert failed.returncode != 0, (
        "a client that cannot be written must still fail the command"
    )
    assert claude_index.read_text() == SKILL_V2, (
        "the first client was materialized before the failure; this test "
        "only means something if that actually happened"
    )

    retry = runner.run("install", check=False)
    assert retry.returncode == 0, (
        "the retry must converge on its own — a partial pass that records "
        "nothing leaves Claude looking locally modified, which refuses "
        f"every later install (rc={retry.returncode}): {retry.stderr}"
    )
    assert claude_index.read_text() == SKILL_V2
    assert cursor_index.read_text() == SKILL_V2


def test_mcp_refusal_leaves_every_client_config_untouched(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """One client's refusal aborts the whole MCP registration.

    Cursor carries a hand-authored ``grim-mcp`` member, so the install is
    refused. Claude is processed first — its config must come out of the
    run with no grim-authored entry, or the user is left running an MCP
    server that no record covers and no uninstall can remove.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)

    cursor_config = project_dir / ".cursor/mcp.json"
    cursor_config.parent.mkdir(parents=True)
    cursor_config.write_text(
        '{\n  "mcpServers": {\n    "grim-mcp": {"command": "user-owned"}\n  }\n}\n'
    )
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    result = runner.run("install", check=False)
    assert result.returncode == 65, (
        f"untracked MCP member clobber must exit 65, got "
        f"{result.returncode}; {result.stderr}"
    )
    assert (
        json.loads(cursor_config.read_text())["mcpServers"]["grim-mcp"]["command"]
        == "user-owned"
    ), "the refusal must leave the hand-authored member untouched"

    claude_config = project_dir / ".mcp.json"
    registered = (
        json.loads(claude_config.read_text()).get("mcpServers", {})
        if claude_config.exists()
        else {}
    )
    assert "grim-mcp" not in registered, (
        "a refused registration must write nothing anywhere: Claude's "
        "config holds an entry no install record covers, so uninstall "
        "reports not-installed and the server runs unmanaged"
    )


def test_env_aliased_root_refuses_instead_of_deleting(
    grim: "object", grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A hand-authored file at an aliased root is refused, never deleted.

    The record is written with ``CLAUDE_CONFIG_DIR`` pointing at an
    alternate root; the next run has the variable unset, so ``claude-root``
    resolves to ``~/.claude`` and a hand-authored skill there classifies to
    the very ``(anchor, relative)`` pair the record already holds. Grim
    must not treat that pair match as proof it wrote the bytes.
    """
    make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": SKILL_V1},
        tag="1.0.0",
    )
    alt_root = grim.home / "alt-claude"
    alt_root.mkdir(parents=True)
    grim.env["CLAUDE_CONFIG_DIR"] = str(alt_root)
    ref = f"{registry}/{unique_repo}/code-review:1.0.0"
    grim.run("add", "--global", ref)
    assert (alt_root / "skills/code-review/SKILL.md").read_text() == SKILL_V1

    del grim.env["CLAUDE_CONFIG_DIR"]
    hand = grim.home / ".claude/skills/code-review/SKILL.md"
    hand.parent.mkdir(parents=True)
    hand.write_text("# hand-authored, not grim's\n")

    result = grim.run("install", "--global", check=False)
    assert result.returncode != 0, (
        "grim wrote nothing at the un-aliased root, so overwriting it "
        "needs --force"
    )
    assert hand.read_text() == "# hand-authored, not grim's\n", (
        "a destination whose bytes are not the recorded ones must never "
        "be removed without --force"
    )


def test_partial_destination_file_is_never_a_permanent_wedge(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A truncated destination refuses, and ``--force`` always clears it.

    This is the shape an interrupted write leaves behind. The refusal is
    correct — grim cannot tell a torn file from an edit — but it must stay
    recoverable rather than becoming a state no command can leave.
    """
    v1, _ = _publish_two_versions(unique_repo)
    write_config(project_dir, skills={"code-review": v1.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")

    index = project_dir / ".claude/skills/code-review/SKILL.md"
    index.write_text(SKILL_V1[: len(SKILL_V1) // 2])

    refused = runner.run("install", check=False)
    assert refused.returncode == 65, (
        f"a drifted destination must refuse with 65, got "
        f"{refused.returncode}: {refused.stderr}"
    )

    forced = runner.run("install", "--force", check=False)
    assert forced.returncode == 0, (
        f"--force must always clear the refusal; got "
        f"{forced.returncode}: {forced.stderr}"
    )
    assert index.read_text() == SKILL_V1
    row = next(
        r for r in runner.json("status")["items"] if r["name"] == "code-review"
    )
    assert row["state"] == "installed", row
