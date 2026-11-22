# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim fetch`` and ``grim_fetch`` read a published mod without installing it."""

from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_mod
from src.stdio_drivers import drive_until_answered, initialize

INDEX_JS = "export default {};\n"


def test_fetch_mod_reads_manifest_files_and_one_path(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/status-band", "1")
    (project_dir / "grimoire.toml").write_text(
        "[skills]\n", encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)

    doc = runner.json("fetch", mod.fq)
    assert doc["kind"] == "mod"
    assert doc["name"] == "status-band"
    assert 'name        = "status-band"' in doc["content"]
    sizes = {f["path"]: f["size"] for f in doc["files"]}
    assert sizes == {
        "status-band/mod.toml": len(doc["content"].encode("utf-8")),
        "status-band/claude/index.js": len(INDEX_JS),
        "status-band/opencode/index.ts": len(INDEX_JS),
    }

    one = runner.json("fetch", mod.fq, "--path", "status-band/claude/index.js")
    assert one["content"] == INDEX_JS

    refused = runner.run("fetch", mod.fq, "--vendor", "claude", check=False)
    assert refused.returncode == 1, refused.stderr

    responses = drive_until_answered(
        runner,
        project_dir,
        [
            initialize(1),
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "grim_fetch", "arguments": {"ref": mod.fq}},
            },
        ],
        offline=False,
    )
    call = responses[2]["result"]
    assert call["isError"] is False, call
    mcp_doc = json.loads(call["content"][0]["text"])
    assert mcp_doc["content"] == doc["content"]
    assert mcp_doc["files"] == doc["files"]
