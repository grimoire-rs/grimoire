# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`mcp` artifact kind — release wire shape and kind inference.

An MCP server descriptor (`mcp/<name>.toml`) releases as a single
canonical-JSON layer (``application/vnd.grimoire.mcp.v1+json``) with the
kind riding on the ``com.grimoire.kind`` annotation, exactly like every
other kind. Install/registration coverage lives alongside once the
vendor MCP writers land.
"""
from __future__ import annotations

import hashlib
import json
import sys
import tomllib  # stdlib (Python 3.11+)
from pathlib import Path

import pytest

from src.helpers import write_config
from src.registry import fetch_blob, fetch_manifest, retag

MCP_LAYER_MEDIA_TYPE = "application/vnd.grimoire.mcp.v1+json"

DESCRIPTOR = """\
description = "Grimoire catalog search and install status over MCP."
summary = "grim as an MCP server"
keywords = "grimoire,mcp"

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


def _write_descriptor(project_dir: Path, name: str = "grim-mcp", body: str = DESCRIPTOR) -> Path:
    path = project_dir / "mcp" / f"{name}.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    return path


def test_release_mcp_wire_shape_and_layer_content(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The pushed manifest carries the MCP JSON layer, the OCI empty
    config, and the ``com.grimoire.kind: mcp`` annotation; the layer blob
    is the canonical JSON serialization of the descriptor."""
    descriptor = _write_descriptor(project_dir)
    repo = f"{registry}/{unique_repo}/mcp/grim-mcp"
    repo_path = f"{unique_repo}/mcp/grim-mcp"
    runner = grim_at(project_dir)

    out = runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    assert out["pushed"] is True
    assert set(out["tags"]) == {"1.0.0", "1.0", "1", "latest"}

    manifest = fetch_manifest(repo_path, "1.0.0")
    assert manifest["config"]["mediaType"] == "application/vnd.oci.empty.v1+json"
    assert "artifactType" not in manifest
    layers = manifest["layers"]
    assert len(layers) == 1
    assert layers[0]["mediaType"] == MCP_LAYER_MEDIA_TYPE

    annotations = manifest["annotations"]
    assert annotations["com.grimoire.kind"] == "mcp"
    assert annotations["org.opencontainers.image.title"] == "grim-mcp"
    assert annotations["org.opencontainers.image.description"].startswith("Grimoire catalog")
    assert annotations["com.grimoire.summary"] == "grim as an MCP server"

    blob = json.loads(fetch_blob(repo_path, layers[0]["digest"]))
    assert blob["server"]["transport"] == "stdio"
    assert blob["server"]["command"] == "grim"
    assert blob["server"]["args"] == ["mcp"]


def test_release_mcp_is_idempotent_and_add_infers_kind(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Re-releasing identical content is a no-op (same digest), and
    `grim add` infers the ``mcp`` kind from the annotation."""
    descriptor = _write_descriptor(project_dir)
    repo = f"{registry}/{unique_repo}/mcp/grim-mcp"
    runner = grim_at(project_dir)

    first = runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    second = runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    assert first["manifest_digest"] == second["manifest_digest"]

    write_config(project_dir)
    out = runner.json("add", f"{repo}:1.0.0")
    assert out["kind"] == "mcp", f"kind must be inferred from the annotation, got {out['kind']!r}"
    assert out["name"] == "grim-mcp"
    assert out["status"] == "added"

    # The declaration lands in the [mcp] table and undeclares cleanly.
    config = (project_dir / "grimoire.toml").read_text()
    assert "[mcp]" in config
    lock = (project_dir / "grimoire.lock").read_text()
    assert "[[mcp]]" in lock
    removed = runner.json("remove", "mcp", "grim-mcp")
    assert removed["status"] == "removed"


def test_release_mcp_invalid_descriptor_exits_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A descriptor with a `${VAR:-default}` reference (unsupported v1) is
    rejected at release time with a data error."""
    bad = DESCRIPTOR + 'env = { HOME_DIR = "${HOME:-/root}" }\n'
    descriptor = _write_descriptor(project_dir, name="bad", body=bad)
    runner = grim_at(project_dir)

    result = runner.run(
        "release", str(descriptor), f"{registry}/{unique_repo}/mcp/bad:1.0.0", "--kind", "mcp", check=False
    )
    assert result.returncode == 65, result.stderr
    assert "${VAR}" in result.stderr or "environment reference" in result.stderr


def test_build_toml_without_kind_hints_mcp(grim_at, project_dir: Path) -> None:
    """A `.toml` with a `[server]` table detected as a bundle produces the
    --kind mcp hint instead of a cryptic parse error."""
    descriptor = _write_descriptor(project_dir)
    runner = grim_at(project_dir)

    result = runner.run("build", str(descriptor), check=False)
    assert result.returncode == 65
    assert "--kind mcp" in result.stderr


# ── Install: per-client config registration ──────────────────────────────

ENV_DESCRIPTOR = """\
description = "Server with an env reference."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
env = { GRIM_TOKEN = "${GITHUB_TOKEN}" }
"""


def _release(runner, project_dir: Path, registry: str, unique_repo: str, body: str = DESCRIPTOR) -> str:
    descriptor = _write_descriptor(project_dir / "src", name="grim-mcp", body=body)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def _detect_all_clients(project_dir: Path) -> None:
    (project_dir / ".opencode").mkdir()
    (project_dir / ".github").mkdir()
    (project_dir / ".github" / "copilot-instructions.md").write_text("# ci\n")


def test_install_registers_entries_in_every_client_config(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A project install writes the vendor-native entry into each detected
    client's MCP config — Claude's `.mcp.json` (canonical `${VAR}`),
    OpenCode's `opencode.json` (`{env:VAR}`, command array), and VS Code's
    `.vscode/mcp.json` (`${env:VAR}`) — preserving user content."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=ENV_DESCRIPTOR)
    _detect_all_clients(project_dir)
    # Pre-seed user-owned configs with foreign content that must survive.
    (project_dir / ".mcp.json").write_text(
        '{\n  "mcpServers": {\n    "user-server": {"command": "keep-me"}\n  }\n}\n'
    )
    (project_dir / "opencode.json").write_text('{\n  "model": "anthropic/claude"\n}\n')
    write_config(project_dir)

    # `--no-install` isolates the `install` step under test (which registers
    # the entry into every detected client) from the default install-on-add.
    runner.json("add", "--no-install", ref)
    rows = runner.json("install")["items"]
    assert rows[0]["status"] == "installed", rows

    claude = json.loads((project_dir / ".mcp.json").read_text())
    assert claude["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert claude["mcpServers"]["grim-mcp"]["env"]["GRIM_TOKEN"] == "${GITHUB_TOKEN}"
    assert claude["mcpServers"]["user-server"]["command"] == "keep-me", "user entry preserved"

    opencode = json.loads((project_dir / "opencode.json").read_text())
    assert opencode["mcp"]["grim-mcp"]["type"] == "local"
    assert opencode["mcp"]["grim-mcp"]["command"] == ["grim", "mcp"], "command is one array"
    assert opencode["mcp"]["grim-mcp"]["environment"]["GRIM_TOKEN"] == "{env:GITHUB_TOKEN}"
    assert opencode["mcp"]["grim-mcp"]["enabled"] is True
    assert opencode["model"] == "anthropic/claude", "user key preserved"

    vscode = json.loads((project_dir / ".vscode" / "mcp.json").read_text())
    assert vscode["servers"]["grim-mcp"]["type"] == "stdio"
    assert vscode["servers"]["grim-mcp"]["env"]["GRIM_TOKEN"] == "${env:GITHUB_TOKEN}"

    status = runner.json("status")["items"]
    row = next(r for r in status if r["name"] == "grim-mcp")
    assert row["kind"] == "mcp"
    assert row["state"] == "installed"


def test_project_status_reports_installed_for_claude_mcp_without_claude_dir(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Project-scope sibling of the global regression
    (test_global_status_reports_installed_for_claude_mcp_without_claude_dir):
    ``.mcp.json`` (Claude's project MCP config) is a SIBLING of ``.claude/``,
    not something inside it — ``Vendor::detect`` checks only ``.claude/``."""
    runner = grim_at(bare_project_dir)
    ref = _release(runner, bare_project_dir, registry, unique_repo)

    # CRITICAL repro condition: another vendor IS detected (.codex/) so
    # detect_clients returns a non-empty set that excludes claude. No
    # .claude/ dir is ever created.
    (bare_project_dir / ".codex").mkdir()
    write_config(bare_project_dir)
    runner.json("add", "--no-install", ref)
    rows = runner.json("install", "--client", "claude")["items"]
    assert rows[0]["status"] == "installed", rows

    # Sanity: write side is correct — fails only on the read side below.
    claude_json = bare_project_dir / ".mcp.json"
    assert claude_json.is_file()
    assert json.loads(claude_json.read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert not (bare_project_dir / ".claude").exists(), "install must not create .claude itself"

    state_text = (bare_project_dir / ".grimoire" / "state.json").read_text()
    assert "grim-mcp" in state_text and '"claude"' in state_text, (
        f"install-state record must carry the mcp entry: {state_text}"
    )

    status_rows = runner.json("status")["items"]
    row = next(r for r in status_rows if r["name"] == "grim-mcp")
    assert row["state"] == "installed", (
        "read side must report the mcp artifact installed: Vendor::detect() for "
        "Claude checks only .claude/, never the sibling .mcp.json config; "
        f"got state={row['state']!r}"
    )


def test_reformatting_the_config_is_not_modified_but_a_value_change_is(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The drift check is semantic: reordering keys / reformatting the file
    leaves the artifact `installed`; changing the managed value flips it to
    `modified`, refuses install without --force, and --force restores it."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    write_config(project_dir)
    runner.json("add", ref)
    runner.json("install")
    cfg = project_dir / ".mcp.json"

    # Reformat + reorder without changing values: still installed.
    doc = json.loads(cfg.read_text())
    entry = doc["mcpServers"]["grim-mcp"]
    reordered = {"mcpServers": {"grim-mcp": dict(reversed(list(entry.items())))}}
    cfg.write_text(json.dumps(reordered, indent=None, separators=(",", ": ")))
    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "installed"

    # A real value change: modified + refused without --force.
    doc = json.loads(cfg.read_text())
    doc["mcpServers"]["grim-mcp"]["command"] = "evil"
    cfg.write_text(json.dumps(doc))
    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "modified"
    refused = runner.run("install", check=False)
    assert refused.returncode == 65, refused.stderr
    assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]["command"] == "evil", (
        "a refused install must not overwrite the user's edit"
    )
    forced = runner.run("install", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"

    # Deleting the managed entry (file survives) reads as missing.
    cfg.write_text('{"mcpServers": {"other": {"command": "x"}}}')
    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "missing"


def test_reformatting_codex_config_toml_is_not_modified_but_a_value_change_is(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The drift check for Codex's TOML-spliced entry is semantic too:
    reformatting `config.toml` leaves the artifact `installed`; changing
    the managed value flips it to `modified` (mirrors
    `test_reformatting_the_config_is_not_modified_but_a_value_change_is`,
    Claude's JSON-spliced counterpart, for the TOML target)."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    (project_dir / ".codex").mkdir()  # detect Codex only
    write_config(project_dir)
    runner.json("add", ref)
    runner.json("install")
    cfg = project_dir / ".codex" / "config.toml"
    doc = tomllib.loads(cfg.read_text())
    assert doc["mcp_servers"]["grim-mcp"]["command"] == "grim"

    # Reformat (whitespace / spacing) without changing values: still installed.
    cfg.write_text('[mcp_servers.grim-mcp]\ncommand   =    "grim"\nargs = [ "mcp" ]\n')
    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "installed"

    # A real value change: modified + refused without --force.
    cfg.write_text('[mcp_servers.grim-mcp]\ncommand = "evil"\nargs = ["mcp"]\n')
    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "modified"
    refused = runner.run("install", check=False)
    assert refused.returncode == 65, refused.stderr
    assert tomllib.loads(cfg.read_text())["mcp_servers"]["grim-mcp"]["command"] == "evil", (
        "a refused install must not overwrite the user's edit"
    )
    forced = runner.run("install", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert tomllib.loads(cfg.read_text())["mcp_servers"]["grim-mcp"]["command"] == "grim"

    # Deleting the managed entry (file survives) reads as missing.
    cfg.write_text('[mcp_servers.other]\ncommand = "x"\n')
    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "missing"


REFINED_DESCRIPTOR = """\
description = "Refined stdio server."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
timeout = 30000
always_load = true
cwd = "./srv"
"""


def test_install_mcp_refinement_fields_project_claude_and_opencode(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``timeout``/``always_load``/``cwd`` project onto each client's
    native keys: Claude gets ``timeout`` + ``alwaysLoad``, OpenCode gets
    ``timeout`` + ``cwd``; fields with no native target are dropped."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=REFINED_DESCRIPTOR)
    (project_dir / ".opencode").mkdir()
    write_config(project_dir)
    runner.json("add", ref)
    runner.json("install")

    claude_entry = json.loads((project_dir / ".mcp.json").read_text())["mcpServers"]["grim-mcp"]
    assert claude_entry["timeout"] == 30000
    assert claude_entry["alwaysLoad"] is True
    assert "cwd" not in claude_entry, "cwd has no Claude target"

    oc_entry = json.loads((project_dir / "opencode.json").read_text())["mcp"]["grim-mcp"]
    assert oc_entry["timeout"] == 30000
    assert oc_entry["cwd"] == "./srv"
    assert "alwaysLoad" not in oc_entry and "always_load" not in oc_entry


REMOTE_HEADERS_DESCRIPTOR = """\
description = "Remote MCP with headers."

[server]
transport = "http"
url = "https://api.example.com/mcp"

[server.headers]
X-Api-Version = "2026-07"
Authorization = "Bearer ${API_TOKEN}"
"""


def test_install_mcp_codex_remote_headers_render_valid_toml(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A remote descriptor with a static and a Bearer header registers a
    Codex entry mapping them onto ``http_headers`` /
    ``bearer_token_env_var`` (Codex's upstream RawMcpServerConfig header
    surfaces); the spliced file is valid TOML and uninstall round-trips."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=REMOTE_HEADERS_DESCRIPTOR)
    (project_dir / ".codex").mkdir()  # detect Codex only
    write_config(project_dir)
    runner.json("add", ref)
    runner.json("install")

    cfg = project_dir / ".codex" / "config.toml"
    doc = tomllib.loads(cfg.read_text())
    entry = doc["mcp_servers"]["grim-mcp"]
    assert entry["url"] == "https://api.example.com/mcp"
    assert entry["http_headers"] == {"X-Api-Version": "2026-07"}
    assert entry["bearer_token_env_var"] == "API_TOKEN", (
        "Authorization: Bearer ${VAR} maps to bearer_token_env_var, never inlined"
    )
    assert "env_http_headers" not in entry

    runner.json("uninstall", "mcp", "grim-mcp")
    doc = tomllib.loads(cfg.read_text()) if cfg.exists() else {}
    assert "grim-mcp" not in doc.get("mcp_servers", {}), "uninstall removes only the managed entry"


def test_update_pin_change_resplices_codex_mcp_entry_in_place(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A version bump on a declared MCP artifact re-splices the Codex
    `[mcp_servers.<name>]` entry in place (plan C1/C2): `grim update`
    rewrites just the managed table when the pin changes, and `status`
    reflects the new value."""
    runner = grim_at(project_dir)
    # The manifest title (and thus the artifact's canonical name) is the
    # descriptor's source file stem — a *fixed* filename across releases is
    # required so a version bump stays the same binding identity, exactly
    # like `_release()`'s convention below.
    descriptor = project_dir / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    repo_path = f"{unique_repo}/mcp/grim-mcp"
    repo = f"{registry}/{repo_path}"

    descriptor.write_text(DESCRIPTOR)
    first = runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    runner.json("release", str(descriptor), f"{repo}:stable", "--kind", "mcp")  # floating tag, initially v1

    (project_dir / ".codex").mkdir()  # detect Codex only
    (project_dir / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{repo}:stable"\n')
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".codex" / "config.toml"
    doc = tomllib.loads(cfg.read_text())
    assert doc["mcp_servers"]["grim-mcp"]["command"] == "grim"

    # Publish v2 content (same filename, changed body) and move the floating
    # tag onto it (rolling release).
    descriptor.write_text(DESCRIPTOR.replace('command = "grim"', 'command = "grim2"'))
    second = runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")
    assert first["manifest_digest"] != second["manifest_digest"]
    retag(repo_path, "stable", second["manifest_digest"])

    update_rows = runner.json("update")["items"]
    # Regression guard: a still-declared mcp record must produce exactly one
    # `updated` row, never a spurious extra `removed` row from the prune
    # pass treating it as orphaned (`declared` omitting `lock.mcp`).
    assert len(update_rows) == 1, update_rows
    assert update_rows[0]["action"] == "updated", update_rows

    doc = tomllib.loads(cfg.read_text())
    assert doc["mcp_servers"]["grim-mcp"]["command"] == "grim2", (
        "the entry must re-splice in place at the new pin"
    )
    assert doc["mcp_servers"]["grim-mcp"]["args"] == ["mcp"], "unrelated field carried through the resplice"

    status = runner.json("status")["items"]
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "installed"


def test_prune_removes_codex_mcp_entry_when_declaration_dropped(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Dropping an MCP declaration and re-locking prunes the orphaned
    record through the shared uninstall seam — for Codex this must remove
    only the managed `[mcp_servers.<name>]` entry from `config.toml`,
    never the file, on the update/prune path (mirrors the explicit
    `grim uninstall` coverage in
    `test_uninstall_codex_mcp_entry_removed_file_and_foreign_keys_remain`)."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    (project_dir / ".codex").mkdir()
    (project_dir / ".codex" / "config.toml").write_text('[mcp_servers.other-server]\ncommand = "npx"\n')
    write_config(project_dir)
    runner.json("add", ref)
    runner.json("install")

    cfg = project_dir / ".codex" / "config.toml"
    assert "grim-mcp" in tomllib.loads(cfg.read_text())["mcp_servers"]

    # Drop the declaration and re-lock — the prune pass must reap the
    # orphaned Codex MCP record.
    (project_dir / "grimoire.toml").write_text("")
    runner.run("lock", check=False)
    update_rows = runner.json("update")["items"]
    assert any(r["action"] == "removed" for r in update_rows), update_rows

    assert cfg.is_file(), "config.toml must survive the prune"
    doc = tomllib.loads(cfg.read_text())
    assert "grim-mcp" not in doc.get("mcp_servers", {}), "pruned entry must be removed"
    assert doc["mcp_servers"]["other-server"]["command"] == "npx", "foreign entry preserved"

    status = runner.json("status")["items"]
    assert not any(r["name"] == "grim-mcp" for r in status), status


def test_uninstall_removes_entries_but_never_the_config_files(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    _detect_all_clients(project_dir)
    (project_dir / ".mcp.json").write_text('{"mcpServers": {"user-server": {"command": "keep-me"}}}')
    write_config(project_dir)
    runner.json("add", ref)
    runner.json("install")

    out = runner.json("uninstall", "mcp", "grim-mcp")
    assert out["status"] in ("uninstalled", "removed"), out

    claude = json.loads((project_dir / ".mcp.json").read_text())
    assert "grim-mcp" not in claude.get("mcpServers", {}), "managed entry removed"
    assert claude["mcpServers"]["user-server"]["command"] == "keep-me", "user entry preserved"
    opencode = json.loads((project_dir / "opencode.json").read_text())
    assert "grim-mcp" not in opencode.get("mcp", {})
    vscode_cfg = project_dir / ".vscode" / "mcp.json"
    assert vscode_cfg.is_file(), "the config file itself must survive"
    assert "grim-mcp" not in json.loads(vscode_cfg.read_text()).get("servers", {})


def test_global_claude_splice_preserves_user_state(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """A global install splices only the managed member into the user's
    live `~/.claude.json` — every byte before the managed span survives."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor_dir = tmp_path / "src"
    descriptor = descriptor_dir / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    user_state = (
        '{\n'
        '  "numStartups": 42,\n'
        '  "tipsHistory": {"tip-a": 3},\n'
        '  "projects": {\n'
        '    "/home/u/dev/x": {"allowedTools": [], "history": [{"display": "hi"}]}\n'
        '  }\n'
        '}\n'
    )
    claude_json = runner.home / ".claude.json"
    claude_json.write_text(user_state)
    # Make Claude the only detected global client.
    (runner.home / ".claude").mkdir()

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global")["items"]
    assert rows[0]["status"] == "installed", rows

    text = claude_json.read_text()
    prefix_end = user_state.rfind("}")  # everything before the final closing brace
    assert text.startswith(user_state[: prefix_end - 1].rstrip().rstrip(",")) or (
        '"numStartups": 42' in text and '"history": [{"display": "hi"}]' in text
    ), f"user state must survive byte-preserving: {text}"
    doc = json.loads(text)
    assert doc["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert doc["numStartups"] == 42

    # Uninstall restores the original bytes exactly.
    runner.json("uninstall", "mcp", "grim-mcp", "--global")
    assert claude_json.read_text() == user_state, "uninstall must restore the original file byte-for-byte"


def test_global_status_reports_installed_for_claude_mcp_without_claude_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Regression: a global MCP install pinned to Claude must be reported
    ``installed`` by ``grim status --global`` even when no ``~/.claude``
    directory ever exists — only ``~/.claude.json`` (the MCP config, a
    SIBLING of ``~/.claude``, not something inside it). ``Vendor::detect``
    for Claude checks only the ``~/.claude`` directory, never the MCP
    config file, so when another client IS detected (Codex here, via its
    own native config root) the non-empty-active-set fallback in
    ``detect_clients`` never fires and the recorded Claude output is
    filtered out of the active set on every read-side derivation — even
    though both the config file and the install-state record are correct.
    """
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor_dir = tmp_path / "src"
    descriptor = descriptor_dir / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    # CRITICAL repro condition: another vendor IS detected (Codex, via its
    # native config root ~/.codex) so `detect_clients` returns the
    # non-empty set [codex] and the all-clients fallback never fires. No
    # ~/.claude directory is ever created.
    (runner.home / ".codex").mkdir()
    assert not (runner.home / ".claude").exists()

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global", "--client", "claude")["items"]
    assert rows[0]["status"] == "installed", rows

    # Sanity: the write side is correct — both the config file and the
    # install-state record carry the entry, so the assertion below can
    # only fail because of the READ side.
    claude_json = runner.home / ".claude.json"
    assert claude_json.is_file(), "install must write ~/.claude.json even without ~/.claude"
    claude_doc = json.loads(claude_json.read_text())
    assert claude_doc["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert not (runner.home / ".claude").exists(), "install must not create ~/.claude itself"

    state_text = (grim_home / "state" / "global.json").read_text()
    assert "grim-mcp" in state_text and '"claude"' in state_text, (
        f"install-state record must carry the mcp entry: {state_text}"
    )

    status_rows = runner.json("status", "--global")["items"]
    row = next(r for r in status_rows if r["name"] == "grim-mcp")
    assert row["state"] == "installed", (
        "read side must report the mcp artifact installed: Vendor::detect() for "
        "Claude checks only ~/.claude, never the sibling ~/.claude.json MCP "
        f"config; got state={row['state']!r}"
    )


def test_global_status_reports_installed_for_copilot_mcp_without_copilot_skills_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Same regression class for Copilot: the global MCP config lives at
    ``~/.copilot/mcp-config.json``, a SIBLING of ``~/.copilot/skills`` (the
    directory ``Vendor::detect`` actually checks) — a Copilot-pinned MCP
    install with no ``~/.copilot/skills`` present, but another vendor
    detected, must still read back as installed. Mirrors
    ``test_global_status_reports_installed_for_claude_mcp_without_claude_dir``."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    # CRITICAL repro condition, same as the Claude variant above.
    (runner.home / ".codex").mkdir()
    assert not (runner.home / ".copilot").exists()

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global", "--client", "copilot")["items"]
    assert rows[0]["status"] == "installed", rows

    copilot_json = runner.home / ".copilot" / "mcp-config.json"
    assert copilot_json.is_file(), "install must write ~/.copilot/mcp-config.json even without ~/.copilot/skills"
    copilot_doc = json.loads(copilot_json.read_text())
    assert copilot_doc["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert not (runner.home / ".copilot" / "skills").exists(), "install must not create ~/.copilot/skills itself"

    state_text = (grim_home / "state" / "global.json").read_text()
    assert "grim-mcp" in state_text and '"copilot"' in state_text, (
        f"install-state record must carry the mcp entry: {state_text}"
    )

    status_rows = runner.json("status", "--global")["items"]
    row = next(r for r in status_rows if r["name"] == "grim-mcp")
    assert row["state"] == "installed", (
        "read side must report the mcp artifact installed: Vendor::detect() for "
        "Copilot checks only ~/.copilot/skills, never the sibling "
        f"~/.copilot/mcp-config.json; got state={row['state']!r}"
    )


def test_global_copilot_registers_env_ref_descriptors_verbatim(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Copilot CLI expands `${VAR}` in its global `mcp-config.json` itself
    (live-verified, CLI 1.0.88), so an env-ref descriptor registers there with
    the reference written as authored — never its value. Grim skipped Copilot
    for such descriptors until 2026-09-27: an install recorded before then
    (simulated by a Claude-only first install) self-heals on the next install
    without disturbing the existing entry, and a repeat install is a no-op."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    # All three clients must be detected; an unmarked home resolves to the
    # generic `agents` client, which has no MCP surface at all.
    for marker in (".claude", ".config/opencode/skills", ".copilot/skills"):
        (runner.home / marker).mkdir(parents=True, exist_ok=True)
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(ENV_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "claude")
    copilot_cfg = runner.home / ".copilot" / "mcp-config.json"
    assert not copilot_cfg.exists(), "the pre-change record carries no Copilot output"
    pending = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert any(o["client"] == "copilot" for o in pending["outputs_pending"]), pending
    claude_before = (runner.home / ".claude.json").read_bytes()

    result = runner.run("install", "--global", check=False)
    assert result.returncode == 0, result.stderr
    assert "substitution" not in result.stderr, f"no Copilot skip any more: {result.stderr}"
    entry = json.loads(copilot_cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["type"] == "local"
    assert entry["env"]["GRIM_TOKEN"] == "${GITHUB_TOKEN}", "reference written verbatim"
    assert (runner.home / ".claude.json").read_bytes() == claude_before, "existing entry untouched"
    opencode = json.loads((runner.home / ".config" / "opencode" / "opencode.json").read_text())
    assert opencode["mcp"]["grim-mcp"]["environment"]["GRIM_TOKEN"] == "{env:GITHUB_TOKEN}"

    before = copilot_cfg.read_bytes()
    runner.json("install", "--global")
    assert copilot_cfg.read_bytes() == before, "repeat install must be byte-identical"
    row = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    assert row["outputs_pending"] == [], row


def test_project_repeat_install_is_byte_stable_for_every_json_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C2 idempotency, proven for the three existing JSON-spliced clients:
    a second `grim install` with an unchanged pin writes exactly one entry
    per client config, byte-identical to the first install's output."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    _detect_all_clients(project_dir)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    first = runner.json("install")["items"]
    assert first[0]["status"] == "installed", first

    claude_cfg = project_dir / ".mcp.json"
    opencode_cfg = project_dir / "opencode.json"
    vscode_cfg = project_dir / ".vscode" / "mcp.json"
    claude_before = claude_cfg.read_text()
    opencode_before = opencode_cfg.read_text()
    vscode_before = vscode_cfg.read_text()

    second = runner.json("install")["items"]
    assert second[0]["status"] == "unchanged", second

    assert claude_cfg.read_text() == claude_before, "claude config must be byte-stable on repeat install"
    assert opencode_cfg.read_text() == opencode_before, "opencode config must be byte-stable on repeat install"
    assert vscode_cfg.read_text() == vscode_before, "vscode config must be byte-stable on repeat install"

    for cfg, container in (
        (claude_cfg, "mcpServers"),
        (opencode_cfg, "mcp"),
        (vscode_cfg, "servers"),
    ):
        doc = json.loads(cfg.read_text())
        assert list(doc[container].keys()).count("grim-mcp") == 1, (
            f"{cfg}: exactly one grim-mcp entry expected, got {doc[container]!r}"
        )


# client, project MCP config path, container key (single top-level key —
# Amp's is a literal dotted key, not a nested `amp` -> `mcpServers` object).
_WAVE1_MCP_CLIENTS = [
    ("cursor", ".cursor/mcp.json", "mcpServers"),
    ("kiro", ".kiro/settings/mcp.json", "mcpServers"),
    ("junie", ".junie/mcp/mcp.json", "mcpServers"),
    ("gemini", ".gemini/settings.json", "mcpServers"),
    ("zed", ".zed/settings.json", "context_servers"),
    ("amp", ".amp/settings.json", "amp.mcpServers"),
]


@pytest.mark.parametrize("client, config_rel, container_key", _WAVE1_MCP_CLIENTS)
def test_project_repeat_install_is_byte_stable_for_wave1_mcp_clients(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    client: str,
    config_rel: str,
    container_key: str,
) -> None:
    """C2 idempotency, extended to the six wave-1 MCP splice targets: a
    second `grim install` with an unchanged pin writes exactly one entry,
    byte-identical to the first install's output — proving self-heal
    (Principle 9) end-to-end for every new splice target, including Zed's
    JSONC-tolerant path and Amp's literal dotted `amp.mcpServers` key.
    Mirrors `test_project_repeat_install_is_byte_stable_for_every_json_client`
    (the three pre-existing JSON-spliced clients)."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    first = runner.json("install", "--client", client)["items"]
    assert first[0]["status"] == "installed", first

    cfg = project_dir / config_rel
    before = cfg.read_text()

    second = runner.json("install", "--client", client)["items"]
    assert second[0]["status"] == "unchanged", second

    assert cfg.read_text() == before, f"{client} config must be byte-stable on repeat install"
    doc = json.loads(before)
    assert list(doc[container_key].keys()).count("grim-mcp") == 1, (
        f"{client}: exactly one grim-mcp entry expected in {cfg}, got {doc[container_key]!r}"
    )


def test_global_codex_registers_entry_in_config_toml_idempotent(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """A global Codex MCP install writes the `[mcp_servers.<name>]` entry
    into `$CODEX_HOME/config.toml` (plan C1); a repeat install is
    byte-stable with exactly one entry (idempotency, plan C2)."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor_dir = tmp_path / "src"
    descriptor = descriptor_dir / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    codex_home = grim_home.parent / "codex_home"
    runner.env["CODEX_HOME"] = str(codex_home)

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global", "--client", "codex")["items"]
    assert rows[0]["status"] == "installed", rows
    assert rows[0]["target"] is not None, "target must be non-null once Codex MCP registration lands"

    config = codex_home / "config.toml"
    assert config.is_file(), "Codex global MCP registration must land at $CODEX_HOME/config.toml"
    doc = tomllib.loads(config.read_text())
    assert doc["mcp_servers"]["grim-mcp"]["command"] == "grim"
    first_text = config.read_text()

    # Repeat install is byte-stable with exactly one entry.
    rows2 = runner.json("install", "--global", "--client", "codex")["items"]
    assert rows2[0]["status"] == "unchanged", rows2
    second_text = config.read_text()
    assert second_text == first_text, "repeat Codex install must be byte-stable"
    assert second_text.count("[mcp_servers.grim-mcp]") == 1


def test_project_codex_config_toml_preserves_comments_and_foreign_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A pre-existing `.codex/config.toml` with user comments and unrelated
    keys/tables must survive an MCP install untouched outside the managed
    `[mcp_servers.grim-mcp]` entry (span-preserving splice, plan C1)."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    (project_dir / ".codex").mkdir()  # detect Codex only
    user_toml = (
        "# managed by the user, not grim\n"
        "model = \"gpt-5-codex\"\n"
        "\n"
        "[sandbox]\n"
        "mode = \"workspace-write\"\n"
        "\n"
        "[mcp_servers.other-server]\n"
        "command = \"npx\"\n"
    )
    (project_dir / ".codex" / "config.toml").write_text(user_toml)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    rows = runner.json("install")["items"]
    assert rows[0]["status"] == "installed", rows

    text = (project_dir / ".codex" / "config.toml").read_text()
    assert "# managed by the user, not grim" in text, "user comment must survive"
    assert "model = \"gpt-5-codex\"" in text, "unrelated key must survive"
    doc = tomllib.loads(text)
    assert doc["sandbox"]["mode"] == "workspace-write"
    assert doc["mcp_servers"]["other-server"]["command"] == "npx", "foreign mcp server entry must survive"
    assert doc["mcp_servers"]["grim-mcp"]["command"] == "grim"


def test_uninstall_codex_mcp_entry_removed_file_and_foreign_keys_remain(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Arch-verify gap (plan C1): `uninstall` must remove only the managed
    Codex TOML entry — never the file, never a foreign `mcp_servers`
    entry."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    (project_dir / ".codex").mkdir()
    (project_dir / ".codex" / "config.toml").write_text(
        "[mcp_servers.other-server]\ncommand = \"npx\"\n"
    )
    write_config(project_dir)
    runner.json("add", ref)

    out = runner.json("uninstall", "mcp", "grim-mcp")
    assert out["status"] in ("uninstalled", "removed"), out

    cfg = project_dir / ".codex" / "config.toml"
    assert cfg.is_file(), "config.toml must survive uninstall"
    doc = tomllib.loads(cfg.read_text())
    assert "grim-mcp" not in doc.get("mcp_servers", {}), "managed entry removed"
    assert doc["mcp_servers"]["other-server"]["command"] == "npx", "foreign entry preserved"


def test_global_copilot_registers_env_free_descriptors(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    # Copilot must be detected globally for the default target set.
    (runner.home / ".copilot" / "skills").mkdir(parents=True, exist_ok=True)
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global")["items"]
    assert rows[0]["status"] == "installed", rows

    copilot = json.loads((runner.home / ".copilot" / "mcp-config.json").read_text())
    assert copilot["mcpServers"]["grim-mcp"]["type"] == "local"
    assert copilot["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert copilot["mcpServers"]["grim-mcp"]["tools"] == ["*"]


def test_global_copilot_refuses_untracked_hand_inlined_workaround_then_force_replaces(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Regression guard for the 2026-09-27 upgrade: before Copilot's live
    `${VAR}` expansion was verified, the documented workaround was
    hand-inlining the secret value into `~/.copilot/mcp-config.json`
    instead of writing the reference. That hand-written entry has no grim
    install record, so `grim install --global` must refuse to clobber it
    (untracked-destination gate, exit 65) and leave it byte-untouched;
    `--force` replaces it with the `${VAR}` form grim now writes verbatim.
    Mirrors `test_mcp_install_refuses_untracked_member`
    (test_clobber_guard.py), extended to the global Copilot env-ref path."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(ENV_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    copilot_cfg = runner.home / ".copilot" / "mcp-config.json"
    copilot_cfg.parent.mkdir(parents=True, exist_ok=True)
    hand_written = {
        "mcpServers": {
            "grim-mcp": {
                "type": "local",
                "command": "grim",
                "args": ["mcp"],
                "tools": ["*"],
                "env": {"GRIM_TOKEN": "sk-hand-inlined-secret"},
            }
        }
    }
    copilot_cfg.write_text(json.dumps(hand_written, indent=2))

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")

    result = runner.run("install", "--global", "--client", "copilot", check=False)
    assert result.returncode == 65, (
        f"untracked Copilot MCP member clobber must exit 65, got {result.returncode}; {result.stderr}"
    )
    assert "--force" in result.stderr, f"refusal must hint --force; stderr: {result.stderr}"
    assert "copilot" in result.stderr, f"refusal must name the copilot client; stderr: {result.stderr}"
    entry = json.loads(copilot_cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["env"]["GRIM_TOKEN"] == "sk-hand-inlined-secret", (
        "refusal must leave the hand-written entry untouched"
    )

    rows = runner.json("install", "--global", "--client", "copilot", "--force")["items"]
    assert rows[0]["status"] == "installed", rows
    entry = json.loads(copilot_cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["env"]["GRIM_TOKEN"] == "${GITHUB_TOKEN}", (
        "--force must replace the hand-inlined workaround with the ${VAR} form, never the resolved value"
    )


def _oauth_descriptor(oauth: str) -> str:
    return (
        'description = "Server behind OAuth."\n\n'
        '[server]\ntransport = "http"\nurl = "https://example.com/mcp"\n\n'
        f"[server.oauth]\n{oauth}"
    )


# client, project MCP config path, container key, oauth block every field of
# which the client maps, the oauth object it must write, an oauth block it
# cannot map losslessly, and the field the skip warning must name
# (adr_mcp_oauth_projection.md, lossless-or-skip).
_OAUTH_CLIENTS = [
    (
        "opencode",
        "opencode.json",
        "mcp",
        'client_id = "${CID}"\nscopes = ["read", "write"]\ncallback_port = 43110\n',
        {"clientId": "{env:CID}", "scope": "read write", "callbackPort": 43110},
        'client_id = "c"\nauth_server_metadata_url = "https://auth.example.com/.well-known/x"\n',
        "auth_server_metadata_url",
    ),
    (
        "zed",
        ".zed/settings.json",
        "context_servers",
        'client_id = "grim-client"\n',
        {"client_id": "grim-client"},
        'client_id = "grim-client"\nscopes = ["read"]\n',
        "scopes",
    ),
]


@pytest.mark.parametrize(
    "client, config_rel, container_key, mapped, want, unmapped, unmapped_field", _OAUTH_CLIENTS
)
def test_oauth_server_is_written_losslessly_and_self_heals(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    client: str,
    config_rel: str,
    container_key: str,
    mapped: str,
    want: dict,
    unmapped: str,
    unmapped_field: str,
) -> None:
    """An oauth block whose every field the client maps is written onto its
    native oauth object; a repeat install is byte-stable and `grim status`
    reports it installed, never modified (Principle 9 self-heal)."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=_oauth_descriptor(mapped))
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    first = runner.json("install", "--client", client)["items"]
    assert first[0]["status"] == "installed", first
    cfg = project_dir / config_rel
    entry = json.loads(cfg.read_text())[container_key]["grim-mcp"]
    assert entry["oauth"] == want, entry
    assert "clientSecret" not in entry["oauth"] and "client_secret" not in entry["oauth"]

    before = cfg.read_text()
    second = runner.json("install", "--client", client)["items"]
    assert second[0]["status"] == "unchanged", second
    assert cfg.read_text() == before, f"{client} config must be byte-stable on repeat install"
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row


@pytest.mark.parametrize(
    "client, config_rel, container_key, mapped, want, unmapped, unmapped_field", _OAUTH_CLIENTS
)
def test_oauth_server_with_an_unmapped_field_is_skipped_and_named(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    client: str,
    config_rel: str,
    container_key: str,
    mapped: str,
    want: dict,
    unmapped: str,
    unmapped_field: str,
) -> None:
    """A field the client cannot carry skips the whole server — dropping a
    scope or a pinned metadata URL could widen the grant — and the warning
    names the field in the descriptor's own vocabulary."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=_oauth_descriptor(unmapped))
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    # Claude maps every field, so the install succeeds and only `client` skips.
    result = runner.run("install", "--client", f"claude,{client}", check=False)
    assert result.returncode == 0, result.stderr
    assert "grim-mcp" in json.loads((project_dir / ".mcp.json").read_text())["mcpServers"]
    assert f"skipped for {client}" in result.stderr, result.stderr
    assert unmapped_field in result.stderr, result.stderr
    cfg = project_dir / config_rel
    if cfg.exists():
        assert "grim-mcp" not in json.loads(cfg.read_text()).get(container_key, {})


@pytest.mark.parametrize(
    "client, config_rel, container_key, mapped, want, unmapped, unmapped_field", _OAUTH_CLIENTS
)
def test_oauth_server_newly_written_refuses_hand_authored_entry_then_force_replaces(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    client: str,
    config_rel: str,
    container_key: str,
    mapped: str,
    want: dict,
    unmapped: str,
    unmapped_field: str,
) -> None:
    """Upgrade guard: before the oauth projection, grim skipped this server
    for the client, so a user may have hand-authored a same-named entry.
    Now that grim writes it, the untracked entry is refused with exit 65 and
    left untouched; `--force` replaces it (upgrading.md)."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=_oauth_descriptor(mapped))
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    cfg = project_dir / config_rel
    cfg.parent.mkdir(parents=True, exist_ok=True)
    hand_written = {container_key: {"grim-mcp": {"url": "https://example.com/mcp", "hand": True}}}
    cfg.write_text(json.dumps(hand_written, indent=2))

    result = runner.run("install", "--client", client, check=False)
    assert result.returncode == 65, result.stderr
    assert "--force" in result.stderr, result.stderr
    assert json.loads(cfg.read_text()) == hand_written, "refusal must leave the hand-written entry untouched"

    rows = runner.json("install", "--client", client, "--force")["items"]
    assert rows[0]["status"] == "installed", rows
    entry = json.loads(cfg.read_text())[container_key]["grim-mcp"]
    assert entry["oauth"] == want and "hand" not in entry, entry


@pytest.mark.parametrize(
    "client, config_rel, container_key, mapped, want, unmapped, unmapped_field", _OAUTH_CLIENTS
)
def test_oauth_server_recorded_without_the_client_is_added_by_a_plain_install(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    client: str,
    config_rel: str,
    container_key: str,
    mapped: str,
    want: dict,
    unmapped: str,
    unmapped_field: str,
) -> None:
    """The upgrade path: an install record that covers only Claude — what a
    grim that skipped oauth for `client` left behind — is completed by a
    plain `grim install` (no `--client`, no `--force`) once the client is
    configured, which writes the oauth entry,
    leaves Claude's untouched, and reads back not-modified."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=_oauth_descriptor(mapped))
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    first = runner.json("install", "--client", "claude")["items"]
    assert first[0]["status"] == "installed", first
    claude_cfg = project_dir / ".mcp.json"
    claude_before = claude_cfg.read_text()
    cfg = project_dir / config_rel
    assert not cfg.exists() or "grim-mcp" not in json.loads(cfg.read_text()).get(container_key, {})

    runner.run("config", "set", "options.clients", f"claude,{client}")
    upgraded = runner.run("install", check=False)
    assert upgraded.returncode == 0, upgraded.stderr
    assert json.loads(cfg.read_text())[container_key]["grim-mcp"]["oauth"] == want
    assert claude_cfg.read_text() == claude_before, "claude entry must be untouched"

    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    assert row["outputs_pending"] == [], row


# ── Copilot project scope: VS Code Chat's file and the CLI's file ──────────

_VSCODE_MCP = Path(".vscode") / "mcp.json"
_GITHUB_MCP = Path(".github") / "mcp.json"


def _copilot_project(project_dir: Path) -> None:
    (project_dir / ".github").mkdir(exist_ok=True)
    (project_dir / ".github" / "copilot-instructions.md").write_text("# ci\n")


def _copilot_outputs(row: dict, field: str = "outputs") -> set[str]:
    return {Path(o["path"]).as_posix() for o in row[field] if o["client"] == "copilot"}


def test_project_copilot_writes_both_mcp_files_and_uninstall_removes_both(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Copilot CLI never reads `.vscode/mcp.json`, so a project install also
    writes `.github/mcp.json` (`mcpServers`, `${VAR}` verbatim — the CLI
    expands it). Status lists both outputs, a repeat install is byte-stable,
    and uninstall removes both entries but neither file."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=ENV_DESCRIPTOR)
    _copilot_project(project_dir)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    rows = runner.json("install", "--client", "copilot")["items"]
    assert rows[0]["status"] == "installed", rows

    vscode = json.loads((project_dir / _VSCODE_MCP).read_text())["servers"]["grim-mcp"]
    assert vscode["type"] == "stdio"
    assert vscode["env"]["GRIM_TOKEN"] == "${env:GITHUB_TOKEN}"
    cli = json.loads((project_dir / _GITHUB_MCP).read_text())["mcpServers"]["grim-mcp"]
    assert cli["type"] == "local"
    assert cli["command"] == "grim"
    assert cli["env"]["GRIM_TOKEN"] == "${GITHUB_TOKEN}", "reference written verbatim"
    assert cli["tools"] == ["*"]

    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    assert _copilot_outputs(row, "outputs_pending") == set(), row
    outputs = _copilot_outputs(row)
    assert any(p.endswith(".vscode/mcp.json") for p in outputs), row
    assert any(p.endswith(".github/mcp.json") for p in outputs), row

    before = {p: (project_dir / p).read_bytes() for p in (_VSCODE_MCP, _GITHUB_MCP)}
    again = runner.json("install", "--client", "copilot")["items"]
    assert again[0]["status"] == "unchanged", again
    for p, data in before.items():
        assert (project_dir / p).read_bytes() == data, f"{p} must be byte-stable on repeat install"

    out = runner.json("uninstall", "mcp", "grim-mcp")
    assert out["status"] in ("uninstalled", "removed"), out
    for p, container in ((_VSCODE_MCP, "servers"), (_GITHUB_MCP, "mcpServers")):
        cfg = project_dir / p
        assert cfg.is_file(), f"{p} itself must survive"
        assert "grim-mcp" not in json.loads(cfg.read_text()).get(container, {}), p



@pytest.mark.skipif(sys.platform == "win32", reason="file symlinks need a privilege on Windows")
def test_project_copilot_mcp_files_aliased_by_a_symlink_compose(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A workspace that links `.github/mcp.json` to `.vscode/mcp.json` gives
    Copilot's two registrations one physical file. The second splice must
    land on top of the first, not on bytes read before it was written, so
    both members survive and a repeat install is a byte-stable no-op."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=ENV_DESCRIPTOR)
    _copilot_project(project_dir)
    (project_dir / ".vscode").mkdir(exist_ok=True)
    (project_dir / _VSCODE_MCP).write_text("{}\n")
    (project_dir / _GITHUB_MCP).symlink_to(Path("..") / _VSCODE_MCP)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    runner.json("install", "--client", "copilot")

    doc = json.loads((project_dir / _VSCODE_MCP).read_text())
    assert "grim-mcp" in doc["servers"], doc
    assert "grim-mcp" in doc["mcpServers"], doc
    assert (project_dir / _GITHUB_MCP).is_symlink(), "the alias survives the write"

    before = (project_dir / _VSCODE_MCP).read_bytes()
    again = runner.json("install", "--client", "copilot")["items"]
    assert again[0]["status"] == "unchanged", again
    assert (project_dir / _VSCODE_MCP).read_bytes() == before

def test_project_copilot_record_without_cli_file_heals_on_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A Copilot project install recorded before grim wrote `.github/mcp.json`
    covers only `.vscode/mcp.json`. Status reports the CLI file pending; the
    next install writes it without touching the VS Code entry, and after
    that nothing is pending."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    _copilot_project(project_dir)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    runner.json("install", "--client", "copilot")

    # Rewind to the pre-change shape: drop the CLI file and its record output.
    (project_dir / _GITHUB_MCP).unlink()
    state_path = project_dir / ".grimoire" / "state.json"
    state = json.loads(state_path.read_text())
    record = next(r for r in state["records"] if r["name"] == "grim-mcp")
    record["outputs"] = [o for o in record["outputs"] if not o["target"]["relative"].endswith(".github/mcp.json")]
    assert len(record["outputs"]) == 1, record
    state_path.write_text(json.dumps(state))
    vscode_before = (project_dir / _VSCODE_MCP).read_bytes()

    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    pending = _copilot_outputs(row, "outputs_pending")
    assert len(pending) == 1 and pending.pop().endswith(".github/mcp.json"), row

    runner.json("install", "--client", "copilot")
    assert (project_dir / _VSCODE_MCP).read_bytes() == vscode_before, "the VS Code entry is untouched"
    assert json.loads((project_dir / _GITHUB_MCP).read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    assert _copilot_outputs(row, "outputs_pending") == set(), row
    assert len(_copilot_outputs(row)) == 2, row


_OAUTH_DESCRIPTOR = """\
description = "Remote server behind OAuth."

[server]
transport = "http"
url = "https://mcp.example.com/mcp"

[server.oauth]
"""


@pytest.mark.parametrize(
    "oauth, written",
    [
        ('client_id = "grim-client"\n', True),
        ('client_id = "grim-client"\nscopes = ["read"]\n', False),
        ('client_id = "grim-client"\ncallback_port = 8080\n', False),
    ],
    ids=["client-id-only", "scopes", "callback-port"],
)
def test_project_copilot_oauth_client_id_written_or_skipped(
    grim_at, project_dir: Path, registry: str, unique_repo: str, oauth: str, written: bool
) -> None:
    """Lossless-or-skip: a client id alone becomes VS Code's `oauth.clientId`
    and the CLI's `oauthClientId`; any other oauth field skips Copilot with a
    warning naming it, while Claude (full mapping) still registers."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=_OAUTH_DESCRIPTOR + oauth)
    _copilot_project(project_dir)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    result = runner.run("install", "--client", "claude", "--client", "copilot", check=False)
    assert result.returncode == 0, result.stderr
    assert "grim-mcp" in json.loads((project_dir / ".mcp.json").read_text())["mcpServers"]

    if written:
        vscode = json.loads((project_dir / _VSCODE_MCP).read_text())["servers"]["grim-mcp"]
        assert vscode["oauth"] == {"clientId": "grim-client"}
        cli = json.loads((project_dir / _GITHUB_MCP).read_text())["mcpServers"]["grim-mcp"]
        assert cli["oauthClientId"] == "grim-client"
        assert cli["type"] == "http"
    else:
        for p in (_VSCODE_MCP, _GITHUB_MCP):
            assert not (project_dir / p).exists(), f"{p} must not be written for a lossy oauth block"
        unmapped = "scopes" if "scopes" in oauth else "callback_port"
        assert unmapped in result.stderr, result.stderr


def test_update_copilot_one_file_declines_new_pin_and_its_stale_entry_goes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A pin that only one Copilot file can take: the CLI rejects a url with a
    `${VAR}` in the port, VS Code does not. The update re-splices
    `.vscode/mcp.json` and removes the old `.github/mcp.json` member instead of
    stranding it outside every record."""
    runner = grim_at(project_dir)
    descriptor = project_dir / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    repo_path = f"{unique_repo}/mcp/grim-mcp"
    repo = f"{registry}/{repo_path}"
    body = 'description = "Remote."\n\n[server]\ntransport = "http"\nurl = "{url}"\n'

    descriptor.write_text(body.format(url="http://mcp.example.com:8080/mcp"))
    runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    runner.json("release", str(descriptor), f"{repo}:stable", "--kind", "mcp")
    _copilot_project(project_dir)
    (project_dir / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{repo}:stable"\n')
    runner.run("lock", check=False)
    runner.json("install", "--client", "copilot")
    assert "grim-mcp" in json.loads((project_dir / _GITHUB_MCP).read_text())["mcpServers"]

    descriptor.write_text(body.format(url="http://mcp.example.com:${PORT}/mcp"))
    second = runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")
    retag(repo_path, "stable", second["manifest_digest"])
    result = runner.run("update", "--client", "copilot", check=False)
    assert result.returncode == 0, result.stderr
    assert ".github/mcp.json" in result.stderr, result.stderr

    vscode = json.loads((project_dir / _VSCODE_MCP).read_text())["servers"]["grim-mcp"]
    assert vscode["url"] == "http://mcp.example.com:${env:PORT}/mcp"
    assert "grim-mcp" not in json.loads((project_dir / _GITHUB_MCP).read_text()).get("mcpServers", {}), (
        "the stale CLI entry must not outlive its record"
    )
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    outputs = _copilot_outputs(row)
    assert len(outputs) == 1 and outputs.pop().endswith(".vscode/mcp.json"), row
    # Pinned: plain `grim status` has no local copy of a registry descriptor
    # (no manifest cache), so the declined file still reads pending there.
    # Follow-up: cache manifests so status can ask the renderer too.
    pending = _copilot_outputs(row, "outputs_pending")
    assert len(pending) == 1 and pending.pop().endswith(".github/mcp.json"), row
    # The install gate does ask it: a repeat install is a byte-stable no-op.
    before = {p: (project_dir / p).read_bytes() for p in (_VSCODE_MCP, _GITHUB_MCP)}
    again = runner.json("install", "--client", "copilot")["items"]
    assert again[0]["status"] == "unchanged", again
    for p, data in before.items():
        assert (project_dir / p).read_bytes() == data, p



def test_project_copilot_port_env_url_reinstalls_unchanged(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A `${PORT}` url is valid for VS Code but not for the Copilot CLI (it
    validates before expanding), so
    `.github/mcp.json` is skipped on every pass. The skip must not make each
    later install re-run the MCP pass and report `updated`."""
    runner = grim_at(project_dir)
    body = 'description = "Remote."\n\n[server]\ntransport = "http"\nurl = "http://mcp.example.com:${PORT}/mcp"\n'
    ref = _release(runner, project_dir, registry, unique_repo, body=body)
    _copilot_project(project_dir)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    first = runner.run("install", "--client", "copilot", check=False)
    assert first.returncode == 0, first.stderr
    assert "grim-mcp" in json.loads((project_dir / _VSCODE_MCP).read_text())["servers"]
    cli = project_dir / _GITHUB_MCP
    assert not cli.exists() or "grim-mcp" not in json.loads(cli.read_text()).get("mcpServers", {})

    before = (project_dir / _VSCODE_MCP).read_bytes()
    again = runner.json("install", "--client", "copilot")["items"]
    assert again[0]["status"] == "unchanged", again
    assert (project_dir / _VSCODE_MCP).read_bytes() == before

# ── Gemini: `timeout` is not projected (grimoire-rs/grimoire#146) ─────────


def _entry_hash(value: dict) -> str:
    """grim's semantic hash of a managed MCP member: sha256 over the compact
    JSON of the value, keys sorted (``install_state::entry_value_hash``)."""
    canonical = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return "sha256:" + hashlib.sha256(canonical.encode()).hexdigest()


def _rehash_recorded_entry(state_path: Path, client: str, value: dict) -> None:
    """Point the recorded hash of ``client``'s MCP entry at ``value`` — what
    an older grim that rendered ``value`` would have recorded."""
    state = json.loads(state_path.read_text())
    hits = 0

    def walk(node: object) -> None:
        nonlocal hits
        if isinstance(node, dict):
            if node.get("client") == client and node.get("entry"):
                node["content_hash"] = _entry_hash(value)
                hits += 1
            for child in node.values():
                walk(child)
        elif isinstance(node, list):
            for child in node:
                walk(child)

    walk(state)
    assert hits == 1, f"expected one {client} MCP output in {state_path}"
    state_path.write_text(json.dumps(state))


def test_gemini_drops_timeout_with_warning_and_heals_an_old_render(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Gemini applies `timeout` to every tool call, so grim no longer writes
    it and warns instead. An entry an older grim rendered with `timeout`
    stays `installed` (the record matches it), keeps its bytes on a
    same-pin install, and is rewritten without the key on the next pin
    change; a repeat install is then a no-op that `status` reports
    unmodified. A hand-added `timeout` reads `modified` and is refused."""
    runner = grim_at(project_dir)
    descriptor = project_dir / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    repo_path = f"{unique_repo}/mcp/grim-mcp"
    repo = f"{registry}/{repo_path}"
    descriptor.write_text(DESCRIPTOR + "timeout = 7000\n")
    runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    runner.json("release", str(descriptor), f"{repo}:stable", "--kind", "mcp")
    (project_dir / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{repo}:stable"\n')
    runner.run("lock", check=False)

    first = runner.run("install", "--client", "gemini", check=False)
    assert first.returncode == 0, first.stderr
    assert "timeout dropped for gemini" in first.stderr, first.stderr
    cfg = project_dir / ".gemini" / "settings.json"
    entry = json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    assert "timeout" not in entry, entry

    # What an older grim wrote and recorded: the same entry plus `timeout`.
    old = {**entry, "timeout": 7000}
    cfg.write_text(json.dumps({"mcpServers": {"grim-mcp": old}}, indent=2))
    _rehash_recorded_entry(project_dir / ".grimoire" / "state.json", "gemini", old)
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", f"the simulated old render must match its record: {row}"
    for extra in ((), ("--force",)):
        runner.json("install", "--client", "gemini", *extra)
        assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]["timeout"] == 7000, (
            f"a same-pin install {extra} keeps the intact old bytes (the integrity gate trusts the record)"
        )

    descriptor.write_text(DESCRIPTOR.replace("grim as an MCP server", "grim over MCP") + "timeout = 7000\n")
    second = runner.json("release", str(descriptor), f"{repo}:1.0.1", "--kind", "mcp")
    retag(repo_path, "stable", second["manifest_digest"])
    runner.json("update", "--client", "gemini")
    healed = json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    assert "timeout" not in healed, f"the pin change must re-render without timeout: {healed}"

    before = cfg.read_bytes()
    again = runner.json("install", "--client", "gemini")["items"]
    assert all(r["status"] == "unchanged" for r in again), again
    assert cfg.read_bytes() == before, "regeneration must be byte-identical"
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row

    # A timeout the user adds back by hand is theirs: modified, never clobbered.
    cfg.write_text(json.dumps({"mcpServers": {"grim-mcp": {**healed, "timeout": 7000}}}))
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "modified", row
    refused = runner.run("install", "--client", "gemini", check=False)
    assert refused.returncode == 65, refused.stderr
    assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]["timeout"] == 7000

    # The documented immediate remedy: delete the entry, then install.
    cfg.write_text(json.dumps({"mcpServers": {}}))
    runner.json("install", "--client", "gemini")
    assert "timeout" not in json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row


# ── Warp: `.warp/.mcp.json` (grimoire-rs/grimoire#155) ───────────────────


def test_warp_project_registers_in_dot_warp_mcp_json_and_is_idempotent(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A project install writes Warp's `.warp/.mcp.json` under
    `mcpServers`, mapping `cwd` to `working_directory`; a repeat install is a
    byte-identical no-op and `status` reports it installed with nothing
    pending. Warp is the only detected client here."""
    project_dir = bare_project_dir
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=DESCRIPTOR + 'cwd = "./srv"\n')
    (project_dir / ".warp").mkdir()
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    first = runner.json("install", "--client", "warp")["items"]
    assert first[0]["status"] == "installed", first
    cfg = project_dir / ".warp" / ".mcp.json"
    assert json.loads(cfg.read_text()) == {
        "mcpServers": {"grim-mcp": {"command": "grim", "args": ["mcp"], "working_directory": "./srv"}}
    }

    before = cfg.read_bytes()
    second = runner.json("install", "--client", "warp")["items"]
    assert second[0]["status"] == "unchanged", second
    assert cfg.read_bytes() == before, "warp config must be byte-stable on repeat install"
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    assert row["outputs_pending"] == [], row


def test_warp_global_registers_in_home_dot_warp_mcp_json(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """A global install writes `~/.warp/.mcp.json`, the same path on every OS."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n[server]\ntransport = "http"\nurl = "https://mcp.example.com/mcp"\n'
        'headers = { X-Client = "grim" }\n'
    )
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "warp")["items"]
    assert rows[0]["status"] == "installed", rows
    entry = json.loads((runner.home / ".warp" / ".mcp.json").read_text())["mcpServers"]["grim-mcp"]
    assert entry == {"url": "https://mcp.example.com/mcp", "headers": {"X-Client": "grim"}}, entry
    row = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row


@pytest.mark.parametrize(
    "body, reason",
    [
        (ENV_DESCRIPTOR, "env-ref substitution is undocumented"),
        (
            'description = "d"\n[server]\ntransport = "http"\nurl = "https://x.example.com/mcp"\n'
            '[server.oauth]\nclient_id = "grim"\n',
            "no oauth field for client_id",
        ),
    ],
    ids=["env-ref", "oauth"],
)
def test_warp_skips_env_ref_and_oauth_descriptors_with_a_warning(
    grim_at, project_dir: Path, registry: str, unique_repo: str, body: str, reason: str
) -> None:
    """Warp documents neither `${VAR}` expansion in `.mcp.json` nor oauth
    config keys, so such a server is skipped for Warp with a warning while
    Claude (also detected) still registers it and the install succeeds."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo, body=body)
    (project_dir / ".warp").mkdir()
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    result = runner.run("install", check=False)
    assert result.returncode == 0, result.stderr
    assert "skipped for warp" in result.stderr and reason in result.stderr, result.stderr
    assert "grim-mcp" in json.loads((project_dir / ".mcp.json").read_text())["mcpServers"]
    cfg = project_dir / ".warp" / ".mcp.json"
    assert not cfg.exists() or "grim-mcp" not in json.loads(cfg.read_text()).get("mcpServers", {})



def test_warp_global_env_ref_server_is_never_pending_and_reinstalls_unchanged(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Warp skips a `${VAR}` server on every pass, so the install gate must
    not count its surface as uncovered: otherwise every `grim install`
    re-runs the MCP pass and reports `updated` for a file it never writes."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor = _write_descriptor(tmp_path / "src", name="grim-mcp", body=ENV_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    (grim_home / "grimoire.toml").write_text(
        f'[options]\nclients = ["claude", "warp"]\n\n[mcp]\ngrim-mcp = "{ref}"\n'
    )
    runner.json("lock", "--global")

    first = runner.run("install", "--global", check=False)
    assert first.returncode == 0, first.stderr
    assert "skipped for warp" in first.stderr, first.stderr
    claude_cfg = runner.home / ".claude.json"
    before = claude_cfg.read_bytes()
    again = runner.json("install", "--global")["items"]
    assert again[0]["status"] == "unchanged", again
    assert claude_cfg.read_bytes() == before
    assert not (runner.home / ".warp" / ".mcp.json").exists()
    # Pinned: plain `grim status` has no local copy of a registry descriptor
    # (no manifest cache), so it still lists Warp. Follow-up: cache manifests
    # so status can ask the renderer too.
    row = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert [o["client"] for o in row["outputs_pending"]] == ["warp"], row

def test_warp_refuses_untracked_hand_added_entry_then_force_replaces(
    grim_at, bare_project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A same-named server the user added to `.warp/.mcp.json` by hand has no
    grim record, so `grim install` refuses to clobber it (exit 65) and leaves
    it byte-untouched; `--force` replaces it with grim's entry. Mirrors
    `test_global_copilot_refuses_untracked_hand_inlined_workaround_then_force_replaces`."""
    project_dir = bare_project_dir
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)
    cfg = project_dir / ".warp" / ".mcp.json"
    cfg.parent.mkdir(parents=True)
    hand_written = json.dumps({"mcpServers": {"grim-mcp": {"command": "npx", "args": ["grim-by-hand"]}}}, indent=2)
    cfg.write_text(hand_written)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    result = runner.run("install", "--client", "warp", check=False)
    assert result.returncode == 65, result.stderr
    assert "--force" in result.stderr and "warp" in result.stderr, result.stderr
    assert cfg.read_text() == hand_written, "refusal must leave the hand-written entry untouched"

    rows = runner.json("install", "--client", "warp", "--force")["items"]
    assert rows[0]["status"] == "installed", rows
    assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"] == {"command": "grim", "args": ["mcp"]}
    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
