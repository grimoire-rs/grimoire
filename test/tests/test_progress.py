# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`--progress json` acceptance tests — NDJSON events on stderr."""
from __future__ import annotations

import json
import uuid
from pathlib import Path

from src.helpers import make_artifact, write_config


def _declare_two(project_dir: Path, registry: str) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    s_repo = f"{ns}/skills/s"
    r_repo = f"{ns}/rules/r"
    make_artifact(s_repo, "skill", {"s/SKILL.md": "s\n"}, tag="stable")
    make_artifact(r_repo, "rule", {"r.md": "r\n"}, tag="stable")
    write_config(
        project_dir,
        skills={"s": f"{registry}/{s_repo}:stable"},
        rules={"r": f"{registry}/{r_repo}:stable"},
    )


def _events(stderr: str) -> list[dict]:
    lines = [ln for ln in stderr.splitlines() if ln.strip()]
    events = []
    for ln in lines:
        parsed = json.loads(ln)  # every stderr line must parse as JSON
        assert isinstance(parsed, dict), ln
        events.append(parsed)
    return events


def test_progress_json_on_install_emits_ndjson_stream(
    grim_at, project_dir: Path, registry: str
) -> None:
    _declare_two(project_dir, registry)
    runner = grim_at(project_dir)
    runner.run("lock")

    result = runner.run("--progress", "json", "install")
    events = _events(result.stderr)
    assert events[0] == {"event": "start", "total": 2}
    advances = [e for e in events if e["event"] == "advance"]
    assert len(advances) == 2, events
    assert all(e["total"] == 2 and "label" in e for e in advances)
    assert [e["position"] for e in advances] == [1, 2]
    assert events[-1] == {"event": "finish"}
    # stdout still carries exactly one report.
    assert result.stdout.splitlines()[0].startswith("Kind")


def test_progress_json_on_update_and_add(
    grim_at, project_dir: Path, registry: str
) -> None:
    _declare_two(project_dir, registry)
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")

    result = runner.run("--progress", "json", "update")
    events = _events(result.stderr)
    assert events[0]["event"] == "start"
    assert events[-1] == {"event": "finish"}

    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    extra = f"{ns}/skills/extra"
    make_artifact(extra, "skill", {"extra/SKILL.md": "e\n"}, tag="stable")
    result = runner.run("--progress", "json", "add", f"{registry}/{extra}:stable")
    events = _events(result.stderr)
    assert {"event": "start", "total": 1} in events
    assert events[-1] == {"event": "finish"}


def test_progress_none_keeps_stderr_empty(
    grim_at, project_dir: Path, registry: str
) -> None:
    _declare_two(project_dir, registry)
    runner = grim_at(project_dir)
    runner.run("lock")
    result = runner.run("--progress", "none", "install")
    assert result.stderr.strip() == "", result.stderr
