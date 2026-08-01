# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Follow-up regressions on the install/update state machinery."""
from __future__ import annotations

import json
from pathlib import Path

from src.assertions import assert_path_exists
from src.helpers import make_artifact
from src.registry import retag

RULE_V1 = "---\npaths: ['**/*.rs']\n---\n# Rust Style v1\n"
RULE_V2 = "---\npaths: ['**/*.rs']\n---\n# Rust Style v2\n"
EDITED = "locally edited by the user\n"


MCP_DESCRIPTOR = """\
description = "Test MCP server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


def _release_mcp(runner, project_dir: Path, registry: str, unique_repo: str) -> str:
    src = project_dir / "src"
    src.mkdir(parents=True, exist_ok=True)
    descriptor = src / "grim-mcp.toml"
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def _write_rule_config(project_dir: Path, rule_ref: str, clients: list[str]) -> None:
    clients_toml = ", ".join(f'"{c}"' for c in clients)
    (project_dir / "grimoire.toml").write_text(
        f"[options]\nclients = [{clients_toml}]\n\n"
        "[rules]\n"
        f'rust-style = "{rule_ref}"\n'
    )


def _recorded_clients(project_dir: Path, name: str = "rust-style") -> set[str]:
    state = json.loads((project_dir / ".grimoire" / "state.json").read_text())
    for rec in state.get("records", []):
        if rec.get("name") == name:
            return {o["client"] for o in rec.get("outputs", [])}
    return set()


def test_dropped_client_edit_survives_an_update_that_also_rolls_the_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Dropping a client and rolling the pin in one update must still
    preserve the dropped client's hand-edited copy.

    The no-pin-change case is covered in test_update.py. With a pin roll
    the installer used to pull every recorded client back into the
    materialize set — including the one the config just dropped — and
    `update` re-materializes with force, so the edit was overwritten
    before the reaper ever looked at it. The reaper then compared the
    freshly written bytes against the record, found them intact, deleted
    the file, and reported an empty kept_modified_clients: the promise in
    docs/src/commands.md silently void.
    """
    repo = f"{unique_repo}/rust-style"
    make_artifact(repo, "rule", {"rust-style.md": RULE_V1}, tag="stable")
    _write_rule_config(project_dir, f"{registry}/{repo}:stable", ["claude", "copilot"])
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")

    claude = project_dir / ".claude/rules/rust-style.md"
    copilot = project_dir / ".github/instructions/rust-style.instructions.md"
    assert_path_exists(claude)
    assert_path_exists(copilot)
    copilot.write_text(EDITED)

    # Drop copilot AND roll the floating tag in the same update.
    _write_rule_config(project_dir, f"{registry}/{repo}:stable", ["claude"])
    v2 = make_artifact(repo, "rule", {"rust-style.md": RULE_V2}, tag="2.0.0")
    retag(repo, "stable", v2.digest)

    row = next(
        r for r in runner.json("update")["items"] if r["name"] == "rust-style"
    )
    assert copilot.read_text() == EDITED, (
        "the dropped client's edit must survive an update without --force"
    )
    assert row["kept_modified_clients"] == ["copilot"], row
    assert row["reaped_clients"] == [], row
    assert "copilot" in _recorded_clients(project_dir), (
        "a kept-modified output stays in the record so status can still see it"
    )
    assert claude.read_text().endswith("# Rust Style v2\n"), (
        "the still-configured client must have rolled to the new pin"
    )


def test_uninstall_leaves_an_adopted_mcp_entry_in_the_config(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An adopted MCP member is not grim's to remove.

    The install-side untracked gate adopts a pre-existing member that is
    semantically identical to what grim would write — recording it without
    touching a byte. Uninstall is the inverse of install, so the inverse of
    writing nothing is removing nothing: the member stays in the user's
    config and only the record entry goes.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)

    config = project_dir / ".mcp.json"
    config.write_text(
        '{\n  "mcpServers": {\n'
        '    "grim-mcp": {"command": "grim", "args": ["mcp"]}\n'
        "  }\n}\n"
    )
    (project_dir / "grimoire.toml").write_text("[skills]\n[rules]\n[agents]\n")
    runner.json("add", "--no-install", ref)

    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"unchanged"}, (
        f"an identical pre-existing member must be adopted, not written: {rows}"
    )
    state = json.loads((project_dir / ".grimoire/state.json").read_text())
    record = next(r for r in state["records"] if r["name"] == "grim-mcp")
    assert record["outputs"][0]["adopted"] is True, record

    runner.run("uninstall", "mcp", "grim-mcp")
    assert "grim-mcp" in json.loads(config.read_text())["mcpServers"], (
        "grim wrote nothing to install this member, so it must write nothing "
        "to remove it — the user's own entry is not grim's to splice out"
    )
    state = json.loads((project_dir / ".grimoire/state.json").read_text())
    assert not any(r["name"] == "grim-mcp" for r in state["records"]), (
        "the record must still be dropped"
    )
