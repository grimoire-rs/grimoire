# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A threaded HTTP proxy in front of the session registry.

Modes (``proxy.mode``, settable at any time):

- ``count``: forward, and count requests per path.
- ``record``: ``count`` plus one ``RecordedRequest`` per request.
- ``hang``: accept the connection and never answer until the proxy closes.

A config that names ``proxy.host`` makes every pin name it, so the proxy sees
all of grim's traffic. The runner needs the host in ``GRIM_INSECURE_REGISTRIES``
(``src.scenarios.allow_insecure``).

Header values are never stored: a recorded request keeps the header names and
whether an ``Authorization`` header was present, so a credential cannot leak
through a test failure message.
"""

from __future__ import annotations

import http.client
import threading
from collections import Counter
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from src.registry import REGISTRY_HOST

MODES = ("count", "record", "hang")
_HOP_BY_HOP = {"connection", "keep-alive", "transfer-encoding", "te", "upgrade", "host"}


@dataclass(frozen=True)
class RecordedRequest:
    method: str
    path: str
    header_names: tuple[str, ...]
    has_authorization: bool


class _Handler(BaseHTTPRequestHandler):
    server: _Server

    def _handle(self) -> None:
        proxy = self.server
        if proxy.mode == "hang":
            proxy.release.wait()
            return
        proxy.note(self)
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else None
        headers = {k: v for k, v in self.headers.items() if k.lower() not in _HOP_BY_HOP}
        upstream = http.client.HTTPConnection(proxy.upstream, timeout=60)
        try:
            upstream.request(self.command, self.path, body=body, headers=headers)
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


class _Server(ThreadingHTTPServer):
    daemon_threads = True  # a hung handler must not keep the process alive
    block_on_close = False

    def __init__(self, upstream: str) -> None:
        super().__init__(("127.0.0.1", 0), _Handler)
        self.upstream = upstream
        self.mode = "count"
        self.release = threading.Event()
        self.lock = threading.Lock()
        self.counts: Counter[str] = Counter()
        self.records: list[RecordedRequest] = []

    def note(self, handler: _Handler) -> None:
        with self.lock:
            self.counts[handler.path] += 1
            if self.mode == "record":
                names = tuple(sorted({k.lower() for k in handler.headers}))
                self.records.append(
                    RecordedRequest(
                        handler.command,
                        handler.path,
                        names,
                        "authorization" in names,
                    )
                )


class OciProxy:
    def __init__(self, upstream: str = REGISTRY_HOST, mode: str = "count") -> None:
        self._server = _Server(upstream)
        self.mode = mode
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def mode(self) -> str:
        return self._server.mode

    @mode.setter
    def mode(self, value: str) -> None:
        if value not in MODES:
            raise ValueError(f"unknown proxy mode {value!r}")
        self._server.mode = value

    @property
    def host(self) -> str:
        """``127.0.0.1:<port>``: the registry host a config names."""
        return f"127.0.0.1:{self._server.server_address[1]}"

    @property
    def counts(self) -> dict[str, int]:
        """Requests per path, a snapshot."""
        with self._server.lock:
            return dict(self._server.counts)

    def count(self, path: str) -> int:
        return self.counts.get(path, 0)

    @property
    def requests(self) -> list[RecordedRequest]:
        """Recorded requests in arrival order (``record`` mode only)."""
        with self._server.lock:
            return list(self._server.records)

    def close(self) -> None:
        self._server.release.set()  # unblock `hang` handlers first
        self._server.shutdown()
        self._server.server_close()


@contextmanager
def serve_proxy(upstream: str = REGISTRY_HOST, mode: str = "count") -> Iterator[OciProxy]:
    proxy = OciProxy(upstream, mode)
    try:
        yield proxy
    finally:
        proxy.close()
