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

import json
import tomllib  # stdlib (Python 3.11+)
from pathlib import Path

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
    (project_dir / ".claude").mkdir()
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
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Project-scope sibling of the global regression
    (test_global_status_reports_installed_for_claude_mcp_without_claude_dir):
    ``.mcp.json`` (Claude's project MCP config) is a SIBLING of ``.claude/``,
    not something inside it — ``Vendor::detect`` checks only ``.claude/``."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir, registry, unique_repo)

    # CRITICAL repro condition: another vendor IS detected (.codex/) so
    # detect_clients returns a non-empty set that excludes claude. No
    # .claude/ dir is ever created.
    (project_dir / ".codex").mkdir()
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    rows = runner.json("install", "--client", "claude")["items"]
    assert rows[0]["status"] == "installed", rows

    # Sanity: write side is correct — fails only on the read side below.
    claude_json = project_dir / ".mcp.json"
    assert claude_json.is_file()
    assert json.loads(claude_json.read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert not (project_dir / ".claude").exists(), "install must not create .claude itself"

    state_text = (project_dir / ".grimoire" / "state.json").read_text()
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
    (project_dir / ".claude").mkdir()  # detect Claude only
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
    (project_dir / ".claude").mkdir()
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


def test_global_copilot_skips_env_ref_descriptors(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Copilot CLI's global config supports no variable substitution: a
    descriptor with `${VAR}` refs registers for Claude/OpenCode but skips
    Copilot with a warning — no secrets (or broken literals) on disk."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(ENV_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    result = runner.run("install", "--global", check=False)
    assert result.returncode == 0, result.stderr
    assert "copilot" in result.stderr and "substitution" in result.stderr, (
        f"the Copilot skip must be announced: {result.stderr}"
    )

    assert not (runner.home / ".copilot" / "mcp-config.json").exists(), (
        "no Copilot config may be written for an env-ref descriptor"
    )
    claude = json.loads((runner.home / ".claude.json").read_text())
    assert claude["mcpServers"]["grim-mcp"]["env"]["GRIM_TOKEN"] == "${GITHUB_TOKEN}"
    opencode = json.loads((runner.home / ".config" / "opencode" / "opencode.json").read_text())
    assert opencode["mcp"]["grim-mcp"]["environment"]["GRIM_TOKEN"] == "{env:GITHUB_TOKEN}"


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
