# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""JSON interface contract tests — the 1.0 machine-readable surface.

Locks the cross-command invariants documented in
``docs/src/json-interface.md``: the uniform ``{"items": [...]}`` envelope
for multi-item reports and (later) the structured error document.
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact, write_config


def test_list_reports_use_items_envelope(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Multi-item reports are `{"items": [...]}` objects, never bare arrays."""
    repo = f"{unique_repo}/s"
    make_artifact(repo, "skill", {"s/SKILL.md": "v\n"}, tag="stable")
    write_config(project_dir, skills={"s": f"{registry}/{repo}:stable"})
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "status", check=False)
    assert result.returncode == 0
    doc = json.loads(result.stdout)
    assert isinstance(doc, dict), "top-level JSON must be an object envelope"
    assert isinstance(doc["items"], list)
    assert doc["items"][0]["name"] == "s"
