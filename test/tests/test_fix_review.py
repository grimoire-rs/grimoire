# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Regressions found by the adversarial review of the state-drift branch."""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact

BAD_SKILL = (
    "---\n"
    "name: code-review\n"
    "description: Test skill.\n"
    "metadata:\n"
    '  claude.effort: "not-a-valid-effort"\n'
    "---\n"
    "# CR\n"
)


def test_fresh_partial_install_does_not_report_itself_installed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A first install that fails on its last client must stay recoverable.

    OpenCode renders the skill fine (it drops the foreign ``claude.*``
    key); Claude tries to lift it, hits an unconvertible literal, and
    fails. With no prior record to fall back on, recording a half-done
    destination at the NEW pin would make the retry's integrity gate see
    an intact, fully-covered record at the locked pin and answer
    `unchanged` — leaving the wrong bytes in place forever, at exit 0.

    Since forced and first installs stage into a sibling temp dir and
    publish only on success, the failing client now leaves *nothing* at
    its destination rather than the half-written tree ``copy_tree`` used
    to leave there. That is the stronger property, so it is what this
    asserts: the hazard above is structurally unreachable, not merely
    unreported. The recoverability assertion at the end is unchanged.
    """
    repo = f"{unique_repo}/code-review"
    make_artifact(repo, "skill", {"code-review/SKILL.md": BAD_SKILL}, tag="1.0.0")
    (project_dir / ".opencode").mkdir(exist_ok=True)
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["opencode", "claude"]\n\n'
        "[skills]\n"
        f'code-review = "{registry}/{repo}:1.0.0"\n'
    )
    runner = grim_at(project_dir)
    runner.run("lock")

    first = runner.run("install", check=False)
    assert first.returncode != 0, "the failing client must fail the command"
    claude_index = project_dir / ".claude/skills/code-review/SKILL.md"
    assert not claude_index.exists(), (
        "a failed materialize must leave nothing at the destination: the "
        "footprint is staged in a sibling temp dir and published only on "
        f"success, yet {claude_index} exists"
    )

    retry = runner.run("install", check=False)
    rows = runner.json("status")["items"]
    row = next(r for r in rows if r["name"] == "code-review")
    assert not (retry.returncode == 0 and row["state"] == "installed"), (
        "the retry claimed a clean install over a destination grim never "
        f"finished writing: rc={retry.returncode} state={row['state']}"
    )


MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

ADOPTED_CONFIG = (
    '{\n  "mcpServers": {\n'
    '    "grim-mcp": {"command": "grim", "args": ["mcp"]}\n'
    "  }\n}\n"
)


def _install_adopted_mcp(runner, project_dir: Path, registry: str, unique_repo: str) -> Path:
    """Install an MCP entry that was already in the config, byte-identical."""
    src = project_dir / "src"
    src.mkdir(parents=True, exist_ok=True)
    descriptor = src / "grim-mcp.toml"
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    config = project_dir / ".mcp.json"
    config.write_text(ADOPTED_CONFIG)
    (project_dir / "grimoire.toml").write_text("[skills]\n[rules]\n[agents]\n")
    runner.json("add", "--no-install", ref)
    assert {r["status"] for r in runner.json("install")["items"]} == {"unchanged"}
    return config


def test_force_uninstall_can_remove_an_adopted_mcp_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Leaving an adopted entry alone must not make it unremovable.

    Without `--force` grim keeps it (it never wrote it). With `--force`
    the user has said explicitly that they want it gone, and there is no
    other command that can reach it once the record is dropped.
    """
    runner = grim_at(project_dir)
    config = _install_adopted_mcp(runner, project_dir, registry, unique_repo)

    runner.run("uninstall", "--force", "mcp", "grim-mcp")
    assert "grim-mcp" not in json.loads(config.read_text()).get("mcpServers", {}), (
        "--force must be able to remove an adopted entry; nothing else can "
        "reach it after the record is gone"
    )


def test_uninstall_reports_the_adopted_entry_it_left_behind(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A kept adopted entry is reported, not silently dropped.

    The record that named it is gone, so this is the last moment the user
    can be told the member is still live in their config.
    """
    runner = grim_at(project_dir)
    config = _install_adopted_mcp(runner, project_dir, registry, unique_repo)

    report = runner.json("uninstall", "mcp", "grim-mcp")
    assert "grim-mcp" in json.loads(config.read_text())["mcpServers"]
    abandoned = report.get("abandoned_entries") or []
    assert any(e["pointer"].endswith("grim-mcp") for e in abandoned), (
        f"the kept entry must be reported so it is not invisible: {report}"
    )


def test_adopted_flag_survives_a_second_install_pass(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Re-running install must not turn an adopted entry into grim's own.

    The second pass finds the member already tracked, so the fresh-adoption
    branch never runs and the rebuilt record was written with adopted:false.
    Uninstall then spliced out a member grim had never authored.
    """
    runner = grim_at(project_dir)
    config = _install_adopted_mcp(runner, project_dir, registry, unique_repo)

    # Widen the client set so the pass actually rebuilds the record (a
    # no-op re-install short-circuits at the integrity gate and never
    # reaches the write pass).
    (project_dir / ".cursor").mkdir(exist_ok=True)
    cfg = project_dir / "grimoire.toml"
    cfg.write_text('[options]\nclients = ["claude", "cursor"]\n\n' + cfg.read_text())
    runner.json("install")

    state = json.loads((project_dir / ".grimoire/state.json").read_text())
    record = next(r for r in state["records"] if r["name"] == "grim-mcp")
    claude_out = next(o for o in record["outputs"] if o["client"] == "claude")
    assert claude_out.get("adopted") is True, (
        f"the flag must survive a rebuilding pass: {record}"
    )

    runner.run("uninstall", "mcp", "grim-mcp")
    assert "grim-mcp" in json.loads(config.read_text())["mcpServers"], (
        "a re-install must not convert the user's own entry into grim's"
    )
