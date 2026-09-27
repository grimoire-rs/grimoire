# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Droid (Factory) custom droids and MCP servers.

Droid gained the Agent kind (`.factory/droids/<name>.md`) and MCP
(`.factory/mcp.json`) on 2026-09-27 (grimoire-rs/grimoire#145). The client is
`droid`; every path it writes sits under `.factory`.
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact
from src.runner import GrimRunner

AGENT_MD = (
    "---\n"
    "name: {name}\n"
    "description: Reviews diffs.\n"
    "model: inherit\n"
    "tools: Read, Grep\n"
    "metadata:\n"
    "  droid.reasoning-effort: high\n"
    "---\n"
    "You review.\n"
)

STDIO_MCP = """\
description = "d"

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]

[server.env]
GRIM_TOKEN = "${GITHUB_TOKEN}"
"""


def _push_agent(unique_repo: str, name: str = "reviewer"):
    return make_artifact(f"{unique_repo}/{name}", "agent", {f"{name}.md": AGENT_MD.format(name=name)}, tag="v1")


def _release_mcp(
    runner: GrimRunner, tmp_path: Path, registry: str, unique_repo: str, body: str, name: str = "srv", version: str = "1.0.0"
) -> str:
    descriptor = tmp_path / "src" / "mcp" / f"{name}.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(body)
    ref = f"{registry}/{unique_repo}/mcp/{name}:{version}"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def _row(runner: GrimRunner, name: str, *scope: str) -> dict:
    return next(r for r in runner.json("status", *scope)["items"] if r["name"] == name)


def _assert_droid(doc: str) -> None:
    assert doc.startswith(
        "---\nname: reviewer\ndescription: Reviews diffs.\nmodel: inherit\ntools:\n- Read\n- Grep\n"
        "reasoningEffort: high\n---\n"
    ), doc
    assert doc.endswith("You review.\n"), doc


def test_droid_agent_renders_at_both_scopes(
    grim_at, grim_binary: Path, grim_home: Path, project_dir: Path, unique_repo: str
) -> None:
    ag = _push_agent(unique_repo)
    (project_dir / "grimoire.toml").write_text(f'[agents]\nreviewer = "{ag.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.json("install", "--client", "droid")
    _assert_droid((project_dir / ".factory" / "droids" / "reviewer.md").read_text())
    assert not (project_dir / ".factory" / "agents").exists(), "the old stub path is never written"

    (grim_home / "grimoire.toml").write_text(f'[agents]\nreviewer = "{ag.fq}"\n')
    home = GrimRunner(grim_binary, grim_home)
    home.run("lock", "--global")
    home.json("install", "--global", "--client", "droid")
    _assert_droid((home.home / ".factory" / "droids" / "reviewer.md").read_text())


def test_droid_skips_a_dotted_agent_name_without_reporting_drift(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Droid only accepts `[a-z0-9_-]+`. A dotted name installs for the other
    clients and is skipped for Droid with a warning — and the skip is not
    pending drift a later install would try, and fail, to clear."""
    ag = _push_agent(unique_repo, name="code.rev")
    (project_dir / "grimoire.toml").write_text(
        f'[options]\nclients = ["claude", "droid"]\n\n[agents]\n"code.rev" = "{ag.fq}"\n'
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    result = runner.run("install", format="json")
    assert "skipped for droid" in result.stderr and "[a-z0-9_-]+" in result.stderr, result.stderr
    assert (project_dir / ".claude" / "agents" / "code.rev.md").is_file()
    assert not (project_dir / ".factory" / "droids").exists()

    row = _row(runner, "code.rev")
    assert row["state"] == "installed", row
    assert row["outputs_pending"] == [], row
    assert row["clients_missing"] == [], row
    again = runner.json("install")["items"]
    assert again[0]["status"] != "installed", f"a repeat install is a no-op: {again}"

    # Droid alone selected: the grammar is still what the warning names.
    alone = runner.run("install", "--client", "droid", format="json")
    assert "skipped for droid" in alone.stderr and "[a-z0-9_-]+" in alone.stderr, alone.stderr
    assert "no native target" not in alone.stderr.lower(), alone.stderr


def test_droid_writes_the_binding_name_over_a_frontmatter_name_it_rejects(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Bound as `coderev`, a `code.rev` agent installs for Droid, but its own
    frontmatter `name` is one Droid rejects — so the binding name is written."""
    ag = _push_agent(unique_repo, name="code.rev")
    (project_dir / "grimoire.toml").write_text(f'[agents]\ncoderev = "{ag.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock")
    result = runner.run("install", "--client", "droid", format="json")
    assert "written as 'coderev'" in result.stderr, result.stderr
    doc = (project_dir / ".factory" / "droids" / "coderev.md").read_text()
    assert doc.startswith("---\nname: coderev\ndescription: Reviews diffs.\n"), doc
    again = runner.json("install", "--client", "droid")["items"]
    assert again[0]["status"] != "installed", f"a repeat install is a no-op: {again}"


def test_droid_mcp_registers_at_both_scopes_and_self_heals(
    grim_at, grim_binary: Path, grim_home: Path, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """An install recorded before Droid gained MCP (simulated by a
    Claude-only install) lists Droid pending, the next install adds it, and a
    repeat install is byte-identical with nothing pending."""
    (project_dir / ".factory").mkdir()
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, tmp_path, registry, unique_repo, STDIO_MCP)
    (project_dir / "grimoire.toml").write_text(f'[mcp]\nsrv = "{ref}"\n')
    runner.run("lock")
    runner.json("install", "--client", "claude")
    cfg = project_dir / ".factory" / "mcp.json"
    assert not cfg.exists()
    assert any(o["client"] == "droid" for o in _row(runner, "srv")["outputs_pending"])

    runner.json("install")
    entry = json.loads(cfg.read_text())["mcpServers"]["srv"]
    assert entry == {"type": "stdio", "command": "grim", "args": ["mcp"], "env": {"GRIM_TOKEN": "${GITHUB_TOKEN}"}}
    before = cfg.read_bytes()
    runner.json("install")
    assert cfg.read_bytes() == before, "repeat install must be byte-identical"
    row = _row(runner, "srv")
    assert row["state"] == "installed" and row["outputs_pending"] == [], row

    home = GrimRunner(grim_binary, grim_home)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\nsrv = "{ref}"\n')
    home.run("lock", "--global")
    home.json("install", "--global", "--client", "droid")
    assert json.loads((home.home / ".factory" / "mcp.json").read_text())["mcpServers"]["srv"] == entry


def test_droid_mcp_skips_env_refs_it_does_not_expand(
    grim_at, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    runner = grim_at(project_dir)
    body = 'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["--token", "${TOKEN}"]\n'
    ref = _release_mcp(runner, tmp_path, registry, unique_repo, body)
    (project_dir / "grimoire.toml").write_text(f'[mcp]\nsrv = "{ref}"\n')
    runner.run("lock")
    result = runner.run("install", "--client", "claude,droid", format="json")
    assert "skipped for droid" in result.stderr and "command, args or url" in result.stderr, result.stderr
    assert not (project_dir / ".factory" / "mcp.json").exists()
    claude = json.loads((project_dir / ".mcp.json").read_text())["mcpServers"]["srv"]
    assert claude["args"] == ["--token", "${TOKEN}"], "other clients are unaffected"


def test_droid_mcp_oauth_client_id_is_written_other_fields_skip(
    grim_at, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    runner = grim_at(project_dir)
    remote = 'description = "d"\n\n[server]\ntransport = "http"\nurl = "https://x.example/mcp"\n\n[server.oauth]\n'
    written = _release_mcp(runner, tmp_path, registry, unique_repo, remote + 'client_id = "${CID}"\n', name="ok")
    skipped = _release_mcp(
        runner, tmp_path, registry, unique_repo, remote + 'client_id = "c"\nscopes = ["read"]\n', name="scoped"
    )
    (project_dir / "grimoire.toml").write_text(f'[mcp]\nok = "{written}"\nscoped = "{skipped}"\n')
    runner.run("lock")
    result = runner.run("install", "--client", "claude,droid", format="json")
    assert "oauth scopes has no Droid mapping" in result.stderr, result.stderr
    servers = json.loads((project_dir / ".factory" / "mcp.json").read_text())["mcpServers"]
    assert servers == {"ok": {"type": "http", "url": "https://x.example/mcp", "oauth": {"clientId": "${CID}"}}}


def test_droid_global_mcp_refuses_a_ui_copied_entry_then_force_replaces(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Toggling a project server in Droid's UI copies it into
    `~/.factory/mcp.json`. grim did not write that copy, so a global install
    of the same name refuses (65) and leaves it alone; `--force` replaces it,
    keeping Droid's own `disabled` toggle."""
    home = GrimRunner(grim_binary, grim_home)
    ref = _release_mcp(home, tmp_path, registry, unique_repo, STDIO_MCP)
    cfg = home.home / ".factory" / "mcp.json"
    cfg.parent.mkdir(parents=True)
    copied = {"mcpServers": {"srv": {"type": "stdio", "command": "grim", "args": ["mcp"], "disabled": True}}}
    cfg.write_text(json.dumps(copied, indent=2))
    (grim_home / "grimoire.toml").write_text(f'[mcp]\nsrv = "{ref}"\n')
    home.run("lock", "--global")

    result = home.run("install", "--global", "--client", "droid", check=False)
    assert result.returncode == 65, result.stderr
    assert "--force" in result.stderr, result.stderr
    assert json.loads(cfg.read_text()) == copied, "the refusal leaves the copy untouched"

    home.json("install", "--global", "--client", "droid", "--force")
    entry = json.loads(cfg.read_text())["mcpServers"]["srv"]
    assert entry["env"] == {"GRIM_TOKEN": "${GITHUB_TOKEN}"}, "grim's entry replaces the copy"
    assert entry["disabled"] is True, "the toggle is Droid's state and survives the replace"


def test_droid_global_mcp_keeps_a_toggle_across_status_install_and_update(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Droid's toggles write `disabled`/`disabledTools` into grim's own entry.
    They are Droid's state: the entry stays not-modified, a reinstall or a
    pin-change update keeps them, and uninstall still removes the entry."""
    home = GrimRunner(grim_binary, grim_home)
    ref = _release_mcp(home, tmp_path, registry, unique_repo, STDIO_MCP)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\nsrv = "{ref.rsplit(":", 1)[0]}:1"\n')
    home.run("lock", "--global")
    home.json("install", "--global", "--client", "droid")
    cfg = home.home / ".factory" / "mcp.json"
    doc = json.loads(cfg.read_text())
    doc["mcpServers"]["srv"] |= {"disabled": True, "disabledTools": ["search"]}
    cfg.write_text(json.dumps(doc, indent=2))
    toggled = {"disabled": True, "disabledTools": ["search"]}

    assert _row(home, "srv", "--global")["state"] == "installed", "a toggle is not a user edit"
    home.json("install", "--global", "--client", "droid")
    assert json.loads(cfg.read_text())["mcpServers"]["srv"].items() >= toggled.items()

    _release_mcp(home, tmp_path, registry, unique_repo, STDIO_MCP.replace('["mcp"]', '["mcp", "--v2"]'), version="1.1.0")
    home.json("update", "--global", "--client", "droid")
    entry = json.loads(cfg.read_text())["mcpServers"]["srv"]
    assert entry["args"] == ["mcp", "--v2"], entry
    assert entry.items() >= toggled.items(), f"a pin-change update keeps the toggle: {entry}"
    assert _row(home, "srv", "--global")["state"] == "installed"

    home.json("uninstall", "--global", "mcp", "srv")
    assert "srv" not in json.loads(cfg.read_text()).get("mcpServers", {}), "uninstall removes the toggled entry"
