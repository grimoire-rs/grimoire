# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Opt-in usage telemetry — the export path against a capture OTLP collector.

Every negative ("nothing is sent") test carries a positive control in the same
test: the identical environment, switch flipped, must produce exactly one
request. ``GrimRunner`` builds its environment from scratch, so nothing from
the developer's shell (``OTEL_*``, ``CI``, ``DO_NOT_TRACK``) leaks in.
"""
from __future__ import annotations

import platform
import re
import subprocess
import sys
import time
from pathlib import Path

import pytest

from src.otlp_fixture import (
    LOGS_PATH,
    REDIRECT_TARGET,
    OtlpCollector,
    closed_port,
    serve_collector,
    telemetry_payload,
    telemetry_payload_lines,
)
from src.runner import GrimRunner

# Headers reqwest/hyper add themselves; everything else on the wire is grim's.
TRANSPORT_HEADERS = {"host", "content-length", "accept"}
UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
WINDOWS = sys.platform == "win32"


@pytest.fixture()
def runner(grim_at, project_dir: Path) -> GrimRunner:
    (project_dir / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    return grim_at(project_dir)


def arm(runner: GrimRunner, collector: OtlpCollector, **extra: str) -> None:
    runner.env.update(
        {"GRIM_TELEMETRY": "1", "OTEL_EXPORTER_OTLP_ENDPOINT": collector.endpoint, **extra}
    )


def go(runner: GrimRunner, *args: str, debug: bool = False) -> subprocess.CompletedProcess[str]:
    """A cheap, network-free command; a hang fails the test instead of stalling it."""
    if debug:
        runner.env["GRIM_LOG"] = "debug"
    return runner.run(*(args or ("context",)), timeout=10)


def wire_headers(collector: OtlpCollector, nth: int = 0) -> dict[str, str]:
    return {k.lower(): v for k, v in collector.log_posts[nth].headers.items()}


def telemetry_dir(runner: GrimRunner) -> Path:
    return runner.grim_home / "telemetry"


def test_unarmed_sends_nothing_with_ambient_otel(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    runner.env.update(
        {
            "OTEL_EXPORTER_OTLP_ENDPOINT": otlp_collector.endpoint,
            "OTEL_EXPORTER_OTLP_HEADERS": "authorization=Bearer%20ambient",
            "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
            "OTEL_RESOURCE_ATTRIBUTES": "a=b",
            "OTEL_SERVICE_NAME": "other-tool",
        }
    )
    result = go(runner, debug=True)
    assert result.returncode == 0
    assert otlp_collector.requests == []
    assert not telemetry_dir(runner).exists()
    assert telemetry_payload_lines(result.stderr) == []

    runner.env["GRIM_TELEMETRY"] = "1"  # positive control: only the switch differs
    go(runner)
    assert len(otlp_collector.log_posts) == 1


@pytest.mark.parametrize(
    "disarm",
    [
        pytest.param({"OTEL_SDK_DISABLED": "true"}, id="OTEL_SDK_DISABLED"),
        pytest.param({"DO_NOT_TRACK": "1"}, id="DO_NOT_TRACK"),
        pytest.param({"GRIM_OFFLINE": "1"}, id="GRIM_OFFLINE"),
        pytest.param("--offline", id="--offline"),
    ],
)
def test_disarm_switches_win(
    runner: GrimRunner, otlp_collector: OtlpCollector, disarm: dict[str, str] | str
) -> None:
    arm(runner, otlp_collector)
    args = ["context"]
    if isinstance(disarm, dict):
        runner.env.update(disarm)
    else:
        args = [disarm, "context"]
    result = go(runner, *args, debug=True)
    assert result.returncode == 0
    assert otlp_collector.requests == []
    assert not telemetry_dir(runner).exists()
    assert telemetry_payload_lines(result.stderr) == []

    for key in disarm if isinstance(disarm, dict) else ():  # positive control
        del runner.env[key]
    go(runner)
    assert len(otlp_collector.log_posts) == 1


def test_base_endpoint_gets_generic_headers(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    arm(runner, otlp_collector, OTEL_EXPORTER_OTLP_HEADERS="authorization=Bearer%20tok,X-Team=a=b")
    go(runner)
    headers = wire_headers(otlp_collector)
    assert headers["authorization"] == "Bearer tok"
    assert headers["x-team"] == "a=b"
    assert headers["content-type"] == "application/x-protobuf"
    version = otlp_collector.exports[0]["resource"]["service.version"]
    assert headers["user-agent"] == f"grim/{version}"
    assert set(headers) <= TRANSPORT_HEADERS | {
        "content-type",
        "user-agent",
        "authorization",
        "x-team",
    }, sorted(headers)


def test_generic_headers_cannot_replace_the_content_type(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    arm(runner, otlp_collector, OTEL_EXPORTER_OTLP_HEADERS="content-type=text/evil,x-team=a")
    go(runner)
    headers = wire_headers(otlp_collector)
    assert headers["content-type"] == "application/x-protobuf"
    assert headers["x-team"] == "a"


def test_signal_endpoint_never_gets_generic_headers(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    runner.env.update(
        {
            "GRIM_TELEMETRY": "1",
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT": otlp_collector.url(LOGS_PATH),
            "OTEL_EXPORTER_OTLP_HEADERS": "authorization=Bearer%20LEAK",
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS": "x-ok=1",
        }
    )
    go(runner)
    headers = wire_headers(otlp_collector, 0)
    assert headers["x-ok"] == "1"
    assert "authorization" not in headers
    assert "LEAK" not in "".join(headers.values())
    assert set(headers) <= TRANSPORT_HEADERS | {"content-type", "user-agent", "x-ok"}, sorted(headers)

    # Only the generic variable set: the crate would merge it into the request
    # itself, so its absence on the wire proves grim's wrapper strips it.
    del runner.env["OTEL_EXPORTER_OTLP_LOGS_HEADERS"]
    go(runner)
    headers = wire_headers(otlp_collector, 1)
    assert set(headers) <= TRANSPORT_HEADERS | {"content-type", "user-agent"}, sorted(headers)
    assert "LEAK" not in "".join(headers.values())


def test_no_endpoint_sends_nothing(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    runner.env.update(
        {"GRIM_TELEMETRY": "1", "OTEL_EXPORTER_OTLP_HEADERS": "authorization=Bearer%20x"}
    )
    result = go(runner, debug=True)
    assert result.returncode == 0
    assert otlp_collector.requests == []
    assert len(telemetry_payload_lines(result.stderr)) == 1
    assert "not sent" in result.stderr

    runner.env["OTEL_EXPORTER_OTLP_ENDPOINT"] = otlp_collector.endpoint  # positive control
    go(runner)
    assert len(otlp_collector.log_posts) == 1


def test_redirect_is_not_followed(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    otlp_collector.mode = "redirect"
    arm(runner, otlp_collector)
    result = go(runner)
    assert result.returncode == 0
    # The POST arrived and was answered 307, so a following client would have
    # produced a request to the target.
    assert len(otlp_collector.requests_to(LOGS_PATH)) == 1
    assert otlp_collector.requests_to(REDIRECT_TARGET) == []


@pytest.mark.parametrize("protocol", ["grpc", "http/json"])
def test_unsupported_protocol_sends_nothing(
    runner: GrimRunner, otlp_collector: OtlpCollector, protocol: str
) -> None:
    arm(runner, otlp_collector, OTEL_EXPORTER_OTLP_PROTOCOL=protocol)
    result = go(runner, debug=True)
    assert result.returncode == 0
    assert otlp_collector.requests == []
    assert len(telemetry_payload_lines(result.stderr)) == 1
    skips = [ln for ln in result.stderr.splitlines() if "not sent" in ln]
    assert len(skips) == 1, result.stderr

    runner.env["OTEL_EXPORTER_OTLP_PROTOCOL"] = "http/protobuf"  # positive control
    go(runner)
    assert len(otlp_collector.log_posts) == 1


def test_compression_gzip_is_ignored(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    arm(runner, otlp_collector, OTEL_EXPORTER_OTLP_COMPRESSION="gzip")
    go(runner)
    assert "content-encoding" not in wire_headers(otlp_collector)
    assert len(otlp_collector.exports) == 1  # decoded as plain protobuf


@pytest.mark.parametrize("otlp_collector", ["status=503"], indirect=True)
def test_no_retry_on_503(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    arm(runner, otlp_collector, OTEL_EXPORTER_OTLP_TIMEOUT="3000")
    start = time.monotonic()
    result = go(runner)
    elapsed = time.monotonic() - start
    assert result.returncode == 0
    assert len(otlp_collector.requests) == 1
    assert elapsed < 2.5, f"a retry/backoff would show here: {elapsed:.2f}s"


def _expected_os() -> str:
    return {"linux": "linux", "darwin": "darwin", "win32": "windows"}.get(sys.platform, sys.platform)


def _expected_arch() -> str:
    machine = platform.machine().lower()
    return {"x86_64": "amd64", "aarch64": "arm64"}.get(machine, machine)


def test_resource_attributes_exact(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    arm(runner, otlp_collector)
    go(runner)
    (export,) = otlp_collector.exports
    resource = export["resource"]
    assert set(resource) == {
        "service.name",
        "service.version",
        "os.type",
        "host.arch",
        "grim.installation.id",
    }, resource
    assert resource["service.name"] == "grim"
    assert re.fullmatch(r"\d+\.\d+\.\d+.*", resource["service.version"])
    assert resource["os.type"] == _expected_os()
    assert resource["host.arch"] == _expected_arch()
    assert UUID.match(resource["grim.installation.id"]), resource
    assert export["scope"] == {"name": "grim", "version": resource["service.version"]}


def test_otel_resource_attributes_and_service_name_override(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    arm(
        runner,
        otlp_collector,
        OTEL_RESOURCE_ATTRIBUTES="deployment.environment=prod%20eu,service.name=from-attrs,"
        "grim.installation.id=fixed-1,junk,=novalue, k = a=b ",
    )
    go(runner)
    resource = otlp_collector.exports[-1]["resource"]
    assert resource["service.name"] == "from-attrs"  # attributes beat the default
    assert resource["deployment.environment"] == "prod eu"  # percent-decoded
    assert resource["grim.installation.id"] == "fixed-1"  # operator wins over derived
    assert resource["k"] == "a=b"
    assert "junk" not in resource and "" not in resource

    runner.env["OTEL_SERVICE_NAME"] = "svc"  # beats the attribute
    go(runner)
    assert otlp_collector.exports[-1]["resource"]["service.name"] == "svc"
    assert len(otlp_collector.exports) == 2


def test_installation_id_absent_when_unarmed(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    runner.env["OTEL_EXPORTER_OTLP_ENDPOINT"] = otlp_collector.endpoint
    go(runner)
    assert not telemetry_dir(runner).exists()

    runner.env["GRIM_TELEMETRY"] = "1"  # positive control
    go(runner)
    assert (telemetry_dir(runner) / "installation-id").is_file()


def test_installation_id_stable_across_armed_runs(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    arm(runner, otlp_collector)
    go(runner)
    go(runner)
    first, second = (e["resource"]["grim.installation.id"] for e in otlp_collector.exports)
    assert UUID.match(first)
    assert first == second
    assert (telemetry_dir(runner) / "installation-id").read_text().strip() == first


def test_debug_line_matches_wire(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    arm(runner, otlp_collector, OTEL_SERVICE_NAME="audit")
    result = go(runner, debug=True)
    line = telemetry_payload(result.stderr)
    (export,) = otlp_collector.exports
    assert line["resource"] == export["resource"]
    assert [e["name"] for e in line["events"]] == [r["event_name"] for r in export["records"]]
    assert [e["attributes"] for e in line["events"]] == [r["attributes"] for r in export["records"]]
    assert export["records"][0]["event_name"] == "grim.command"


def test_debug_line_never_carries_header_values(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    secrets = ["SECRETVALUE", "OTHERSECRET", "QUERYSECRET"]
    arm(
        runner,
        otlp_collector,
        OTEL_EXPORTER_OTLP_HEADERS="x-secret-name=SECRETVALUE,bad key=OTHERSECRET,good=SECRETVALUE",
    )
    result = go(runner, debug=True)
    assert len(otlp_collector.log_posts) == 1  # positive control: the secrets were sent
    for text in secrets[:2] + ["x-secret-name"]:
        assert text not in result.stderr, result.stderr

    # The failure line names a redacted endpoint, never headers or query.
    runner.env.pop("OTEL_EXPORTER_OTLP_ENDPOINT")
    runner.env["OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"] = (
        f"http://127.0.0.1:{closed_port()}/v1/logs?token=QUERYSECRET"
    )
    result = go(runner, debug=True)
    assert "failed" in result.stderr, result.stderr
    for text in secrets + ["x-secret-name"]:
        assert text not in result.stderr, result.stderr


def _failing(runner: GrimRunner) -> subprocess.CompletedProcess[str]:
    """`grim install` with no grimoire.toml anywhere above the cwd: exit 79."""
    return runner.run("install", check=False, timeout=10)


def _ok(runner: GrimRunner) -> subprocess.CompletedProcess[str]:
    return runner.run("context", check=False, timeout=10)


@pytest.mark.parametrize(
    ("command", "expected_exit"), [(_ok, 0), (_failing, 79)], ids=["success", "exit-79"]
)
@pytest.mark.parametrize("failure", ["refused", "503", "hang"])
def test_export_failure_never_changes_outcome(
    grim_at, tmp_path: Path, failure: str, command, expected_exit: int
) -> None:
    workdir = tmp_path / "work"
    workdir.mkdir()
    if command is _ok:
        (workdir / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    started = time.monotonic()
    baseline = command(grim_at(workdir))
    baseline_elapsed = time.monotonic() - started
    assert baseline.returncode == expected_exit, baseline.stderr

    mode = {"503": "status=503", "hang": "hang"}.get(failure, "ok")
    with serve_collector(mode) as collector:
        runner = grim_at(workdir)
        endpoint = f"http://127.0.0.1:{closed_port()}" if failure == "refused" else collector.endpoint
        runner.env.update(
            {
                "GRIM_TELEMETRY": "1",
                "OTEL_EXPORTER_OTLP_ENDPOINT": endpoint,
                "OTEL_EXPORTER_OTLP_TIMEOUT": "300",
            }
        )
        start = time.monotonic()
        armed = command(runner)
        elapsed = time.monotonic() - start
        if failure != "refused":  # positive control: the export was attempted
            assert len(collector.requests_to(LOGS_PATH)) == 1
    assert armed.returncode == baseline.returncode
    assert armed.stderr == baseline.stderr
    assert armed.stdout == baseline.stdout
    added = elapsed - baseline_elapsed
    assert added < 0.3 + 1.5, f"export failure stretched the run by {added:.2f}s"


def test_one_post_per_invocation(runner: GrimRunner, otlp_collector: OtlpCollector) -> None:
    arm(runner, otlp_collector)
    go(runner)
    assert [(r.method, r.path) for r in otlp_collector.requests] == [("POST", LOGS_PATH)]
    assert otlp_collector.requests_to("/v1/traces") == []
    assert otlp_collector.requests_to("/v1/metrics") == []
    assert wire_headers(otlp_collector)["content-type"] == "application/x-protobuf"
    (export,) = otlp_collector.exports
    assert (export["resource_logs"], export["scope_logs"]) == (1, 1)
    assert export["scope"]["name"] == "grim"
    assert export["records"] and export["records"][0]["event_name"] == "grim.command"
