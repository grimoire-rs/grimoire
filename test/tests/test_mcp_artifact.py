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
from pathlib import Path

from src.helpers import write_config
from src.registry import fetch_blob, fetch_manifest

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
    rows = runner.json("install")
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

    status = runner.json("status")
    row = next(r for r in status if r["name"] == "grim-mcp")
    assert row["kind"] == "mcp"
    assert row["state"] == "installed"


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
    status = runner.json("status")
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "installed"

    # A real value change: modified + refused without --force.
    doc = json.loads(cfg.read_text())
    doc["mcpServers"]["grim-mcp"]["command"] = "evil"
    cfg.write_text(json.dumps(doc))
    status = runner.json("status")
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
    status = runner.json("status")
    assert next(r for r in status if r["name"] == "grim-mcp")["state"] == "missing"


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
    rows = runner.json("install", "--global")
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
    rows = runner.json("install", "--global")
    assert rows[0]["status"] == "installed", rows

    copilot = json.loads((runner.home / ".copilot" / "mcp-config.json").read_text())
    assert copilot["mcpServers"]["grim-mcp"]["type"] == "local"
    assert copilot["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert copilot["mcpServers"]["grim-mcp"]["tools"] == ["*"]
