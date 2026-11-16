# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The unproven-files line for an MCP server a clone holds with no record.

An MCP server lives as one entry in a config file other servers share, so the
line names that entry in the file and advises removing the entry by hand,
never deleting the file. Fixture style of ``test_record_cache_uninstall_remove.py``.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path

from src.registry import REGISTRY_HOST
from src.scenarios import Team

MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


def _go(t: Team, *args: str):
    """Run Bob's ``grim --format json`` and return ``(exit, parsed stdout, stderr)``."""
    result = t.bob.runner.run(*args, format="json", check=False)
    try:
        doc = json.loads(result.stdout)
    except json.JSONDecodeError:
        doc = {}
    return result.returncode, doc, result.stderr


def _clone(team: Callable[[str], Team], descriptor: Path, ref: str) -> Team:
    """Bob holds Alice's `.mcp.json` and lock, no install state, an empty store."""
    t = team("CO")
    t.alice.runner.json("release", str(descriptor), ref, "--kind", "mcp")
    text = f'[options]\nclients = ["claude"]\n\n[mcp]\nsrv = "{ref}"\n'
    (t.alice.work / "grimoire.toml").write_text(text, encoding="utf-8", newline="\n")
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: declare")
    t.alice.push()
    t.bob.pull()
    assert not (t.bob.work / ".grimoire").exists()
    return t


def test_a_cold_remove_advises_removing_the_entry_not_the_shared_file(
    team, unique_repo, tmp_path
):
    descriptor = tmp_path / "srv.toml"
    descriptor.write_text(MCP_DESCRIPTOR, encoding="utf-8", newline="\n")
    ref = f"{REGISTRY_HOST}/{unique_repo}/mcp/srv:1.0.0"
    t = _clone(team, descriptor, ref)
    config = t.bob.work / ".mcp.json"
    doc = json.loads(config.read_text(encoding="utf-8"))
    assert "srv" in doc["mcpServers"], "the fixture: srv is installed"
    # A second server grim never declared shares the file.
    doc["mcpServers"]["other"] = {"command": "keep-me", "args": ["--still"]}
    config.write_text(json.dumps(doc, indent=2), encoding="utf-8", newline="\n")
    before = config.read_bytes()

    rc, report, stderr = _go(t, "--offline", "remove", "mcp", "srv")

    assert rc == 0, stderr
    assert report["status"] == "removed"
    assert config.read_bytes() == before
    lines = [line for line in stderr.splitlines() if "mcp 'srv'" in line]
    (line,) = lines
    assert ".mcp.json" in line
    assert "left its entry in" in line
    assert "'srv' entry" in line
    assert "by hand" in line
    assert "delete" not in line
    assert "on disk" not in line
    servers = json.loads(config.read_text(encoding="utf-8"))["mcpServers"]
    assert servers["other"] == {"command": "keep-me", "args": ["--still"]}
