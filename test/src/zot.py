# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A throwaway zot OCI registry, owned by the process that starts it.

``start_zot`` serves an empty registry on a random loopback port and returns
once *our* zot is the one answering there. The zot binary comes from the
repo's ``ocx.toml`` (``ocx exec -- …`` or ``task test`` puts it on PATH).

zot runs under a watchdog that kills it when the starting process dies, so
no registry outlives the test session — not even after a SIGKILL.
"""
from __future__ import annotations

import json
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

# Runs zot as its child and kills it on stdin EOF, which the OS delivers when
# the process holding the pipe's write end exits — however it exits. Exits
# with zot's own status when zot dies first (e.g. its port was taken), so the
# watchdog's liveness is zot's liveness.
_WATCHDOG = """\
import subprocess, sys, threading
p = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL)
threading.Thread(target=lambda: (sys.stdin.buffer.read(), p.kill()), daemon=True).start()
sys.exit(p.wait())
"""

_ATTEMPTS = 5


@dataclass
class Zot:
    host: str
    _proc: subprocess.Popen[bytes]
    _root: Path

    def stop(self) -> None:
        """Stop zot and delete its storage."""
        _halt(self._proc)
        shutil.rmtree(self._root, ignore_errors=True)


def start_zot(*, htpasswd: str | None = None) -> Zot:
    """Start zot on a free ephemeral port; ``htpasswd`` gates it with Basic auth.

    A port is only accepted once zot answers on it *and* is still alive: a
    port grabbed by someone else between the pick and zot's bind shows up as
    zot exiting, and the next attempt picks a new port.

    Raises:
        RuntimeError: zot is not on PATH, or no attempt came up.
    """
    zot = shutil.which("zot")
    if zot is None:
        raise RuntimeError(
            "zot is not on PATH. The repo's ocx.toml pins it; run the suite "
            "through ocx: `ocx exec -- uv run pytest` (or `task test`)."
        )
    root = Path(tempfile.mkdtemp(prefix="grim-zot-"))
    log = root / "zot.log"
    for _ in range(_ATTEMPTS):
        port = _free_port()
        config = root / "config.json"
        config.write_text(json.dumps(_config(root, port, htpasswd)), encoding="utf-8")
        with log.open("ab") as out:
            proc = subprocess.Popen(
                [sys.executable, "-c", _WATCHDOG, zot, "serve", str(config)],
                stdin=subprocess.PIPE,
                stdout=out,
                stderr=subprocess.STDOUT,
            )
        host = f"127.0.0.1:{port}"
        if _wait_ours(proc, host):
            return Zot(host, proc, root)
        _halt(proc)
    tail = log.read_text(encoding="utf-8", errors="replace")[-2000:]
    shutil.rmtree(root, ignore_errors=True)
    raise RuntimeError(f"zot did not come up after {_ATTEMPTS} attempts:\n{tail}")


def _halt(proc: subprocess.Popen[bytes]) -> None:
    if proc.stdin:
        proc.stdin.close()  # EOF: the watchdog kills zot, then exits
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()


def _config(root: Path, port: int, htpasswd: str | None) -> dict:
    http: dict = {"address": "127.0.0.1", "port": str(port)}
    if htpasswd is not None:
        path = root / "htpasswd"
        path.write_text(htpasswd + "\n", encoding="utf-8")
        http |= {"realm": "Registry Realm", "auth": {"htpasswd": {"path": str(path)}}}
    return {
        "distSpecVersion": "1.1.1",
        "storage": {"rootDirectory": str(root / "data"), "gc": False, "dedupe": False},
        "http": http,
        "log": {"level": "error"},
    }


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _wait_ours(proc: subprocess.Popen[bytes], host: str, timeout_s: float = 30.0) -> bool:
    """Whether zot answers on ``host`` while still alive — i.e. it owns the port."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline and proc.poll() is None:
        if _answers(host):
            # A squatter can answer before zot fails its bind; zot exits
            # right after, so confirm it outlived its startup.
            time.sleep(0.5)
            return proc.poll() is None
        time.sleep(0.1)
    return False


def _answers(host: str) -> bool:
    try:
        with urllib.request.urlopen(f"http://{host}/v2/", timeout=2) as resp:
            return resp.status == 200
    except urllib.error.HTTPError as e:
        return e.code == 401  # htpasswd-gated
    except OSError:
        return False
