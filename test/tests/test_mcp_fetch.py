# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Acceptance tests for the ``grim_fetch`` MCP tool.

``grim_fetch`` returns artifact content in the tool result — no install, no
state, no harness reload (use ≠ install; see
``adr_mcp_percall_scope_fetch_render.md``). These tests drive the STDIO
server against the real registry fixture and assert canonical bytes, vendor
projections, support-file fetches, truncation, and clean error shapes.
"""
from __future__ import annotations

import json
import subprocess
import uuid
from collections.abc import Callable
from pathlib import Path

from src.helpers import make_artifact
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner

_PROTOCOL = "2025-06-18"

# Mirrors FETCH_DOC_SIZE_LIMIT in src/mcp/fetch.rs.
_DOC_CAP = 256 * 1024


def _drive(
    runner: GrimRunner,
    cwd: Path,
    requests: list[dict],
    *,
    offline: bool = False,
    timeout: int = 60,
) -> dict[int, dict]:
    """Run ``grim mcp`` feeding ``requests``, return responses by id.

    Fetch tests default to *online* (``grim_fetch`` requires network); pass
    ``offline=True`` only to assert the clean offline error.
    """
    args = [str(runner.binary)]
    if offline:
        args.append("--offline")
    args.append("mcp")
    payload = "".join(json.dumps(r) + "\n" for r in requests)
    result = subprocess.run(
        args,
        input=payload,
        capture_output=True,
        text=True,
        env=runner.env,
        cwd=str(cwd),
        timeout=timeout,
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


def _call_fetch(
    runner: GrimRunner, cwd: Path, arguments: dict, *, offline: bool = False
) -> dict:
    """Drive one ``grim_fetch`` tool call, return the raw tool result."""
    responses = _drive(
        runner,
        cwd,
        [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": _PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "pytest", "version": "0"},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "grim_fetch", "arguments": arguments},
            },
        ],
        offline=offline,
    )
    assert 2 in responses, (
        f"grim mcp did not answer the tools/call request; got ids {sorted(responses)}"
    )
    msg = responses[2]
    assert "result" in msg, f"JSON-RPC error instead of tool result: {msg!r}"
    return msg["result"]


def _payload(call: dict) -> dict:
    assert call["isError"] is False, f"grim_fetch must not error, got: {call!r}"
    return json.loads(call["content"][0]["text"])


def test_fetch_canonical_skill_matches_authored_bytes(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    """Canonical fetch returns the authored SKILL.md bytes + files listing."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    doc = "---\nname: fetch-demo\ndescription: canonical fetch fixture\n---\n# Fetch Demo\nBody.\n"
    make_artifact(
        f"{ns}/fetch-demo",
        "skill",
        {
            "fetch-demo/SKILL.md": doc,
            "fetch-demo/reference/notes.md": "support notes\n",
        },
    )
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    payload = _payload(
        _call_fetch(runner, project_dir, {"ref": f"{REGISTRY_HOST}/{ns}/fetch-demo:latest"})
    )
    assert payload["content"] == doc, "canonical content must be the authored bytes"
    assert payload["kind"] == "skill"
    assert payload["name"] == "fetch-demo"
    assert payload["vendor"] == "canonical"
    assert payload["digest"].startswith("sha256:")
    assert not payload.get("truncated", False)
    files = {f["path"]: f["size"] for f in payload["files"]}
    assert files == {
        "fetch-demo/SKILL.md": len(doc.encode()),
        "fetch-demo/reference/notes.md": len(b"support notes\n"),
    }


def test_fetch_vendor_projection_matches_installed_skill_md(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    """``vendor=claude`` returns the projection ``grim install`` writes.

    The fixture carries ``claude.*`` metadata so the Claude projection
    differs from the canonical bytes; the fetched content must equal the
    SKILL.md an actual install materializes.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    doc = (
        "---\n"
        "name: proj-demo\n"
        "description: Projection fixture.\n"
        "metadata:\n"
        '  claude.user-invocable: "false"\n'
        "---\n"
        "# Projection Demo\n"
    )
    make_artifact(f"{ns}/proj-demo", "skill", {"proj-demo/SKILL.md": doc})
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    (project_dir / ".claude").mkdir()
    runner = grim_at(project_dir)

    canonical = _payload(
        _call_fetch(runner, project_dir, {"ref": f"{REGISTRY_HOST}/{ns}/proj-demo:latest"})
    )
    projected = _payload(
        _call_fetch(
            runner,
            project_dir,
            {"ref": f"{REGISTRY_HOST}/{ns}/proj-demo:latest", "vendor": "claude"},
        )
    )
    assert projected["vendor"] == "claude"
    assert projected["content"] != canonical["content"], (
        "a claude.* fixture must project differently from canonical"
    )
    assert "user-invocable: false" in projected["content"]

    # Ground truth: the projection equals what a real install writes.
    runner.json("add", f"{REGISTRY_HOST}/{ns}/proj-demo:latest")
    runner.json("install", "--client", "claude")
    installed = (project_dir / ".claude/skills/proj-demo/SKILL.md").read_text()
    assert projected["content"] == installed, (
        f"fetched projection must equal the installed SKILL.md;\n"
        f"  fetched: {projected['content']!r}\n  installed: {installed!r}"
    )


def test_fetch_path_returns_exact_support_file(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    support = "#!/bin/sh\necho support\n"
    make_artifact(
        f"{ns}/path-demo",
        "skill",
        {"path-demo/SKILL.md": "---\nname: path-demo\ndescription: d\n---\n", "path-demo/run.sh": support},
    )
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    payload = _payload(
        _call_fetch(
            runner,
            project_dir,
            {"ref": f"{REGISTRY_HOST}/{ns}/path-demo:latest", "path": "path-demo/run.sh"},
        )
    )
    assert payload["content"] == support
    assert payload["path"] == "path-demo/run.sh"


def test_fetch_truncates_oversize_file_with_marker(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    """A >256 KiB file truncates with the escape-hatch marker, not an error."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    big = "x" * (_DOC_CAP + 4096)
    doc = f"---\nname: big-demo\ndescription: d\n---\n{big}\n"
    make_artifact(f"{ns}/big-demo", "skill", {"big-demo/SKILL.md": doc})
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    payload = _payload(
        _call_fetch(runner, project_dir, {"ref": f"{REGISTRY_HOST}/{ns}/big-demo:latest"})
    )
    assert payload["truncated"] is True
    assert "grim_render" in payload["content"][-200:], (
        "truncated content must end with the marker naming grim_render"
    )
    # Full (untruncated) size still reported in the files listing.
    files = {f["path"]: f["size"] for f in payload["files"]}
    assert files["big-demo/SKILL.md"] == len(doc.encode())


def test_fetch_unknown_ref_is_clean_tool_error(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    responses = _drive(
        runner,
        project_dir,
        [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": _PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "pytest", "version": "0"},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "grim_fetch",
                    "arguments": {"ref": f"{REGISTRY_HOST}/grim-test/does-not-exist:latest"},
                },
            },
        ],
    )
    msg = responses[2]
    # rmcp surfaces a handler error as a JSON-RPC error object; either shape
    # (protocol error or isError result) is acceptable as long as it is clean.
    if "result" in msg:
        assert msg["result"]["isError"] is True
    else:
        assert "error" in msg
        assert "not found" in msg["error"]["message"]


def test_fetch_mcp_kind_descriptor_and_vendor_pointer(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    """mcp-kind fetch returns the descriptor JSON; vendor adds the pointer."""
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    descriptor = project_dir / "mcp-demo.toml"
    descriptor.write_text(
        'description = "Fetchable MCP server."\n'
        "[server]\n"
        'transport = "stdio"\n'
        'command = "demo-mcp"\n'
        'args = ["serve"]\n'
    )
    repo = f"{REGISTRY_HOST}/{ns}/mcp/demo"
    runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")

    canonical = _payload(_call_fetch(runner, project_dir, {"ref": f"{repo}:1.0.0"}))
    assert canonical["kind"] == "mcp"
    doc = json.loads(canonical["content"])
    assert doc["description"] == "Fetchable MCP server."
    assert doc["server"]["command"] == "demo-mcp"

    projected = _payload(
        _call_fetch(runner, project_dir, {"ref": f"{repo}:1.0.0", "vendor": "claude"})
    )
    assert projected["pointer"] == "/mcpServers/demo"
    entry = json.loads(projected["content"])
    assert entry["command"] == "demo-mcp"
    assert entry["args"] == ["serve"]


def test_fetch_offline_errors_cleanly_without_hang(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, registry: str
) -> None:
    """GRIM_OFFLINE fetch fails at the manifest with a clean error, no hang.

    Manifests are not cached (documented limitation), so even a warm blob
    cache cannot serve a fetch offline.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    make_artifact(
        f"{ns}/offline-demo",
        "skill",
        {"offline-demo/SKILL.md": "---\nname: offline-demo\ndescription: d\n---\n"},
    )
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    responses = _drive(
        runner,
        project_dir,
        [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": _PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "pytest", "version": "0"},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "grim_fetch",
                    "arguments": {"ref": f"{REGISTRY_HOST}/{ns}/offline-demo:latest"},
                },
            },
        ],
        offline=True,
        timeout=30,
    )
    msg = responses[2]
    if "result" in msg:
        assert msg["result"]["isError"] is True
    else:
        assert "error" in msg
