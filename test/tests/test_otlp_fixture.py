# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-test for the capture OTLP collector in ``src/otlp_fixture.py``."""

from __future__ import annotations

import socket
import time
import urllib.error
import urllib.request

import pytest
from opentelemetry.proto.collector.logs.v1 import logs_service_pb2
from opentelemetry.proto.common.v1 import common_pb2
from opentelemetry.proto.logs.v1 import logs_pb2
from opentelemetry.proto.resource.v1 import resource_pb2

from src.otlp_fixture import (
    LOGS_PATH,
    REDIRECT_TARGET,
    OtlpCollector,
    closed_port,
    serve_collector,
    telemetry_payload,
)


def _kv(key: str, **value) -> common_pb2.KeyValue:
    return common_pb2.KeyValue(key=key, value=common_pb2.AnyValue(**value))


def _request() -> logs_service_pb2.ExportLogsServiceRequest:
    strings = common_pb2.ArrayValue(values=[common_pb2.AnyValue(string_value="claude")])
    record = logs_pb2.LogRecord(
        event_name="grim.command",
        attributes=[
            _kv("grim.command.name", string_value="install"),
            _kv("process.exit.code", int_value=0),
            _kv("grim.ci", bool_value=False),
            _kv("grim.command.duration", double_value=0.5),
            _kv("grim.clients", array_value=strings),
        ],
    )
    scope = logs_pb2.ScopeLogs(
        scope=common_pb2.InstrumentationScope(name="grim", version="1.2.3"),
        log_records=[record],
    )
    resource = logs_pb2.ResourceLogs(
        resource=resource_pb2.Resource(
            attributes=[_kv("service.name", string_value="grim")]
        ),
        scope_logs=[scope],
    )
    return logs_service_pb2.ExportLogsServiceRequest(resource_logs=[resource])


def _post(url: str, body: bytes, timeout: float = 5.0):
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={"Content-Type": "application/x-protobuf"},
    )
    return urllib.request.urlopen(req, timeout=timeout)


def test_post_is_captured_and_decoded_to_the_flattened_shape(
    otlp_collector: OtlpCollector,
):
    body = _request().SerializeToString()
    with _post(otlp_collector.url(LOGS_PATH), body) as resp:
        assert resp.status == 200
        assert resp.headers["Content-Type"] == "application/x-protobuf"
        assert resp.read() == b""

    (rec,) = otlp_collector.requests
    assert (rec.method, rec.path, rec.body) == ("POST", LOGS_PATH, body)
    assert rec.headers["content-type"] == "application/x-protobuf"
    assert rec.headers["CONTENT-TYPE"] == "application/x-protobuf"
    assert otlp_collector.exports == [
        {
            "resource": {"service.name": "grim"},
            "scope": {"name": "grim", "version": "1.2.3"},
            "resource_logs": 1,
            "scope_logs": 1,
            "records": [
                {
                    "event_name": "grim.command",
                    "attributes": {
                        "grim.command.name": "install",
                        "process.exit.code": 0,
                        "grim.ci": False,
                        "grim.command.duration": 0.5,
                        "grim.clients": ["claude"],
                    },
                }
            ],
        }
    ]


def test_non_log_requests_are_recorded_without_a_decoded_export(
    otlp_collector: OtlpCollector,
):
    urllib.request.urlopen(otlp_collector.url("/other")).read()
    assert [r.path for r in otlp_collector.requests] == ["/other"]
    assert otlp_collector.log_posts == []


@pytest.mark.parametrize("otlp_collector", ["status=503"], indirect=True)
def test_status_mode_answers_with_that_status(otlp_collector: OtlpCollector):
    with pytest.raises(urllib.error.HTTPError) as err:
        _post(otlp_collector.url(LOGS_PATH), _request().SerializeToString())
    assert err.value.code == 503
    assert len(otlp_collector.log_posts) == 1


def test_mode_is_settable_after_start_and_validated(otlp_collector: OtlpCollector):
    otlp_collector.mode = "status=429"
    with pytest.raises(urllib.error.HTTPError) as err:
        _post(otlp_collector.url(LOGS_PATH), b"")
    assert err.value.code == 429
    with pytest.raises(ValueError):
        otlp_collector.mode = "bogus"


@pytest.mark.parametrize("otlp_collector", ["redirect"], indirect=True)
def test_redirect_mode_answers_307_and_target_is_untouched_unless_followed(
    otlp_collector: OtlpCollector,
):
    class NoFollow(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None

    opener = urllib.request.build_opener(NoFollow)
    req = urllib.request.Request(otlp_collector.url(LOGS_PATH), data=b"", method="POST")
    with pytest.raises(urllib.error.HTTPError) as err:
        opener.open(req, timeout=5)
    assert err.value.code == 307
    assert err.value.headers["Location"] == REDIRECT_TARGET
    assert len(otlp_collector.requests_to(LOGS_PATH)) == 1
    assert otlp_collector.requests_to(REDIRECT_TARGET) == []

    urllib.request.urlopen(otlp_collector.url(REDIRECT_TARGET), timeout=5).read()
    assert len(otlp_collector.requests_to(REDIRECT_TARGET)) == 1


def test_hang_mode_times_out_the_client_and_teardown_returns_promptly():
    with serve_collector("hang") as collector:
        start = time.monotonic()
        with pytest.raises((TimeoutError, socket.timeout, urllib.error.URLError)):
            _post(collector.url(LOGS_PATH), b"", timeout=0.3)
        assert time.monotonic() - start < 2
        assert len(collector.requests) == 1  # accepted and recorded, never answered
        close_start = time.monotonic()
    assert time.monotonic() - close_start < 2


def test_closing_releases_a_hung_connection():
    with serve_collector("hang") as collector:
        conn = socket.create_connection(("127.0.0.1", collector.port), timeout=5)
        conn.sendall(b"POST /v1/logs HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
        deadline = time.monotonic() + 5
        while not collector.requests and time.monotonic() < deadline:
            time.sleep(0.01)
        assert collector.requests
    with conn:
        assert conn.recv(1) == b""  # the server side closed it, not a timeout


def test_closed_port_refuses_connections():
    # Windows retries a SYN to a closed loopback port for ~2s before refusing.
    with pytest.raises(ConnectionRefusedError):
        socket.create_connection(("127.0.0.1", closed_port()), timeout=10).close()


class TestDebugLine:
    PAYLOAD = '{"resource":{"service.name":"grim"},"events":[]}'

    def test_plain_line_is_parsed(self):
        stderr = f"noise\n2026-01-01T00:00:00Z DEBUG grim::telemetry: telemetry payload {self.PAYLOAD}\n"
        assert telemetry_payload(stderr) == {
            "resource": {"service.name": "grim"},
            "events": [],
        }

    def test_ansi_escapes_are_stripped(self):
        stderr = (
            "\x1b[2m2026-01-01T00:00:00Z\x1b[0m \x1b[34mDEBUG\x1b[0m "
            f"\x1b[2mgrim::telemetry\x1b[0m\x1b[2m:\x1b[0m telemetry payload {self.PAYLOAD}\x1b[0m\n"
        )
        assert telemetry_payload(stderr)["events"] == []

    @pytest.mark.parametrize("count", [0, 2])
    def test_not_exactly_one_line_fails(self, count: int):
        stderr = f"DEBUG x: telemetry payload {self.PAYLOAD}\n" * count
        with pytest.raises(AssertionError, match="exactly one"):
            telemetry_payload(stderr)
