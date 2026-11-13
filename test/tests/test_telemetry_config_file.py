# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Opt-in usage telemetry — configuration from ``GRIM_CONFIG`` TOML files.

Every negative ("nothing is sent") test carries a positive control in the same
test: the identical environment with the one difference flipped must produce
exactly one request. ``GrimRunner`` builds its environment from scratch, so
nothing from the developer's shell leaks in. Warnings are matched by
substring: the tracing prefix in front of them is not part of the contract.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from src.otlp_fixture import (
    LOGS_PATH,
    OtlpCollector,
    serve_collector,
    telemetry_payload,
)
from src.runner import GrimRunner

OFF = "telemetry is off for this run"
UNUSED_HEADERS = (
    "telemetry headers from GRIM_CONFIG or GRIM_TELEMETRY_OTLP_HEADERS are not sent: "
    "no GRIM_CONFIG file sets an endpoint"
)
IGNORED = "ignored GRIM_CONFIG keys in"
SENTINEL = "SENTINEL-c0ffee"


@pytest.fixture()
def runner(grim_at, project_dir: Path) -> GrimRunner:
    (project_dir / "grimoire.toml").write_text(
        "[skills]\n\n[rules]\n", encoding="utf-8", newline="\n"
    )
    return grim_at(project_dir)


def go(runner: GrimRunner, *args: str) -> subprocess.CompletedProcess[str]:
    """A cheap, network-free command; a hang fails the test instead of stalling it."""
    return runner.run(*(args or ("context",)), timeout=10)


def wire_headers(collector: OtlpCollector, nth: int = 0) -> dict[str, str]:
    return {k.lower(): v for k, v in collector.log_posts[nth].headers.items()}


def q(text: str) -> str:
    return json.dumps(text)


def write_config(
    path: Path,
    *,
    endpoint: str | None = None,
    enabled: bool | None = True,
    headers: dict[str, str] | None = None,
    inline: tuple[str, ...] = (),
    tail: str = "",
) -> Path:
    """A GRIM_CONFIG file: ``inline`` lines go inside ``[telemetry]``, ``tail`` after it."""
    lines = ["[telemetry]"]
    if enabled is not None:
        lines.append(f"enabled = {str(enabled).lower()}")
    if endpoint is not None:
        lines.append(f"endpoint = {q(endpoint)}")
    lines.extend(inline)
    if headers:
        lines.append("[telemetry.headers]")
        lines.extend(f"{q(k)} = {q(v)}" for k, v in headers.items())
    text = "\n".join(lines) + "\n" + tail
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    return path


def snapshot(root: Path) -> dict[str, bytes | None]:
    return {
        p.relative_to(root).as_posix(): p.read_bytes() if p.is_file() else None
        for p in root.rglob("*")
    }


OPT_OUTS = [
    pytest.param({"DO_NOT_TRACK": "1"}, (), id="DO_NOT_TRACK"),
    pytest.param({"OTEL_SDK_DISABLED": "true"}, (), id="OTEL_SDK_DISABLED"),
    pytest.param({"GRIM_OFFLINE": "1"}, (), id="GRIM_OFFLINE"),
    pytest.param({}, ("--offline",), id="--offline"),
    pytest.param({"GRIM_TELEMETRY": "0"}, (), id="GRIM_TELEMETRY=0"),
]


def test_grim_config_alone_arms_and_exports(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    cfg = write_config(
        tmp_path / "org" / "telemetry.toml",
        endpoint=otlp_collector.endpoint,
        headers={"authorization": "Bearer org-token-fake"},
    )
    runner.env["GRIM_CONFIG"] = str(cfg)
    cwd = runner.cwd
    assert cwd is not None
    project_before = snapshot(cwd)
    assert snapshot(runner.grim_home) == {}

    result = go(runner)
    assert result.returncode == 0
    assert len(otlp_collector.log_posts) == 1
    assert otlp_collector.log_posts[0].path == LOGS_PATH
    assert wire_headers(otlp_collector)["authorization"] == "Bearer org-token-fake"
    assert "GRIM_CONFIG" not in result.stderr
    assert OFF not in result.stderr
    assert snapshot(cwd) == project_before

    twin_home = tmp_path / "twin-home"
    twin_home.mkdir()
    twin = GrimRunner(runner.binary, twin_home, cwd=cwd)
    twin.env.update(
        {"GRIM_TELEMETRY": "1", "OTEL_EXPORTER_OTLP_ENDPOINT": otlp_collector.endpoint}
    )
    go(twin)
    assert len(otlp_collector.log_posts) == 2
    assert snapshot(twin_home)
    assert set(snapshot(runner.grim_home)) == set(snapshot(twin_home))


def test_grim_config_endpoint_gets_only_grim_headers(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    cfg = write_config(
        tmp_path / "org" / "telemetry.toml",
        endpoint=otlp_collector.endpoint,
        headers={"authorization": "Bearer org-token-fake"},
    )
    otlp_collector.mode = "delay=100"
    with serve_collector() as other:
        runner.env.update(
            {
                "GRIM_LOG": "debug",
                "GRIM_CONFIG": str(cfg),
                "OTEL_EXPORTER_OTLP_ENDPOINT": other.endpoint,
                "OTEL_EXPORTER_OTLP_HEADERS": "x-ambient=1",
                "OTEL_EXPORTER_OTLP_PROTOCOL": "grpc",
                "OTEL_EXPORTER_OTLP_TIMEOUT": "1",
                "OTEL_EXPORTER_OTLP_LOGS_TIMEOUT": "1",
            }
        )
        result = go(runner)
        # the file's 500 ms default applies, so the 100 ms delay still succeeds
        assert not any(
            "telemetry export to" in ln and "failed" in ln
            for ln in result.stderr.splitlines()
        )
        assert len(otlp_collector.log_posts) == 1
        headers = wire_headers(otlp_collector)
        assert headers["authorization"] == "Bearer org-token-fake"
        assert "x-ambient" not in headers
        assert other.requests == []


def test_grim_telemetry_otlp_headers_supply_token(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    cfg = write_config(
        tmp_path / "org" / "telemetry.toml", endpoint=otlp_collector.endpoint
    )
    runner.env.update(
        {
            "GRIM_CONFIG": str(cfg),
            "GRIM_TELEMETRY_OTLP_HEADERS": "authorization=Bearer%20tok-fake",
        }
    )
    go(runner)
    assert wire_headers(otlp_collector, 0)["authorization"] == "Bearer tok-fake"

    cfg = write_config(
        tmp_path / "org" / "telemetry.toml",
        endpoint=otlp_collector.endpoint,
        headers={"authorization": "Bearer file-fake", "x-team": "a"},
    )
    runner.env["GRIM_TELEMETRY_OTLP_HEADERS"] = "Authorization=Bearer%20env-fake"
    go(runner)
    headers = wire_headers(otlp_collector, 1)
    assert headers["authorization"] == "Bearer env-fake"
    assert headers["x-team"] == "a"


@pytest.mark.parametrize(
    "with_otel_endpoint", [True, False], ids=["otel-endpoint", "no-endpoint"]
)
def test_file_without_endpoint_uses_otel_endpoint(
    runner: GrimRunner,
    otlp_collector: OtlpCollector,
    tmp_path: Path,
    with_otel_endpoint: bool,
) -> None:
    cfg = write_config(tmp_path / "org" / "telemetry.toml", headers={"x-file": "1"})
    runner.env.update(
        {
            "GRIM_CONFIG": str(cfg),
            "OTEL_EXPORTER_OTLP_HEADERS": "x-otel=1",
            "GRIM_TELEMETRY_OTLP_HEADERS": "x-grim=1",
        }
    )
    if not with_otel_endpoint:
        result = go(runner)
        assert result.returncode == 0
        assert otlp_collector.requests == []

    runner.env["OTEL_EXPORTER_OTLP_ENDPOINT"] = otlp_collector.endpoint
    result = go(runner)
    assert len(otlp_collector.log_posts) == 1
    headers = wire_headers(otlp_collector)
    assert headers["x-otel"] == "1"
    assert "x-file" not in headers
    assert "x-grim" not in headers
    assert result.stderr.count(UNUSED_HEADERS) == 1


@pytest.mark.parametrize(("optout_env", "optout_args"), OPT_OUTS)
def test_grim_config_opt_outs_win(
    runner: GrimRunner,
    otlp_collector: OtlpCollector,
    tmp_path: Path,
    optout_env: dict[str, str],
    optout_args: tuple[str, ...],
) -> None:
    cfg = write_config(
        tmp_path / "org" / "telemetry.toml", endpoint=otlp_collector.endpoint
    )
    runner.env["GRIM_CONFIG"] = str(cfg)
    runner.env.update(optout_env)
    result = go(runner, *optout_args, "context")
    assert result.returncode == 0
    assert otlp_collector.requests == []

    for key in optout_env:  # positive control
        del runner.env[key]
    go(runner)
    assert len(otlp_collector.log_posts) == 1


@pytest.mark.parametrize(("optout_env", "optout_args"), OPT_OUTS)
def test_opted_out_run_never_opens_grim_config(
    runner: GrimRunner,
    otlp_collector: OtlpCollector,
    tmp_path: Path,
    optout_env: dict[str, str],
    optout_args: tuple[str, ...],
) -> None:
    runner.env["GRIM_CONFIG"] = str(tmp_path / "missing" / "telemetry.toml")
    runner.env["OTEL_EXPORTER_OTLP_ENDPOINT"] = otlp_collector.endpoint
    runner.env.update(optout_env)
    result = go(runner, *optout_args, "context")
    assert "GRIM_CONFIG" not in result.stderr
    assert otlp_collector.requests == []

    for key in optout_env:  # positive control
        del runner.env[key]
    result = go(runner)
    assert result.stderr.count(OFF) == 1
    assert otlp_collector.requests == []


def test_later_grim_config_file_wins(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    org = write_config(tmp_path / "org.toml", endpoint=otlp_collector.endpoint)
    with serve_collector() as b:
        user = write_config(tmp_path / "user.toml", enabled=None, endpoint=b.endpoint)
        runner.env["GRIM_CONFIG"] = os.pathsep.join([str(org), str(user)])
        go(runner)
        assert otlp_collector.requests == []
        assert len(b.log_posts) == 1

        user = write_config(tmp_path / "user.toml", enabled=False)
        go(runner)
        assert otlp_collector.requests == []
        assert len(b.log_posts) == 1

        runner.env["GRIM_TELEMETRY"] = "1"  # the switch outranks the file's false
        go(runner)
        assert len(otlp_collector.log_posts) == 1
        assert len(b.log_posts) == 1


def broken_cases(tmp_path: Path) -> dict[str, tuple[Path, list[str]]]:
    parse = tmp_path / "parse.toml"
    parse.write_text(
        f'[telemetry.headers]\nauthorization = "Bearer {SENTINEL}\n',
        encoding="utf-8",
        newline="\n",
    )
    directory = tmp_path / "a-directory"
    directory.mkdir()
    big = tmp_path / "big.toml"
    with open(big, "wb") as f:
        f.truncate(8 * 1024 * 1024 + 1)

    def bad(name: str, body: str, key: str) -> tuple[Path, list[str]]:
        p = tmp_path / name
        p.write_text(body, encoding="utf-8", newline="\n")
        return p, [f"GRIM_CONFIG file '{p}' has an unusable value for '{key}'"]

    return {
        "parse": (
            parse,
            [f"GRIM_CONFIG file '{parse}' is not valid TOML (line", "column"],
        ),
        "missing": (
            tmp_path / "missing.toml",
            [f"cannot read GRIM_CONFIG file '{tmp_path / 'missing.toml'}'"],
        ),
        "directory": (
            directory,
            [f"GRIM_CONFIG file '{directory}' is not a regular file"],
        ),
        "oversized": (big, [f"GRIM_CONFIG file '{big}' is larger than"]),
        "timeout": bad(
            "timeout.toml", '[telemetry]\ntimeout = "fast"\n', "telemetry.timeout"
        ),
        "protocol": bad(
            "protocol.toml", '[telemetry]\nprotocol = "grpc"\n', "telemetry.protocol"
        ),
        "table": bad("table.toml", "telemetry = 1\n", "telemetry"),
    }


@pytest.mark.parametrize(
    "case",
    ["parse", "missing", "directory", "oversized", "timeout", "protocol", "table"],
)
def test_broken_grim_config_file_disarms_quietly(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path, case: str
) -> None:
    path, expected = broken_cases(tmp_path)[case]
    runner.env.update(
        {"GRIM_TELEMETRY": "1", "OTEL_EXPORTER_OTLP_ENDPOINT": otlp_collector.endpoint}
    )
    control = go(runner)
    assert len(otlp_collector.log_posts) == 1

    runner.env["GRIM_CONFIG"] = str(path)
    result = go(runner)
    assert len(otlp_collector.log_posts) == 1  # nothing new
    for text in expected:
        assert text in result.stderr, text
    assert str(path) in result.stderr
    assert result.stderr.count(OFF) == 1
    assert SENTINEL not in result.stderr
    assert result.returncode == control.returncode
    assert result.stdout == control.stdout


def test_relative_grim_config_entry_is_refused(
    runner: GrimRunner, otlp_collector: OtlpCollector
) -> None:
    cwd = runner.cwd
    assert cwd is not None
    cfg = write_config(cwd / "telemetry.toml", endpoint=otlp_collector.endpoint)
    runner.env["GRIM_CONFIG"] = "telemetry.toml"
    result = go(runner)
    assert "GRIM_CONFIG entry 'telemetry.toml' is not an absolute path" in result.stderr
    assert OFF in result.stderr
    assert otlp_collector.requests == []

    runner.env["GRIM_CONFIG"] = str(cfg)  # positive control
    go(runner)
    assert len(otlp_collector.log_posts) == 1


def test_endpoint_userinfo_in_file_is_refused(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    userinfo = f"http://user:secret-fake@127.0.0.1:{otlp_collector.port}"
    cfg = write_config(tmp_path / "org.toml", endpoint=userinfo)
    runner.env["GRIM_CONFIG"] = str(cfg)
    result = go(runner)
    assert "telemetry.endpoint" in result.stderr
    assert "secret-fake" not in result.stderr
    assert "user:" not in result.stderr
    assert otlp_collector.requests == []

    write_config(cfg, endpoint=otlp_collector.endpoint)  # positive control
    go(runner)
    assert len(otlp_collector.log_posts) == 1


def test_other_tables_warn_once(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    cfg = write_config(
        tmp_path / "org.toml",
        endpoint=otlp_collector.endpoint,
        inline=('compression = "gzip"',),
        tail='\n[options]\nx = 1\n\n[[registries]]\nurl = "registry.example"\n',
    )
    runner.env["GRIM_CONFIG"] = str(cfg)
    result = go(runner)
    assert len(otlp_collector.log_posts) == 1
    assert result.stderr.count(IGNORED) == 1
    line = next(ln for ln in result.stderr.splitlines() if IGNORED in ln)
    for text in (str(cfg), "options", "registries", "telemetry.compression"):
        assert text in line, text

    broken = tmp_path / "broken.toml"
    broken.write_text(
        '[options]\nx = 1\n\n[telemetry]\ntimeout = "fast"\n',
        encoding="utf-8",
        newline="\n",
    )
    runner.env["GRIM_CONFIG"] = str(broken)
    result = go(runner)
    assert OFF in result.stderr
    assert "telemetry.timeout" in result.stderr
    assert IGNORED not in result.stderr
    assert len(otlp_collector.log_posts) == 1  # nothing new


def test_grim_config_secrets_never_printed(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    secrets = ["Bearer file-secret-fake", "env-secret-fake", "tok-secret-fake"]
    cfg = write_config(
        tmp_path / "org.toml",
        endpoint=otlp_collector.endpoint,
        headers={"authorization": secrets[0]},
    )
    runner.env.update(
        {
            "GRIM_CONFIG": str(cfg),
            "GRIM_TELEMETRY_OTLP_HEADERS": f"x-env={secrets[1]},x-tok={secrets[2]}",
            "GRIM_LOG": "trace",
        }
    )
    result = go(runner)
    assert len(otlp_collector.log_posts) == 1
    telemetry_payload(result.stderr)  # the debug payload line is present
    for secret in secrets:
        assert secret not in result.stderr

    listing = go(runner, "config", "list", "--format", "json")
    assert listing.returncode == 0
    for secret in secrets:
        assert secret not in listing.stdout
        assert secret not in listing.stderr

    broken = tmp_path / "broken.toml"
    broken.write_text(
        f'[telemetry.headers]\nauthorization = "Bearer {SENTINEL}\n',
        encoding="utf-8",
        newline="\n",
    )
    runner.env["GRIM_CONFIG"] = str(broken)
    result = go(runner)
    assert "is not valid TOML" in result.stderr
    assert SENTINEL not in result.stderr


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX FIFOs")
@pytest.mark.parametrize("held_open", [False, True], ids=["no-writer", "held-open"])
def test_fifo_grim_config_entry_never_blocks(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path, held_open: bool
) -> None:
    fifo = tmp_path / "telemetry.fifo"
    os.mkfifo(fifo)
    runner.env.update(
        {"GRIM_TELEMETRY": "1", "OTEL_EXPORTER_OTLP_ENDPOINT": otlp_collector.endpoint}
    )
    control = go(runner)
    assert len(otlp_collector.log_posts) == 1

    runner.env["GRIM_CONFIG"] = str(fifo)
    fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK) if held_open else None
    try:
        result = go(runner)
    finally:
        if fd is not None:
            os.close(fd)
    assert f"GRIM_CONFIG file '{fifo}' is not a regular file" in result.stderr
    assert result.returncode == control.returncode
    assert len(otlp_collector.log_posts) == 1


def test_file_resource_beats_ambient_service_name(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    cfg = write_config(
        tmp_path / "org.toml",
        endpoint=otlp_collector.endpoint,
        tail='\n[telemetry.resource_attributes]\n"service.name" = "from-file"\n',
    )
    runner.env.update({"GRIM_CONFIG": str(cfg), "OTEL_SERVICE_NAME": "ambient"})
    go(runner)
    assert otlp_collector.exports[0]["resource"]["service.name"] == "from-file"

    write_config(
        cfg,
        endpoint=otlp_collector.endpoint,
        inline=('service_name = "file-svc"',),
        tail='\n[telemetry.resource_attributes]\n"service.name" = "from-file"\n',
    )
    go(runner)
    assert otlp_collector.exports[1]["resource"]["service.name"] == "file-svc"

    write_config(
        cfg,
        enabled=True,
        tail='\n[telemetry.resource_attributes]\n"service.name" = "from-file"\n',
    )
    runner.env["OTEL_EXPORTER_OTLP_ENDPOINT"] = otlp_collector.endpoint
    go(runner)
    assert otlp_collector.exports[2]["resource"]["service.name"] == "from-file"


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX env values are bytes")
def test_non_unicode_grim_telemetry_is_an_explicit_off(
    runner: GrimRunner, otlp_collector: OtlpCollector, tmp_path: Path
) -> None:
    cfg = write_config(tmp_path / "org.toml", endpoint=otlp_collector.endpoint)
    assert (
        os.fsencode("\udcff") == b"\xff"
    )  # subprocess sends this str as one 0xff byte
    runner.env.update({"GRIM_CONFIG": str(cfg), "GRIM_TELEMETRY": "\udcff"})
    result = go(runner)
    assert otlp_collector.requests == []
    assert "GRIM_CONFIG" not in result.stderr

    del runner.env["GRIM_TELEMETRY"]  # positive control
    go(runner)
    assert len(otlp_collector.log_posts) == 1
