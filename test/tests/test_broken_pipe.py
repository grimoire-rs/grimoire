# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim <cmd> | head``-style broken-pipe acceptance tests.

Covers the SIGPIPE / broken-pipe hardening: grim's own stdout closing
underneath it (the classic ``| head`` pipeline) must exit 0 silently, never
panic and never print "broken pipe" noise to stderr.

Determinism: a raw ``os.pipe()`` stands in for the downstream reader. The
write end becomes the child's stdout; ``fcntl.F_SETPIPE_SZ`` shrinks its
kernel buffer to 4096 bytes before the child starts, and the read end is
closed immediately without ever being read. For any output over 4 KiB the
child cannot fit its entire payload in the shrunk buffer, so it must block
in ``write()`` once full — and POSIX guarantees a blocked writer wakes with
EPIPE the instant the last read end closes. That makes the outcome
independent of process-scheduling order: no race between "child writes
everything before we close" and "we close before the child writes".

Linux-only: ``fcntl.F_SETPIPE_SZ`` has no equivalent on Windows or macOS.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    sys.platform != "linux",
    reason="fcntl.F_SETPIPE_SZ pipe-buffer shrinking is Linux-only",
)

_PIPE_CAPACITY = 4096


def _run_stdout_pipe_closed(
    binary: Path,
    args: list[str],
    env: dict[str, str],
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run *binary* with stdout wired to a 4 KiB pipe whose read end is
    closed immediately, without reading. See module docstring for why this
    is deterministic for any output over 4 KiB."""
    import fcntl

    read_fd, write_fd = os.pipe()
    fcntl.fcntl(write_fd, fcntl.F_SETPIPE_SZ, _PIPE_CAPACITY)
    proc = subprocess.Popen(
        [str(binary), *args],
        stdout=write_fd,
        stderr=subprocess.PIPE,
        env=env,
        cwd=str(cwd) if cwd else None,
        text=True,
    )
    os.close(write_fd)  # child holds its own dup'd copy (fd 1)
    os.close(read_fd)  # zero readers now: the next write() gets EPIPE
    _, stderr = proc.communicate(timeout=10)
    return subprocess.CompletedProcess(proc.args, proc.returncode, None, stderr)


@pytest.mark.parametrize(
    "args",
    [
        pytest.param(["schema", "--kind", "publish"], id="schema-publish"),
        pytest.param(["completions", "zsh"], id="completions-zsh"),
    ],
)
def test_stdout_pipe_closed_exits_zero_silently(
    grim: GrimRunner, args: list[str]
) -> None:
    """The two payload-plain stdout-write paths the plan hardens: schema's
    tagged ``writeln!`` (whole-document write) and completions' buffer-first
    ``write_all`` (the vendored clap_complete generator no longer touches a
    live pipe). Both must exit 0 with no panic and no "broken pipe" noise."""
    healthy = grim.plain(*args)
    assert len(healthy.stdout.encode()) > _PIPE_CAPACITY, (
        f"{args} produced <= {_PIPE_CAPACITY} bytes; determinism guarantee "
        "no longer holds for this command"
    )

    result = _run_stdout_pipe_closed(grim.binary, args, grim.env)

    assert result.returncode == 0, result.stderr
    assert "panic" not in result.stderr.lower(), result.stderr
    assert "broken pipe" not in result.stderr.lower(), result.stderr


def test_stdout_pipe_closed_render_command_exits_zero(
    grim_at, project_dir: Path
) -> None:
    """Weaker-determinism companion: ``context --format json`` exercises the
    ``render()`` sentinel-tagging path directly, but its JSON payload is well
    under 4 KiB — unlike the parametrized cases above, grim may finish
    writing before the parent's ``close(read_fd)`` lands, so this is a
    timing-margin case, not a guaranteed-deterministic one. Kept anyway to
    cover the render() path rather than only the two payload-plain commands."""
    (project_dir / "grimoire.toml").write_text("[skills]\n[rules]\n")
    (project_dir / ".claude").mkdir()
    runner = grim_at(project_dir)

    result = _run_stdout_pipe_closed(
        runner.binary,
        ["--format", "json", "context"],
        runner.env,
        cwd=project_dir,
    )

    assert result.returncode == 0, result.stderr
    assert "panic" not in result.stderr.lower(), result.stderr
    assert "broken pipe" not in result.stderr.lower(), result.stderr
