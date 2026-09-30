# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Copilot's hook response dialect, as the 2026-09-28 live probes decided it.

Both facts were settled by running `copilot` 1.0.88 with a PascalCase hook in
an isolated `COPILOT_HOME` (`research_hooks_qoder_copilot_schemas.md` § 6):

- **S-107 / C-130 / C-132.** A `PreToolUse` rewrite applies through
  `hookSpecificOutput.updatedInput` — Copilot ran the rewritten command and
  not the original — so that is the one field grim emits; the top-level
  `modifiedArgs` spelling never appears.
- **S-108 / C-131.** SessionStart context reaches the session only as the
  flat top-level `additionalContext`; the nested Claude form did not surface.

Asserted through `grim hook run` as a process, with a dispatch table written
where grim writes one: what matters is the document Copilot reads on stdout.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the payload fixtures are POSIX shell scripts (v1 launcher scope)"
)

ROOT = "0123456789abcdef0123456789abcdef"

PRE_TOOL_USE = json.dumps(
    {
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/repo/a.txt", "content": "x"},
        "cwd": "/repo",
        "session_id": "s-1",
    }
)

SESSION_START = json.dumps({"hook_event_name": "SessionStart", "cwd": "/repo", "session_id": "s-1"})


def _arm_table(grim_home: Path, tmp_path: Path, *, client: str, event: str, tier: str, matcher: str | None, response: str) -> Path:
    """A one-row dispatch table whose payload answers `response`, plus the
    marker file that proves the payload ran."""
    payload_dir = tmp_path / "payload"
    payload_dir.mkdir()
    (payload_dir / "guard.sh").write_text(f"#!/bin/sh\ncat > '{tmp_path / 'ran'}'\nprintf '%s' '{response}'\n")
    entry = {
        "artifact": "g",
        "id": "h",
        "event": event,
        "tier": tier,
        "matcher": matcher,
        "handler": {"argv": ["sh", str(payload_dir / "guard.sh")]},
        "timeout": None,
        "payload": "stdin",
        "payload_dir": str(payload_dir),
        "resolved_digest": "sha256:" + "ab" * 32,
        "client": client,
    }
    table = grim_home / "hooks" / "dispatch.json"
    table.parent.mkdir(parents=True, exist_ok=True)
    table.write_text(json.dumps({"schema": 1, "roots": {ROOT: {"root": "global", "hooks": [entry]}}}))
    return table


def _run(grim: GrimRunner, table: Path, *, client: str, event: str, stdin: str):
    return grim.run(
        "hook", "run", "--client", client, "--event", event, "--table", str(table), "--root", ROOT,
        stdin=stdin, check=False,
    )


def test_s107_a_copilot_rewrite_uses_only_the_probe_selected_field(grim, grim_home: Path, tmp_path: Path) -> None:
    """**S-107 / C-132.** The rewrite lands in `hookSpecificOutput.updatedInput`
    and nowhere else — never a second, top-level `modifiedArgs` copy."""
    rewrite = '{"decision":"allow","updated_input":{"file_path":"/repo/b.txt","content":"x"}}'
    table = _arm_table(grim_home, tmp_path, client="copilot", event="PreToolUse", tier="mutator", matcher="Write", response=rewrite)

    result = _run(grim, table, client="copilot", event="PreToolUse", stdin=PRE_TOOL_USE)

    assert (tmp_path / "ran").exists(), f"the mutator never ran\n{result.stderr}"
    assert result.returncode == 0, result.stderr
    document = json.loads(result.stdout)
    assert document["hookSpecificOutput"]["updatedInput"] == {"file_path": "/repo/b.txt", "content": "x"}, document
    assert "modifiedArgs" not in result.stdout, document


def test_s108_copilot_session_start_never_gets_the_unprobed_nested_form(
    grim, grim_home: Path, tmp_path: Path
) -> None:
    """**S-108 / C-131.** The copilot SessionStart row's context target is the
    flat `additionalContext` the probe saw reach the session (pinned at unit
    level by `c131_copilot_session_start_context_is_the_probed_flat_field`).

    At runtime only an `observer` is valid at SessionStart, and an observer
    contributes no context (`pipeline::assemble`), so the hook runs and nothing
    is injected — and in particular never the nested Claude form the probe
    showed Copilot ignores.
    """
    table = _arm_table(
        grim_home, tmp_path, client="copilot", event="SessionStart", tier="observer", matcher=None,
        response='{"decision":"none","context":"repo uses tabs"}',
    )

    result = _run(grim, table, client="copilot", event="SessionStart", stdin=SESSION_START)

    assert (tmp_path / "ran").exists(), f"POSITIVE CONTROL: the observer never ran\n{result.stderr}"
    assert result.returncode == 0, result.stderr
    assert "hookSpecificOutput" not in result.stdout, result.stdout
    assert "repo uses tabs" not in result.stdout, "an observer must not inject context"
