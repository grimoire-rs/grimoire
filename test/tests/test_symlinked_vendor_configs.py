# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Vendor config files grim splices in place survive as symlinks (issue #117).

A dotfiles manager (GNU stow, chezmoi, yadm) links ``~/.claude/settings.json``,
``opencode.json``, ``~/.codex/config.toml``, ``~/.claude.json`` … into place.
grim edits those files with a span-preserving splice, but the final
tmp+rename replaced the *link* with a regular file, silently detaching the
dotfiles copy. Every splice must write through the link, and a recorded MCP
entry behind a link that resolves outside the vendor root must still be
read, hashed, and removed — it is spliced, never deleted, so the
containment guard's delete hazard does not apply.
"""
from __future__ import annotations

import json
import os
import sys
import tomllib
from pathlib import Path

import pytest

from src.runner import GrimRunner

unix_only = pytest.mark.skipif(sys.platform == "win32", reason="symlinks are a Unix layout")

DESCRIPTOR = """\
description = "probe"
summary = "probe"
keywords = "probe"

[server]
transport = "stdio"
command = "true"
"""


def _stow(link: Path, real: Path, body: str) -> None:
    """Write ``body`` to ``real`` and symlink it into place at ``link``."""
    real.parent.mkdir(parents=True, exist_ok=True)
    real.write_text(body)
    link.parent.mkdir(parents=True, exist_ok=True)
    os.symlink(os.path.relpath(real, link.parent), link)


def _rule_with_support_dir(root: Path) -> Path:
    index = root / "rules" / "probe.md"
    index.parent.mkdir(parents=True, exist_ok=True)
    index.write_text("---\npaths: ['**']\n---\n# probe\nsee [x](probe/x.md)\n")
    (index.parent / "probe").mkdir()
    (index.parent / "probe" / "x.md").write_text("# x\n")
    return index


@unix_only
def test_managed_config_splices_write_through_symlinks(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    """Claude's ``settings.json`` (``claudeMdExcludes``) and OpenCode's
    ``opencode.json`` (``instructions``) stay links across install and
    uninstall, and the dotfiles copies carry every edit."""
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["GRIM_OFFLINE"] = "1"
    xdg = runner.home / ".config"
    runner.env["XDG_CONFIG_HOME"] = str(xdg)
    (xdg / "opencode" / "skills").mkdir(parents=True)
    dotfiles = tmp_path / "dotfiles"
    claude_link, claude_real = runner.home / ".claude" / "settings.json", dotfiles / "claude-settings.json"
    opencode_link, opencode_real = xdg / "opencode" / "opencode.json", dotfiles / "opencode.json"
    _stow(claude_link, claude_real, '{"userSetting": 1}\n')
    _stow(opencode_link, opencode_real, '{"theme": "dark"}\n')

    result = runner.run("add", "--global", "--kind", "rule", str(_rule_with_support_dir(tmp_path)), check=False)
    assert result.returncode == 0, result.stderr
    assert claude_link.is_symlink() and opencode_link.is_symlink(), "the splice must not replace the links"
    claude = json.loads(claude_real.read_text())
    assert claude["userSetting"] == 1 and any("rules/probe/**" in g for g in claude["claudeMdExcludes"])
    opencode = json.loads(opencode_real.read_text())
    assert opencode["theme"] == "dark" and opencode["instructions"]

    result = runner.run("uninstall", "--global", "rule", "probe", check=False)
    assert result.returncode == 0, result.stderr
    assert claude_link.is_symlink() and opencode_link.is_symlink(), "uninstall must not replace the links either"
    assert json.loads(claude_real.read_text()) == {"userSetting": 1}
    assert json.loads(opencode_real.read_text()) == {"theme": "dark"}


@unix_only
def test_mcp_entries_write_through_symlinks_outside_the_vendor_root(
    grim_binary: Path, grim_home: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """``~/.codex/config.toml`` linked into a dotfiles repository resolves
    outside the ``codex-root`` anchor. The entry is still installed through
    the link, reported installed, and spliced out again — never a bare
    "resolves outside its anchor root" warning, never an abandoned entry."""
    runner = GrimRunner(grim_binary, grim_home)
    dotfiles = tmp_path / "dotfiles"
    codex_link, codex_real = runner.home / ".codex" / "config.toml", dotfiles / "codex" / "config.toml"
    claude_link, claude_real = runner.home / ".claude.json", dotfiles / "claude.json"
    _stow(codex_link, codex_real, 'model = "x"\n')
    _stow(claude_link, claude_real, '{"userSetting": 1}\n')
    (runner.home / ".claude").mkdir()

    descriptor = tmp_path / "mcp" / "probe.toml"
    descriptor.parent.mkdir()
    descriptor.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/probe:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    result = runner.run("add", "--global", ref, "--no-install", check=False)
    assert result.returncode == 0, result.stderr
    result = runner.run("install", "--global", "--client", "codex", "--client", "claude", check=False)
    assert result.returncode == 0, result.stderr
    assert codex_link.is_symlink() and claude_link.is_symlink(), "the MCP splice must not replace the links"
    codex = tomllib.loads(codex_real.read_text())
    assert codex["model"] == "x" and codex["mcp_servers"]["probe"]["command"] == "true"
    claude = json.loads(claude_real.read_text())
    assert claude["userSetting"] == 1 and claude["mcpServers"]["probe"]["command"] == "true"

    result = runner.run("--format", "json", "status", "--global", check=False)
    assert result.returncode == 0, result.stderr
    assert "outside its anchor root" not in result.stderr
    rows = json.loads(result.stdout)["items"]
    assert [r["state"] for r in rows] == ["installed"], rows

    result = runner.run("uninstall", "--global", "mcp", "probe", check=False)
    assert result.returncode == 0, result.stderr
    assert "outside its anchor root" not in result.stderr and "abandon" not in result.stderr
    assert codex_link.is_symlink() and claude_link.is_symlink(), "uninstall must not replace the links either"
    assert tomllib.loads(codex_real.read_text()) == {"model": "x"}
    assert json.loads(claude_real.read_text()) == {"userSetting": 1}
