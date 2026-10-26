# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The dispatch table is shared by every grim version on the machine.

A row this grim cannot read (a newer tier, event or handler literal) is skipped
alone by convergence, and by the runtime when it is not addressed to the firing
`(client, event)`; a row that is addressed to it and unreadable refuses the whole
dispatch, because skipping it could silently drop a gatekeeper. Convergence never
writes over a table it cannot read at all: either would let an older grim disarm
every other workspace on the machine.

The installed `grim-hook` launcher is the reader under test. A refusal exits 65
(table or selected row unreadable) with nothing on stdout, so "refuses" below
means no handler ran.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.hook_launcher import run_launcher

pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and the payload fixtures are POSIX-only in v1"
)

ROOT = "0123456789abcdef0123456789abcdef"
OTHER_ROOT = "ffffffffffffffffffffffffffffffff"

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

PAYLOAD = json.dumps(
    {
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
        "cwd": "/repo",
        "session_id": "s-1",
    }
)


def _row(id: str, payload_dir: Path, argv: list[str], **overrides) -> dict:
    row = {
        "artifact": "shell-guard",
        "id": id,
        "client": "claude",
        "event": "PreToolUse",
        "tier": "observer",
        "matcher": "Bash",
        "handler": {"argv": argv},
        "timeout": None,
        "payload": "stdin",
        "payload_dir": str(payload_dir),
        "resolved_digest": None,
    }
    row.update(overrides)
    return row


def _recording_payload(tmp_path: Path, name: str) -> tuple[list[str], Path, Path]:
    payload_dir = tmp_path / "payloads" / name
    payload_dir.mkdir(parents=True)
    marker = tmp_path / f"{name}.marker"
    (payload_dir / "guard.sh").write_text(f"#!/bin/sh\ncat > '{marker}'\n")
    return ["sh", str(payload_dir / "guard.sh")], marker, payload_dir


def _table(grim_home: Path) -> Path:
    return grim_home / "hooks" / "dispatch.json"


def _seed(grim_home: Path, body: str) -> Path:
    table = _table(grim_home)
    table.parent.mkdir(parents=True, exist_ok=True)
    table.write_text(body)
    return table


def _armed_project(grim_at, project_dir: Path, unique_repo: str):
    hook = make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": "#!/bin/sh\ncat > /dev/null\n"},
        tag="1",
    )
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    return runner


def _claude_marked(project_dir: Path) -> bool:
    surface = project_dir / ".claude" / "settings.local.json"
    return surface.is_file() and "hook-dispatcher" in surface.read_text()


def _run_hook(table: Path, root: str = ROOT):
    # The launcher reads the table beside itself; `table` is that file.
    return run_launcher(table.parent / "grim-hook", "claude", "PreToolUse", root, stdin=PAYLOAD)


def test_launcher_skips_an_unreadable_row_addressed_elsewhere_and_runs_the_rest(
    launcher: Path, grim_home: Path, tmp_path: Path
) -> None:
    unknown_handler, unknown_marker, unknown_dir = _recording_payload(tmp_path, "unknown")
    known_handler, known_marker, known_dir = _recording_payload(tmp_path, "known")
    table = _seed(
        grim_home,
        json.dumps(
            {
                "schema": 1,
                "roots": {
                    ROOT: {
                        "root": "global",
                        "hooks": [
                            _row("unknown", unknown_dir, unknown_handler, client="codex", tier="sentinel"),
                            _row(
                                "unknown-handler", unknown_dir, [], client="codex", handler={"wasm": "guard.wasm"}
                            ),
                            _row("other-event", unknown_dir, unknown_handler, event="Stop", tier="sentinel"),
                            _row("known", known_dir, known_handler),
                        ],
                    }
                },
            }
        ),
    )

    result = _run_hook(table)

    assert result.returncode == 0, result.stderr
    assert known_marker.exists(), f"a row this grim can read must still run\nstderr: {result.stderr}"
    assert not unknown_marker.exists(), "a row addressed elsewhere was run"


@pytest.mark.parametrize(
    "bad",
    [
        {"tier": "sentinel"},
        {"handler": {"wasm": "guard.wasm"}},
        {"payload_dir": "relative/dir"},
    ],
    ids=["unknown-tier", "unknown-handler", "relative-payload-dir"],
)
def test_launcher_refuses_when_a_selected_row_cannot_be_read(
    launcher: Path, grim_home: Path, tmp_path: Path, bad: dict
) -> None:
    known_handler, known_marker, known_dir = _recording_payload(tmp_path, "known")
    table = _seed(
        grim_home,
        json.dumps(
            {
                "schema": 1,
                "roots": {
                    ROOT: {
                        "root": "global",
                        "hooks": [
                            _row("known", known_dir, known_handler),
                            {**_row("unreadable", known_dir, known_handler), **bad},
                        ],
                    }
                },
            }
        ),
    )

    result = _run_hook(table)

    assert result.returncode == 65, result.stderr
    assert not known_marker.exists(), "a selected unreadable row must refuse the dispatch, not be skipped"
    assert "unreadable" in result.stderr, result.stderr


def test_launcher_refuses_a_newer_schema(launcher: Path, grim_home: Path, tmp_path: Path) -> None:
    handler, marker, payload_dir = _recording_payload(tmp_path, "known")
    table = _seed(
        grim_home,
        json.dumps(
            {"schema": 2, "roots": {ROOT: {"root": "global", "hooks": [_row("known", payload_dir, handler)]}}}
        ),
    )

    result = _run_hook(table)

    assert result.returncode == 65, result.stderr
    assert not marker.exists(), "a table from a newer schema must arm nothing"


def test_launcher_treats_an_unknown_root_as_empty_even_beside_unreadable_roots(
    launcher: Path, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _recording_payload(tmp_path, "known")
    table = _seed(
        grim_home,
        json.dumps(
            {
                "schema": 1,
                "roots": {
                    ROOT: {"root": "global", "hooks": [_row("known", payload_dir, handler)]},
                    OTHER_ROOT: {"layout": "from a newer grim"},
                },
            }
        ),
    )

    unknown = _run_hook(table, root="1" * 32)
    assert unknown.returncode == 0, unknown.stderr
    assert not marker.exists(), "an unknown root must select nothing"

    known = _run_hook(table)
    assert known.returncode == 0, known.stderr
    assert marker.exists(), "another root this grim cannot read must not disarm this one"


def test_install_keeps_another_workspaces_rows_it_cannot_read(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    newer_row = _row("future", Path("/elsewhere/payload"), ["sh", "guard.sh"], event="FutureEvent")
    known_row = _row("known", Path("/elsewhere/payload"), ["sh", "guard.sh"], env={"MODE": "strict"})
    other = {"root": "/elsewhere", "hooks": [newer_row, known_row]}
    table = _seed(grim_home, json.dumps({"schema": 1, "roots": {OTHER_ROOT: other}}))
    runner = _armed_project(grim_at, project_dir, unique_repo)

    runner.run("install", "--trust-hooks")

    roots = json.loads(table.read_text())["roots"]
    assert roots[OTHER_ROOT] == other, "another workspace's rows must survive this install verbatim"
    mine = [root for token, root in roots.items() if token != OTHER_ROOT]
    assert [row["id"] for root in mine for row in root["hooks"]] == ["guard"], roots
    assert _claude_marked(project_dir)


def test_install_leaves_a_table_it_cannot_read_untouched_and_arms_nothing(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    body = json.dumps({"schema": 2, "roots": {OTHER_ROOT: {"layout": "from a newer grim"}}})
    table = _seed(grim_home, body)
    runner = _armed_project(grim_at, project_dir, unique_repo)
    runner.run("hook", "allow")

    result = runner.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert table.read_text() == body, "a table this grim cannot read must never be written over"
    assert str(table) in result.stderr, result.stderr
    assert not _claude_marked(project_dir)
    item = next(i for i in runner.json("status")["items"] if i["kind"] == "hook")
    assert [a["cause"] for a in item["arming"]] == ["not-registered"], item

    # Positive control: with the table moved aside the same install arms.
    table.unlink()
    runner.run("install")
    assert _claude_marked(project_dir)
    item = next(i for i in runner.json("status")["items"] if i["kind"] == "hook")
    assert item["arming"] == [], item
