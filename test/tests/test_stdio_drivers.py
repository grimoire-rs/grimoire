# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Regression tests for the shared ``grim mcp`` stdio driver.

The MCP stdio transport reads a closed stdin as the client shutting down, and
rmcp then gives a request still in flight a short drain window before the
server exits without answering it. A driver that closes stdin as soon as it
has written the batch loses every tool call slower than that window, which a
live search under parallel load is.
"""

from __future__ import annotations

import os
import subprocess
import time
from collections.abc import Callable
from pathlib import Path

import pytest

from src import stdio_drivers
from src.index_fixture import Fault, IndexHandler, serve
from src.runner import GrimRunner
from src.stdio_drivers import drive_until_answered, initialize

# Longer than rmcp's 5 s post-EOF drain window, so a driver that closes stdin
# before the answer loses it on every run, whatever the machine load.
CATALOG_STALL = 7.0

# Far longer than any answer takes, so a driver that holds stdin open until
# its timeout is told apart from one that closes it on the last answer.
HOLD_TIMEOUT = 30.0
PROMPT = 10.0

_JSON = {"Content-Type": "application/json"}

_POSIX_ONLY = pytest.mark.skipif(
    os.name == "nt", reason="the stand-in servers are sh scripts"
)


def _stand_in(tmp_path: Path, name: str, body: str) -> GrimRunner:
    """A runner whose binary is a sh script standing in for `grim mcp`."""
    script = tmp_path / name
    script.write_text(f"#!/bin/sh\n{body}", encoding="utf-8")
    script.chmod(0o755)
    home = tmp_path / f"{name}-home"
    home.mkdir()
    return GrimRunner(script, home, cwd=tmp_path)


def test_a_tool_call_slower_than_the_drain_window_is_answered(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, tmp_path: Path
) -> None:
    root = tmp_path / "registry-stub"
    root.mkdir()
    handler = IndexHandler.new(
        {
            "/v2/": Fault(body=b"{}", headers=_JSON),
            "/v2/_catalog": Fault(
                body=b'{"repositories": []}', headers=_JSON, stall_before=CATALOG_STALL
            ),
        }
    )
    with serve(root, handler) as base:
        host = base.removeprefix("http://")
        (project_dir / "grimoire.toml").write_text(
            f'[[registries]]\nalias = "stub"\noci = "{host}/grim-test/stall"\n\n[skills]\n'
        )
        runner = grim_at(project_dir)
        insecure = runner.env.get("GRIM_INSECURE_REGISTRIES")
        runner.env["GRIM_INSECURE_REGISTRIES"] = (
            f"{insecure},{host}" if insecure else host
        )

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
                    "params": {"name": "grim_search", "arguments": {"refresh": True}},
                },
            ],
            offline=False,
        )

    paths = [r.path for r in handler.log]
    assert any(p.startswith("/v2/_catalog") for p in paths), paths
    assert 1 in responses, sorted(responses)
    assert 2 in responses, (
        f"the slow grim_search call (id 2) got no response; ids {sorted(responses)}"
    )
    assert responses[2]["result"]["isError"] is False, responses[2]


def test_a_batch_without_request_ids_closes_stdin_at_once(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path
) -> None:
    """Nothing to wait for, so stdin closes after the write: a server that
    reads to EOF before acting, as `grim mcp` does on an empty batch, would
    otherwise sit out the whole timeout."""
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    started = time.monotonic()
    responses = drive_until_answered(runner, project_dir, [], timeout=30)

    assert responses == {}
    assert time.monotonic() - started < 15, "stdin stayed open with no id to wait for"


def test_stdin_closes_once_every_id_is_answered(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path
) -> None:
    """`grim mcp` answers `initialize` at once and then waits for stdin EOF, so
    the call returns promptly only if the driver closes stdin on the last answer
    rather than when its timeout runs out."""
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = grim_at(project_dir)

    started = time.monotonic()
    responses = drive_until_answered(
        runner, project_dir, [initialize(1)], timeout=HOLD_TIMEOUT
    )
    elapsed = time.monotonic() - started

    assert 1 in responses, sorted(responses)
    assert elapsed < PROMPT, f"stdin stayed open {elapsed:.1f}s after the last answer"


@_POSIX_ONLY
def test_a_server_that_exits_before_reading_is_not_waited_for(tmp_path: Path) -> None:
    """A batch larger than the pipe buffer meets a closed pipe, and the server's
    stdout EOF releases the driver well before its timeout."""
    runner = _stand_in(tmp_path, "gone", "exit 0\n")
    bulky = {**initialize(1), "padding": "x" * 256 * 1024}

    started = time.monotonic()
    responses = drive_until_answered(runner, tmp_path, [bulky], timeout=HOLD_TIMEOUT)

    assert responses == {}
    assert time.monotonic() - started < PROMPT, (
        "a dead server held the driver to its timeout"
    )


@_POSIX_ONLY
def test_a_server_outliving_the_grace_is_killed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A server that ignores stdin EOF is killed once the grace runs out, and the
    caller sees `TimeoutExpired` rather than a hang."""
    pid_file = tmp_path / "server.pid"
    runner = _stand_in(tmp_path, "stuck", f"echo $$ > '{pid_file}'\nexec sleep 60\n")
    monkeypatch.setattr(stdio_drivers, "_EXIT_GRACE", 0.5)

    started = time.monotonic()
    with pytest.raises(subprocess.TimeoutExpired):
        drive_until_answered(runner, tmp_path, [initialize(1)], timeout=0.5)

    assert time.monotonic() - started < PROMPT, (
        "the driver waited for the server instead of killing it"
    )
    pid = int(pid_file.read_text(encoding="utf-8"))
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
