# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A stored credential never travels in cleartext to a host only a project file declares insecure.

Every recorder keeps a boolean for the ``Authorization`` header, never its value,
and every credential here is a dummy string.
"""

from __future__ import annotations

import base64
import http.client
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.scenarios import allow_insecure, fresh_runner

_USER = "dummy-user"
_SECRET = "dummy-secret"
_IDENTITY_TOKEN = "dummy-identity-token"

_HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "upgrade",
    "host",
    "authorization",
}
_CONTENT = re.compile(r"^/v2/.+/(manifests|blobs)/")
_BASIC = {"WWW-Authenticate": 'Basic realm="grim-test"'}

_HELPER = f"""#!/usr/bin/env python3
import json, os, sys
if sys.argv[1:] == ["get"]:
    server = sys.stdin.read().strip()
    open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "ran"), "w").close()
    print(json.dumps({{"ServerURL": server, "Username": "{_USER}", "Secret": "{_SECRET}"}}))
else:
    sys.exit(1)
"""


@dataclass(frozen=True)
class _Seen:
    role: str
    path: str
    has_authorization: bool


class _Handler(BaseHTTPRequestHandler):
    server: _Scripted

    def _handle(self) -> None:
        srv = self.server
        with srv.lock:
            srv.records.append(
                _Seen(
                    srv.role,
                    self.path,
                    "authorization" in {k.lower() for k in self.headers},
                )
            )
        if srv.role == "realm":
            return self._reply(401)
        if srv.mode == "token-realm":
            return self._reply(
                401,
                {"WWW-Authenticate": f'Bearer realm="{srv.realm}",service="grim-test"'},
            )
        path = self.path
        if path.startswith("/v2/_catalog"):
            return self._reply(404)
        # An armed hop challenges for Basic, so a password or helper credential would be sent.
        if srv.mode == "demand-basic" or (srv.mode != "open" and path == "/v2/"):
            return self._reply(401, _BASIC)
        if srv.mode == "redirect":
            if _CONTENT.match(path):
                return self._reply(307, {"Location": f"/hop{path}"})
            path = path.removeprefix("/hop")
        self._forward(path)

    def _reply(self, status: int, headers: dict[str, str] | None = None) -> None:
        code = {401: "UNAUTHORIZED", 404: "NAME_UNKNOWN"}.get(status)
        body = (
            json.dumps({"errors": [{"code": code, "message": "scripted"}]}).encode()
            if code
            else b""
        )
        self.send_response(status)
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _forward(self, path: str) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else None
        headers = {
            k: v for k, v in self.headers.items() if k.lower() not in _HOP_BY_HOP
        }
        upstream = http.client.HTTPConnection(REGISTRY_HOST, timeout=60)
        try:
            upstream.request(self.command, path, body=body, headers=headers)
            resp = upstream.getresponse()
            payload = resp.read()
            self.send_response(resp.status)
            for key, value in resp.getheaders():
                if key.lower() not in _HOP_BY_HOP:
                    self.send_header(key, value)
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(payload)
        finally:
            upstream.close()

    do_GET = do_HEAD = do_POST = do_PUT = do_PATCH = do_DELETE = _handle

    def log_message(self, format: str, *args: Any) -> None:
        pass


class _Scripted(ThreadingHTTPServer):
    daemon_threads = True
    block_on_close = False

    def __init__(self, role: str, records: list[_Seen], lock: threading.Lock) -> None:
        super().__init__(("127.0.0.1", 0), _Handler)
        self.role = role
        self.mode = "open"
        self.realm = ""
        self.records = records
        self.lock = lock
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def host(self) -> str:
        return f"127.0.0.1:{self.server_address[1]}"

    def close(self) -> None:
        self.shutdown()
        self.server_close()


class _Hop:
    """One hop's servers: ``open`` while the author locks, then ``arm`` switches the hop on."""

    def __init__(self, hop: str) -> None:
        self.hop = hop
        self.records: list[_Seen] = []
        self.lock = threading.Lock()
        registry = _Scripted("registry", self.records, self.lock)
        self.servers = [registry]
        if hop == "token-realm":
            realm = _Scripted("realm", self.records, self.lock)
            self.servers.append(realm)
            registry.realm = f"http://{realm.host}/token"
        self.host = registry.host

    def arm(self) -> None:
        with self.lock:
            self.records.clear()
        self.servers[0].mode = self.hop

    def seen(self) -> list[_Seen]:
        with self.lock:
            return list(self.records)

    def exercised(self, seen: list[_Seen]) -> bool:
        if self.hop == "token-realm":
            return any(s.role == "realm" for s in seen)
        if self.hop == "redirect":
            return any(s.path.startswith("/hop/") for s in seen)
        return any(_CONTENT.match(s.path) for s in seen)


@contextmanager
def _serve(hop: str) -> Iterator[_Hop]:
    served = _Hop(hop)
    try:
        yield served
    finally:
        for server in served.servers:
            server.close()


def _store_credential(runner: GrimRunner, root: Path, host: str, kind: str) -> None:
    config = root / "docker"
    config.mkdir()
    runner.env["DOCKER_CONFIG"] = str(config)
    if kind == "password":
        auth = base64.b64encode(f"{_USER}:{_SECRET}".encode()).decode()
        doc: dict[str, Any] = {"auths": {host: {"auth": auth}}}
    elif kind == "identity-token":
        doc = {"auths": {host: {"identitytoken": _IDENTITY_TOKEN}}}
    else:
        bin_dir = root / "helper-bin"
        bin_dir.mkdir()
        helper = bin_dir / "docker-credential-grimtest"
        helper.write_text(_HELPER, encoding="utf-8", newline="\n")
        helper.chmod(helper.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
        if sys.platform == "win32":
            # Windows resolves a helper through PATHEXT, so a batch file runs the script.
            shim = f'@"{sys.executable}" "{helper}" %*\r\n'
            (bin_dir / "docker-credential-grimtest.cmd").write_text(
                shim, encoding="utf-8", newline=""
            )
        runner.env["PATH"] = f"{bin_dir}{os.pathsep}{runner.env.get('PATH', '')}"
        doc = {"credHelpers": {host: "grimtest"}}
    (config / "config.json").write_text(json.dumps(doc), encoding="utf-8", newline="\n")


def _declare_insecure(project: Path, host: str, namespace: str) -> None:
    config = project / "grimoire.toml"
    text = config.read_text(encoding="utf-8")
    block = f'\n[[registries]]\nalias = "project"\noci = "{host}/{namespace}"\ninsecure = true\n'
    config.write_text(text + block, encoding="utf-8", newline="\n")


@dataclass(frozen=True)
class _Pull:
    result: subprocess.CompletedProcess[str]
    seen: list[_Seen]
    exercised: bool
    project: Path

    def authorized(self) -> list[tuple[str, str]]:
        return [(s.role, s.path) for s in self.seen if s.has_authorization]


def _pull_through(
    hop: str,
    kind: str,
    command: tuple[str, ...],
    tmp_path: Path,
    unique_repo: str,
    *,
    user_declared: bool = False,
) -> _Pull:
    """Lock against an open hop, arm it, then run ``command`` as a user holding a credential."""
    art = make_artifact(
        f"{unique_repo}/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\n---\n# helper\n"},
    )
    project = tmp_path / "project"
    (project / ".claude").mkdir(parents=True)
    with _serve(hop) as served:
        write_config(project, skills={"helper": f"{served.host}/{art.repo}:{art.tag}"})
        _declare_insecure(project, served.host, unique_repo)
        author = fresh_runner(tmp_path, "author", cwd=project)
        author.env["DOCKER_CONFIG"] = str(tmp_path / "author-docker")
        author.run("lock")
        if command == ("status",):
            # Files on disk, no record: a cold store makes plain status fetch to adopt them.
            author.run("install")
            shutil.rmtree(project / ".grimoire")
        served.arm()
        victim = fresh_runner(tmp_path, "victim", cwd=project)
        if user_declared:
            allow_insecure(victim, served.host)
        _store_credential(victim, tmp_path, served.host, kind)
        result = victim.run(*command, check=False)
        seen = served.seen()
    return _Pull(result, seen, served.exercised(seen), project)


@pytest.mark.parametrize(
    "command",
    [("install",), ("status", "--check"), ("status",)],
    ids=["install", "status-check", "status"],
)
@pytest.mark.parametrize("kind", ["password", "identity-token", "helper"])
@pytest.mark.parametrize("hop", ["registry", "token-realm", "redirect"])
def test_stored_credential_stays_off_a_host_only_the_project_declares_insecure(
    hop: str,
    kind: str,
    command: tuple[str, ...],
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """No request on the registry, token-realm or redirect hop carries a stored credential.

    The pull succeeds anonymously where the registry allows it and exits 80 where it demands one.
    """
    pull = _pull_through(hop, kind, command, tmp_path, unique_repo)
    result = pull.result

    assert not pull.authorized(), (
        f"an Authorization header reached the plain-HTTP {hop} hop: {pull.authorized()}"
    )
    assert pull.exercised, (
        f"the {hop} hop saw no matching request: {[(s.role, s.path) for s in pull.seen]}"
    )
    demands = hop == "token-realm" and command == ("install",)
    assert result.returncode == (80 if demands else 0), (
        f"rc={result.returncode} stderr: {result.stderr.strip()}"
    )
    if command == ("install",) and not demands:
        assert (pull.project / ".claude" / "skills" / "helper" / "SKILL.md").is_file()
    assert "GRIM_INSECURE_REGISTRIES" in result.stderr, "the warning names the opt-in"


def test_a_registry_demanding_basic_auth_gets_no_stored_password_and_exits_80(
    tmp_path: Path, unique_repo: str
) -> None:
    """A plain-HTTP registry only the project declares, refusing anonymous reads, never sees the password."""
    pull = _pull_through(
        "demand-basic", "password", ("install",), tmp_path, unique_repo
    )

    assert not pull.authorized(), (
        f"an Authorization header was sent: {pull.authorized()}"
    )
    assert pull.exercised, f"no content request: {[s.path for s in pull.seen]}"
    assert pull.result.returncode == 80, (
        f"rc={pull.result.returncode} stderr: {pull.result.stderr.strip()}"
    )


@pytest.mark.parametrize("kind", ["password", "identity-token", "helper"])
def test_the_recorder_sees_the_credential_a_user_declared_host_receives(
    kind: str, tmp_path: Path, unique_repo: str
) -> None:
    """Positive control: with the host in GRIM_INSECURE_REGISTRIES the stored credential is sent and recorded."""
    pull = _pull_through(
        "registry", kind, ("install",), tmp_path, unique_repo, user_declared=True
    )

    assert pull.authorized(), (
        f"no Authorization header recorded: {[(s.role, s.path) for s in pull.seen]}"
    )
    assert pull.result.returncode == 0, (
        f"rc={pull.result.returncode} stderr: {pull.result.stderr.strip()}"
    )
    assert "GRIM_INSECURE_REGISTRIES" not in pull.result.stderr, (
        "no warning for a user's host"
    )
    if kind == "helper":
        assert (tmp_path / "helper-bin" / "ran").is_file(), (
            "the credential helper never ran"
        )
