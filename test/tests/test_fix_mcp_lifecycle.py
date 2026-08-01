# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""MCP lifecycle regressions: lock eviction and entry-integrity on removal.

Two shipped defects, both specific to the ``mcp`` kind:

* ``drop_from_lock`` fanned its effective-set retain pass over
  skills/rules/agents only, then restamped ``declaration_hash`` anyway — so
  an uninstalled MCP server stayed in ``grimoire.lock`` inside a lock that
  read *fresh*, and the very next ``grim install`` re-spliced the server
  into every client config.
* ``uninstall`` spliced a managed MCP member out hash-blind, discarding a
  local customization — or, in the adoption case, an entry grim never wrote.
"""
from __future__ import annotations

import json
import tomllib  # stdlib (Python 3.11+)
from pathlib import Path

from src.helpers import make_bundle, write_config

DESCRIPTOR = """\
description = "Grimoire catalog search and install status over MCP."
summary = "grim as an MCP server"
keywords = "grimoire,mcp"

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


def _release_mcp(runner, project_dir: Path, registry: str, unique_repo: str) -> str:
    """Publish the MCP descriptor and return its fully-qualified reference."""
    path = project_dir / "src" / "mcp" / "grim-mcp.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(path), ref, "--kind", "mcp")
    return ref


def _detect_all_clients(project_dir: Path) -> None:
    (project_dir / ".opencode").mkdir(exist_ok=True)
    (project_dir / ".github").mkdir(exist_ok=True)
    (project_dir / ".github" / "copilot-instructions.md").write_text("# ci\n")


def _lock_mcp_names(project_dir: Path) -> list[str]:
    lock = project_dir / "grimoire.lock"
    if not lock.is_file():
        return []
    return [entry["name"] for entry in tomllib.loads(lock.read_text()).get("mcp", [])]


def _registered_servers(project_dir: Path) -> dict[str, list[str]]:
    """Every grim-managed MCP server name per vendor config, for assertions."""
    found: dict[str, list[str]] = {}
    claude = project_dir / ".mcp.json"
    if claude.is_file():
        found["claude"] = list(json.loads(claude.read_text()).get("mcpServers", {}))
    opencode = project_dir / "opencode.json"
    if opencode.is_file():
        found["opencode"] = list(json.loads(opencode.read_text()).get("mcp", {}))
    vscode = project_dir / ".vscode" / "mcp.json"
    if vscode.is_file():
        found["copilot"] = list(json.loads(vscode.read_text()).get("servers", {}))
    return found


def test_uninstalled_mcp_server_does_not_resurrect_on_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`add mcp` → `uninstall mcp` → `install` must not bring the server back.

    The lock entry survived under a freshly-restamped ``declaration_hash``,
    so ``require_fresh_lock`` passed and the installer re-materialized the
    server from a lock the user had every reason to believe was clean.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)
    _detect_all_clients(project_dir)
    write_config(project_dir)

    runner.json("add", ref)
    runner.json("install")
    assert _lock_mcp_names(project_dir) == ["grim-mcp"], "precondition: the server is locked"
    assert all("grim-mcp" in names for names in _registered_servers(project_dir).values()), (
        f"precondition: registered everywhere, got {_registered_servers(project_dir)}"
    )

    runner.json("uninstall", "mcp", "grim-mcp")
    assert _lock_mcp_names(project_dir) == [], "the uninstalled server must leave the lock"

    runner.json("install")
    assert _lock_mcp_names(project_dir) == [], "install must not re-add it to the lock"
    for client, names in _registered_servers(project_dir).items():
        assert "grim-mcp" not in names, f"{client} config resurrected the uninstalled server: {names}"

    status = runner.json("status")["items"]
    assert not any(row["name"] == "grim-mcp" for row in status), status


def test_removing_a_bundle_evicts_its_mcp_member_from_the_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A bundle-provided MCP member follows the same eviction path.

    ``Origin::Bundles`` re-derivation never ran over ``lock.mcp`` either, so
    dropping the bundle left an orphan member that ``install`` re-splices.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)
    _detect_all_clients(project_dir)
    bundle = make_bundle(f"{unique_repo}/bundles/stack", [("mcp", "grim-mcp", ref)], tag="1.0.0")

    write_config(project_dir, bundles={"stack": bundle.fq})
    runner.json("lock")
    runner.json("install")
    assert _lock_mcp_names(project_dir) == ["grim-mcp"], "precondition: the bundle locked its member"

    runner.json("remove", "bundle", "stack")
    assert _lock_mcp_names(project_dir) == [], "the bundle's mcp member must be evicted with the bundle"

    runner.json("install")
    assert _lock_mcp_names(project_dir) == [], "install must not resurrect the evicted member"


def test_uninstall_refuses_a_locally_edited_mcp_entry_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A managed entry the user edited is preserved; ``--force`` removes it.

    Every sibling removal path checks the recorded hash first. Uninstall did
    not, so it discarded a customization — and, when install had merely
    *adopted* an identical pre-existing member, an entry grim never wrote.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)
    # Claude only: a single config to edit keeps the drift unambiguous.
    write_config(project_dir)
    (project_dir / ".mcp.json").write_text('{"mcpServers": {"user-server": {"command": "keep-me"}}}')

    runner.json("add", ref)
    runner.json("install")
    cfg = project_dir / ".mcp.json"
    assert "grim-mcp" in json.loads(cfg.read_text())["mcpServers"], "precondition: registered"

    # The user customizes the managed entry in place.
    doc = json.loads(cfg.read_text())
    doc["mcpServers"]["grim-mcp"]["args"] = ["mcp", "--allow-writes"]
    edited = json.dumps(doc, indent=2)
    cfg.write_text(edited)

    refused = runner.run("uninstall", "mcp", "grim-mcp", check=False)
    assert refused.returncode == 65, (
        f"a drifted managed entry must be refused as a data error, got {refused.returncode}: {refused.stderr}"
    )
    assert cfg.read_text() == edited, "the refused config must be byte-unchanged"
    assert any(row["name"] == "grim-mcp" for row in runner.json("status")["items"]), (
        "the record must survive a refusal, or --force has nothing left to act on"
    )

    forced = runner.json("uninstall", "mcp", "grim-mcp", "--force")
    assert forced["status"] in ("uninstalled", "removed"), forced
    survivors = json.loads(cfg.read_text())["mcpServers"]
    assert "grim-mcp" not in survivors, "--force removes the edited entry"
    assert survivors["user-server"]["command"] == "keep-me", "the foreign entry is untouched"


def test_uninstall_removes_an_unmodified_mcp_entry_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The integrity gate must not change the ordinary path: an entry still
    matching what grim installed uninstalls with no flag, as before."""
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir, registry, unique_repo)
    _detect_all_clients(project_dir)
    write_config(project_dir)
    (project_dir / ".mcp.json").write_text('{"mcpServers": {"user-server": {"command": "keep-me"}}}')

    runner.json("add", ref)
    runner.json("install")

    out = runner.json("uninstall", "mcp", "grim-mcp")
    assert out["status"] in ("uninstalled", "removed"), out
    for client, names in _registered_servers(project_dir).items():
        assert "grim-mcp" not in names, f"{client} kept the managed entry: {names}"
    claude = json.loads((project_dir / ".mcp.json").read_text())
    assert claude["mcpServers"]["user-server"]["command"] == "keep-me", "user entry preserved"
