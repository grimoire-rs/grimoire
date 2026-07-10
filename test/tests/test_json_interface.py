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


def test_error_document_on_missing_config(
    grim_at, project_dir: Path
) -> None:
    """A failing run under --format json emits the structured error
    document on stdout; the human chain stays on stderr."""
    runner = grim_at(project_dir)
    missing = project_dir / "no-such-grimoire.toml"

    result = runner.run(
        "--format", "json", "--config", str(missing), "status", check=False
    )
    assert result.returncode == 79, result.stderr
    doc = json.loads(result.stdout)
    assert set(doc) == {"error"}, f"top-level error key marks the doc: {doc}"
    assert doc["error"]["code"] == "not-found"
    assert doc["error"]["exit"] == 79
    assert doc["error"]["message"], "message carries the rendered chain"
    assert result.stderr.strip(), "human-readable chain still on stderr"


def test_error_document_on_usage_error(
    grim_at, project_dir: Path
) -> None:
    """A second exit-code class maps to its slug (unknown config key → 64)."""
    from src.helpers import write_config

    write_config(project_dir)
    runner = grim_at(project_dir)

    result = runner.run(
        "--format", "json", "config", "get", "optins.clients", check=False
    )
    assert result.returncode == 64, result.stderr
    doc = json.loads(result.stdout)
    assert doc["error"]["code"] == "usage"
    assert doc["error"]["exit"] == 64


def test_plain_mode_failure_keeps_stdout_empty(
    grim_at, project_dir: Path
) -> None:
    """Without --format json, a failure writes nothing to stdout."""
    runner = grim_at(project_dir)
    missing = project_dir / "no-such-grimoire.toml"

    result = runner.plain("--config", str(missing), "status", check=False)
    assert result.returncode == 79, result.stderr
    assert result.stdout == "", f"plain failure must not write stdout: {result.stdout!r}"
    assert result.stderr.strip()


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
