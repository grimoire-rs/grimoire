# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A dropped client never takes an adopted MCP entry with it."""
from __future__ import annotations

import json
from pathlib import Path

MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

ADOPTED = "hand-authored"
WRITTEN = "grim-written"
MEMBER = {"command": "grim", "args": ["mcp"]}


def _release_mcp(
    runner, project_dir: Path, registry: str, unique_repo: str, name: str
) -> str:
    src = project_dir / "src"
    src.mkdir(parents=True, exist_ok=True)
    descriptor = src / f"{name}.toml"
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/{name}:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def _write_config(project_dir: Path, refs: dict[str, str], clients: list[str]) -> None:
    clients_toml = ", ".join(f'"{c}"' for c in clients)
    members = "\n".join(f'{name} = "{ref}"' for name, ref in refs.items())
    (project_dir / "grimoire.toml").write_text(
        f"[options]\nclients = [{clients_toml}]\n\n[mcp]\n{members}\n"
    )


def _outputs(project_dir: Path, name: str) -> dict[str, dict]:
    state = json.loads((project_dir / ".grimoire/state.json").read_text())
    record = next(r for r in state["records"] if r["name"] == name)
    return {o["client"]: o for o in record["outputs"]}


def test_dropped_client_leaves_adopted_mcp_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Narrowing ``[options].clients`` leaves a member grim never wrote.

    Claude's ``.mcp.json`` holds one member the user wrote and one grim wrote,
    both intact. Dropping Claude splices out only grim's. The other is named in
    ``abandoned_entries``, because the output leaving the record is the last
    moment anything points at it.
    """
    runner = grim_at(project_dir)
    refs = {
        name: _release_mcp(runner, project_dir, registry, unique_repo, name)
        for name in (ADOPTED, WRITTEN)
    }
    config = project_dir / ".mcp.json"
    config.write_text(json.dumps({"mcpServers": {ADOPTED: MEMBER}}, indent=2))
    _write_config(project_dir, refs, ["claude", "cursor"])
    runner.run("lock")
    runner.run("install")

    assert _outputs(project_dir, ADOPTED)["claude"]["adopted"] is True
    assert "adopted" not in _outputs(project_dir, WRITTEN)["claude"]
    assert json.loads(config.read_text())["mcpServers"] == {
        ADOPTED: MEMBER,
        WRITTEN: MEMBER,
    }

    _write_config(project_dir, refs, ["cursor"])
    rows = {r["name"]: r for r in runner.json("update")["items"]}

    assert json.loads(config.read_text())["mcpServers"] == {ADOPTED: MEMBER}, (
        "the reap must splice out grim's own member and leave the adopted one"
    )
    left = rows[ADOPTED]
    assert left["reaped_clients"] == ["claude"], left
    assert left["kept_modified_clients"] == [], left
    assert left["retained"] == [], left
    [entry] = left["abandoned_entries"]
    assert Path(entry["path"]).resolve() == config.resolve(), entry
    assert entry["pointer"] == f"/mcpServers/{ADOPTED}", entry

    removed = rows[WRITTEN]
    assert removed["reaped_clients"] == ["claude"], removed
    assert removed["abandoned_entries"] == [], removed

    for name in refs:
        assert set(_outputs(project_dir, name)) == {"cursor"}, (
            "the dropped client's output leaves the record either way"
        )
