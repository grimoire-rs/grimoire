# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim fetch` acceptance tests — use != install, payload-plain stdout."""
from __future__ import annotations

import json
import uuid
from pathlib import Path

from src.helpers import make_artifact, make_bundle


SKILL_DOC = (
    "---\n"
    "name: fetch-demo\n"
    "description: Demo skill for fetch tests.\n"
    "---\n"
    "# Fetch Demo\n\nBody text.\n"
)
SUPPORT_DOC = "support file body\n"


def _publish_skill(registry: str) -> str:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/fetch-demo"
    make_artifact(
        repo,
        "skill",
        {
            "fetch-demo/SKILL.md": SKILL_DOC,
            "fetch-demo/references/notes.md": SUPPORT_DOC,
        },
        tag="latest",
    )
    return f"{registry}/{repo}:latest"


def test_fetch_plain_stdout_byte_equals_published_index(
    grim_at, project_dir: Path, registry: str
) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    result = runner.plain("fetch", ref)
    assert result.returncode == 0, result.stderr
    # Exact bytes: no table, no added trailing newline.
    assert result.stdout == SKILL_DOC


def test_fetch_path_returns_exact_support_file(
    grim_at, project_dir: Path, registry: str
) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    result = runner.plain("fetch", ref, "--path", "fetch-demo/references/notes.md")
    assert result.returncode == 0, result.stderr
    assert result.stdout == SUPPORT_DOC


def test_fetch_json_is_full_report(grim_at, project_dir: Path, registry: str) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    doc = runner.json("fetch", ref)
    assert doc["kind"] == "skill"
    assert doc["name"] == "fetch-demo"
    assert doc["vendor"] == "canonical"
    assert doc["content"] == SKILL_DOC
    assert doc["digest"].startswith("sha256:")
    paths = [f["path"] for f in doc["files"]]
    assert "fetch-demo/SKILL.md" in paths
    assert "fetch-demo/references/notes.md" in paths


def test_fetch_vendor_claude_projection(grim_at, project_dir: Path, registry: str) -> None:
    ref = _publish_skill(registry)
    runner = grim_at(project_dir)
    doc = runner.json("fetch", ref, "--vendor", "claude")
    assert doc["vendor"] == "claude"
    # A plain skill without tool-namespaced metadata projects verbatim.
    assert doc["content"] == SKILL_DOC


def test_fetch_bundle_vendor_and_unknown_vendor_error(
    grim_at, project_dir: Path, registry: str
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/bundles/stack"
    make_bundle(repo, [], tag="latest")
    runner = grim_at(project_dir)

    result = runner.plain("fetch", f"{registry}/{repo}:latest", "--vendor", "claude", check=False)
    assert result.returncode != 0
    assert "vendor projection" in result.stderr

    ref = _publish_skill(registry)
    result = runner.plain("fetch", ref, "--vendor", "nonesuch", check=False)
    assert result.returncode != 0


def test_fetch_large_content_is_not_truncated(
    grim_at, project_dir: Path, registry: str
) -> None:
    """Pins the cap decision: the CLI never truncates below the 8 MiB
    layer gate — content past the MCP 256 KiB doc cap prints complete."""
    big_body = "x" * (300 * 1024)  # > 256 KiB MCP cap, < 8 MiB gate
    doc = f"---\nname: big\ndescription: d.\n---\n{big_body}\n"
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    repo = f"{ns}/skills/big"
    make_artifact(repo, "skill", {"big/SKILL.md": doc}, tag="latest")
    runner = grim_at(project_dir)

    result = runner.plain("fetch", f"{registry}/{repo}:latest")
    assert result.returncode == 0, result.stderr
    assert result.stdout == doc, "no truncation, no marker"
    assert "truncated" not in result.stdout

    parsed = json.loads(runner.run("fetch", f"{registry}/{repo}:latest", format="json").stdout)
    assert parsed.get("truncated") is not True
