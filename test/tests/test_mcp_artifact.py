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
