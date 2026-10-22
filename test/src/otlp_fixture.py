# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A capture OTLP/HTTP logs collector for telemetry acceptance tests.

``OtlpCollector`` records every request it receives (method, path, headers,
raw body) and decodes ``POST /v1/logs`` bodies into ``ExportLogsServiceRequest``.
The ``otlp_collector`` fixture in ``tests/conftest.py`` serves one per test.

Modes (``collector.mode``, settable at any time):

- ``ok``: 200, empty ``ExportLogsServiceResponse``, ``application/x-protobuf``.
- ``status=<code>``: that status, empty body (``status=503``).
- ``hang``: accepts and never answers until the collector is closed.
- ``redirect``: 307 to ``/redirected`` on this server; that request is
  recorded too, so a test can assert it got zero requests.

Debug-line helpers (``telemetry_payload``) parse grim's ``GRIM_LOG=debug``
stderr, so tests need no collector to read the payload.
"""

from __future__ import annotations

import json
import re
import socket
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from http.client import HTTPMessage
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from opentelemetry.proto.collector.logs.v1 import logs_service_pb2

LOGS_PATH = "/v1/logs"
REDIRECT_TARGET = "/redirected"
_PAYLOAD_MARKER = "telemetry payload "
_ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")


@dataclass
class Recorded:
    method: str
    path: str
    headers: HTTPMessage  # case-insensitive: headers["Content-Type"]
    body: bytes
    logs: logs_service_pb2.ExportLogsServiceRequest | None = None


def any_value(av: Any) -> Any:
    """An OTLP ``AnyValue`` as a plain Python value."""
    kind = av.WhichOneof("value")
    if kind is None:
        return None
    if kind == "array_value":
        return [any_value(v) for v in av.array_value.values]
    if kind == "kvlist_value":
        return attributes(av.kvlist_value.values)
    return getattr(av, kind)


def attributes(kvs: Any) -> dict[str, Any]:
    return {kv.key: any_value(kv.value) for kv in kvs}


def flatten(req: logs_service_pb2.ExportLogsServiceRequest) -> dict[str, Any]:
    """Plain-dict view of one export.

    ``resource`` is the first ResourceLogs' attributes, ``scope`` its first
    scope's ``{name, version}``, ``records`` every log record in wire order as
    ``{event_name, attributes}``. ``resource_logs`` / ``scope_logs`` count the
    messages, for asserting the one-resource, one-scope shape.
    """
    resources = list(req.resource_logs)
    scopes = [s for r in resources for s in r.scope_logs]
    return {
        "resource": attributes(resources[0].resource.attributes) if resources else {},
        "scope": (
            {"name": scopes[0].scope.name, "version": scopes[0].scope.version}
            if scopes
            else {}
        ),
        "resource_logs": len(resources),
        "scope_logs": len(scopes),
        "records": [
            {"event_name": rec.event_name, "attributes": attributes(rec.attributes)}
            for s in scopes
            for rec in s.log_records
        ],
    }


def closed_port() -> int:
    """A loopback port nobody listens on (connections are refused)."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _check_mode(mode: str) -> str:
    if mode in ("ok", "hang", "redirect"):
        return mode
    if mode.startswith("status="):
        int(mode.removeprefix("status="))
        return mode
    raise ValueError(f"unknown collector mode {mode!r}")


class _Handler(BaseHTTPRequestHandler):
    server: _Server

    def _body(self) -> bytes:
        if "chunked" in self.headers.get("Transfer-Encoding", "").lower():
            out = b""
            while True:
                size = int(self.rfile.readline().split(b";")[0].strip() or b"0", 16)
                out += self.rfile.read(size)
                self.rfile.readline()
                if size == 0:
                    return out
        return self.rfile.read(int(self.headers.get("Content-Length") or 0))

    def _handle(self) -> None:
        body = self._body()
        rec = Recorded(self.command, self.path, self.headers, body)
        if self.command == "POST" and self.path == LOGS_PATH:
            req = logs_service_pb2.ExportLogsServiceRequest()
            req.ParseFromString(body)
            rec.logs = req
        self.server.record(rec)

        mode = self.server.mode
        if mode == "hang":
            self.server.release.wait()
            return
        if mode == "redirect" and self.path != REDIRECT_TARGET:
            self.send_response(307)
            self.send_header("Location", REDIRECT_TARGET)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        status = (
            int(mode.removeprefix("status=")) if mode.startswith("status=") else 200
        )
        payload = logs_service_pb2.ExportLogsServiceResponse().SerializeToString()
        self.send_response(status)
        self.send_header("Content-Type", "application/x-protobuf")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    do_GET = do_POST = do_PUT = do_DELETE = _handle

    def log_message(self, format: str, *args: Any) -> None:
        pass


class _Server(ThreadingHTTPServer):
    daemon_threads = True  # a hung handler must not keep the process alive
    block_on_close = False  # server_close must not join a hung handler

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), _Handler)
        self.mode = "ok"
        self.release = threading.Event()
        self._lock = threading.Lock()
        self._requests: list[Recorded] = []

    def record(self, rec: Recorded) -> None:
        with self._lock:
            self._requests.append(rec)


class OtlpCollector:
    def __init__(self) -> None:
        self._server = _Server()
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def mode(self) -> str:
        return self._server.mode

    @mode.setter
    def mode(self, value: str) -> None:
        self._server.mode = _check_mode(value)

    @property
    def port(self) -> int:
        return self._server.server_address[1]

    @property
    def endpoint(self) -> str:
        """Base URL (no path): the value for ``OTEL_EXPORTER_OTLP_ENDPOINT``."""
        return f"http://127.0.0.1:{self.port}"

    def url(self, path: str) -> str:
        return self.endpoint + path

    @property
    def requests(self) -> list[Recorded]:
        """Every request received, in arrival order (a snapshot)."""
        with self._server._lock:
            return list(self._server._requests)

    def requests_to(self, path: str) -> list[Recorded]:
        return [r for r in self.requests if r.path == path]

    @property
    def log_posts(self) -> list[Recorded]:
        """``POST /v1/logs`` requests."""
        return [r for r in self.requests if r.logs is not None]

    @property
    def exports(self) -> list[dict[str, Any]]:
        """``flatten`` of each ``POST /v1/logs``, in arrival order."""
        return [flatten(r.logs) for r in self.log_posts if r.logs is not None]

    def close(self) -> None:
        self._server.release.set()  # unblock `hang` handlers first
        self._server.shutdown()
        self._server.server_close()


@contextmanager
def serve_collector(mode: str = "ok") -> Iterator[OtlpCollector]:
    collector = OtlpCollector()
    collector.mode = mode
    try:
        yield collector
    finally:
        collector.close()


def telemetry_payload_lines(stderr: str) -> list[str]:
    """The ANSI-stripped stderr lines carrying the debug payload marker."""
    return [ln for ln in _ANSI.sub("", stderr).splitlines() if _PAYLOAD_MARKER in ln]


def telemetry_payload(stderr: str) -> dict[str, Any]:
    """Parsed JSON of the single ``telemetry payload <json>`` line; fails otherwise."""
    lines = telemetry_payload_lines(stderr)
    assert len(lines) == 1, (
        f"expected exactly one telemetry payload line, got {len(lines)}; stderr: {stderr!r}"
    )
    line = lines[0]
    return json.loads(line[line.index(_PAYLOAD_MARKER) + len(_PAYLOAD_MARKER) :])
