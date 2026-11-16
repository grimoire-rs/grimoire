# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A project vendor config reached through a link proves no MCP member.

Ownership is never proven by a link (``adr_output_ownership_without_state.md``),
so at project scope ``grim status`` and ``grim uninstall`` record no MCP member
read through a linked vendor config, and every command leaves the file the link
reaches byte for byte. Install and uninstall still splice through such a link by
design; only the proof refuses it. ``grim update``'s own proof records nothing,
but its install gate then adopts the identical member as ``grim install`` does.

World: one MCP server ``srv`` installed for one client, then its vendor config
(or the directory above it) moved out of the workspace and linked back, with
no ``.grimoire/state.json`` left.
"""

from __future__ import annotations

import json
import os
import shutil
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.registry import REGISTRY_HOST
from src.runner import GrimRunner

posix_only = pytest.mark.skipif(
    os.name == "nt", reason="creating a symlink on Windows needs a privilege CI lacks"
)

MCP_DESCRIPTOR = """\
description = "Proof safety test server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

# client -> (its project MCP config, the path linked out of the workspace)
LAYOUTS = {
    "claude": (".mcp.json", ".mcp.json"),
    "cursor": (".cursor/mcp.json", ".cursor"),
}


@dataclass
class Linked:
    runner: GrimRunner
    work: Path
    outside: Path
    before: bytes

    def recorded(self) -> set[str]:
        state = self.work / ".grimoire" / "state.json"
        if not state.exists():
            return set()
        doc = json.loads(state.read_text(encoding="utf-8"))
        return {r["name"] for r in doc["records"]}


def _linked(grim_at, project_dir: Path, repo: str, client: str) -> Linked:
    """``srv`` installed for ``client``, its config linked out, its record gone."""
    descriptor = project_dir.parent / "src" / "srv.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(MCP_DESCRIPTOR, encoding="utf-8", newline="\n")
    runner = grim_at(project_dir)
    runner.run(
        "release", str(descriptor), f"{REGISTRY_HOST}/{repo}/srv:1.0.0", "--kind", "mcp"
    )
    (project_dir / "grimoire.toml").write_text(
        f"[options]\nclients = {json.dumps([client])}\n\n"
        f'[mcp]\nsrv = "{REGISTRY_HOST}/{repo}/srv:1"\n',
        encoding="utf-8",
        newline="\n",
    )
    runner.run("lock")
    runner.run("install")
    config, linked = LAYOUTS[client]
    assert "srv" in (project_dir / config).read_text(encoding="utf-8")
    away = project_dir.parent / "outside"
    away.mkdir()
    moved = away / Path(linked).name
    shutil.move(project_dir / linked, moved)
    os.symlink(moved, project_dir / linked, target_is_directory=moved.is_dir())
    shutil.rmtree(project_dir / ".grimoire")
    outside = moved if linked == config else moved / "mcp.json"
    return Linked(runner, project_dir, outside, outside.read_bytes())


@posix_only
@pytest.mark.parametrize("client", sorted(LAYOUTS))
def test_status_records_no_member_through_a_linked_vendor_config(
    grim_at, project_dir, unique_repo, client
):
    t = _linked(grim_at, project_dir, unique_repo, client)
    result = t.runner.run("status", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert "srv" not in t.recorded()
    assert t.outside.read_bytes() == t.before


@posix_only
@pytest.mark.parametrize("client", sorted(LAYOUTS))
def test_update_leaves_a_member_behind_a_linked_vendor_config_byte_for_byte(
    grim_at, project_dir, unique_repo, client
):
    t = _linked(grim_at, project_dir, unique_repo, client)
    result = t.runner.run("update", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert t.outside.read_bytes() == t.before
    # The record comes from the install gate's adoption of identical bytes, never the proof.
    state = json.loads(
        (t.work / ".grimoire" / "state.json").read_text(encoding="utf-8")
    )
    (srv,) = [r for r in state["records"] if r["name"] == "srv"]
    assert [o["adopted"] for o in srv["outputs"]] == [True]


@posix_only
@pytest.mark.parametrize("force", [(), ("--force",)], ids=["plain", "force"])
@pytest.mark.parametrize("client", sorted(LAYOUTS))
def test_uninstall_leaves_a_member_behind_a_linked_vendor_config_byte_for_byte(
    grim_at, project_dir, unique_repo, client, force
):
    # A recorded member uninstalls under --force: with no record, nothing is removed.
    t = _linked(grim_at, project_dir, unique_repo, client)
    result = t.runner.run("uninstall", *force, "mcp", "srv", format="json", check=False)
    assert result.returncode == 0, result.stderr
    assert t.outside.read_bytes() == t.before
