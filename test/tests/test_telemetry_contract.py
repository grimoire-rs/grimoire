# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Contract-level checks of the opt-in usage telemetry, on the wire.

Each test reads what a capture OTLP collector received, so it proves what
leaves the machine, not what grim meant to build. Every "nothing is sent" or
"nothing is present" assertion carries a positive control in the same test.
``GrimRunner`` builds its environment from scratch, so no ambient ``OTEL_*``,
``GRIM_TELEMETRY*``, ``DO_NOT_TRACK``, ``CI`` or ``GIT_*`` reaches the child.
"""
from __future__ import annotations

import getpass
import os
import re
import shutil
import socket
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from src.helpers import make_artifact, write_config
from src.otlp_fixture import LOGS_PATH, OtlpCollector
from src.runner import GrimRunner
from src.stdio_drivers import HOSTILE_PAYLOAD, ROOT
from src.stdio_drivers import drive as _drive
from src.stdio_drivers import entry as _entry
from src.stdio_drivers import initialize as _initialize
from src.stdio_drivers import payload as _payload
from src.stdio_drivers import write_table as _write_table

COMMAND = "grim.command"
ARTIFACT = "grim.artifact.changed"
TIMEOUT = 60
REPO_ROOT = Path(__file__).resolve().parents[2]
TELEMETRY_DOC = REPO_ROOT / "docs/src/content/docs/telemetry.md"
STABILITY_DOC = REPO_ROOT / "docs/src/content/docs/stability.md"
ORIGIN = "https://github.com/acme/derived.git"

needs_git = pytest.mark.skipif(shutil.which("git") is None, reason="git not on PATH")


def arm(runner: GrimRunner, collector: OtlpCollector, **extra: str) -> GrimRunner:
    runner.env.update(
        {"GRIM_TELEMETRY": "1", "OTEL_EXPORTER_OTLP_ENDPOINT": collector.endpoint, **extra}
    )
    return runner


def git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    out = subprocess.run(
        ["git", "-C", str(cwd), *args],
        check=True,
        capture_output=True,
        text=True,
        env=env,
        timeout=10,
    )
    return out.stdout.strip()


def git_repo(path: Path, origin: str = ORIGIN) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    git(path, "init", "-q")
    git(path, "remote", "add", "origin", origin)
    return path


def init_project(runner: GrimRunner) -> None:
    """A config in the workspace, so scope resolves to project."""
    GrimRunner(runner.binary, runner.grim_home, cwd=runner.cwd).run("init", timeout=10)


def strings_of(value: Any) -> Iterator[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings_of(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            yield from strings_of(key)
            yield from strings_of(item)


def wire_strings(export: dict[str, Any]) -> list[str]:
    """Every string a capture carries: resource, scope, event names, record attributes."""
    found = list(strings_of(export["resource"])) + list(strings_of(export["scope"]))
    for record in export["records"]:
        found.append(record["event_name"])
        found.extend(strings_of(record["attributes"]))
    return found


def skills(project_dir: Path, registry: str, unique_repo: str, *, config_dir: Path | None = None) -> Path:
    a, b = f"{unique_repo}/alpha", f"{unique_repo}/beta"
    make_artifact(a, "skill", {"s/SKILL.md": "a1\n"}, tag="stable")
    make_artifact(b, "skill", {"s/SKILL.md": "b1\n"}, tag="1.0.0")
    target = config_dir or project_dir
    target.mkdir(parents=True, exist_ok=True)
    return write_config(
        target,
        skills={"alpha": f"{registry}/{a}:stable", "beta": f"{registry}/{b}:1.0.0"},
    )


# --------------------------------------------------------------------------- #
# Operator override of derived vcs.* keys
# --------------------------------------------------------------------------- #


@needs_git
def test_operator_vcs_override(
    grim_at, project_dir: Path, otlp_collector: OtlpCollector
) -> None:
    git_repo(project_dir)
    runner = arm(grim_at(project_dir), otlp_collector, GRIM_TELEMETRY_REPOSITORY="1")
    init_project(runner)

    runner.run("status", timeout=10)  # positive control: no override, derived value
    derived = otlp_collector.exports[0]["resource"]
    assert derived["vcs.repository.name"] == "derived"

    runner.env["OTEL_RESOURCE_ATTRIBUTES"] = "vcs.repository.name=override"
    runner.run("status", timeout=10)
    resource = otlp_collector.exports[1]["resource"]

    assert resource["vcs.repository.name"] == "override"
    for key in ("vcs.repository.url.full", "vcs.owner.name", "vcs.provider.name"):
        assert resource[key] == derived[key], key
    assert resource["vcs.owner.name"] == "acme"


# --------------------------------------------------------------------------- #
# Hooks never export
# --------------------------------------------------------------------------- #


@pytest.mark.skipif(os.name == "nt", reason="the hook payload is a POSIX shell script")
def test_hook_run_never_exports(
    grim_at, project_dir: Path, grim_home: Path, tmp_path: Path, otlp_collector: OtlpCollector
) -> None:
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = arm(grim_at(project_dir), otlp_collector)
    handler, marker, payload_dir = _payload(tmp_path, "hit")
    table = _write_table(grim_home, [_entry(id="hit", handler=handler, payload_dir=payload_dir)])

    result = runner.run(
        "hook", "run", "--client", "claude", "--event", "PreToolUse",
        "--table", str(table), "--root", ROOT,
        stdin=HOSTILE_PAYLOAD, check=False, timeout=20,
    )  # fmt: skip

    assert result.returncode == 0, result.stderr
    assert marker.exists(), f"the run never reached dispatch\nstderr: {result.stderr}"
    assert otlp_collector.requests == []
    assert not (grim_home / "telemetry").exists()

    runner.run("status", timeout=10)  # positive control: same env, a normal command
    assert len(otlp_collector.log_posts) == 1


# --------------------------------------------------------------------------- #
# Long-lived commands send one record
# --------------------------------------------------------------------------- #


def test_mcp_session_sends_one_record(
    grim_at, project_dir: Path, otlp_collector: OtlpCollector
) -> None:
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    runner = arm(grim_at(project_dir), otlp_collector)

    responses = _drive(
        runner,
        project_dir,
        [
            _initialize(1),
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "grim_status", "arguments": {}},
            },
        ],
        offline=False,  # `--offline` would disarm the session
    )

    assert responses[2]["result"]["isError"] is False, responses[2]
    posts = otlp_collector.log_posts
    assert len(posts) == 1, [r.path for r in otlp_collector.requests]
    (export,) = otlp_collector.exports
    assert [r["event_name"] for r in export["records"]] == [COMMAND]
    attrs = export["records"][0]["attributes"]
    assert attrs["grim.command.name"] == "mcp"
    assert "grim.scope" not in attrs  # the tool call resolved a scope; the session must not record it
    assert ARTIFACT not in {r["event_name"] for r in export["records"]}


# --------------------------------------------------------------------------- #
# One request for many records
# --------------------------------------------------------------------------- #


def test_multi_record_single_post(
    grim_at, project_dir: Path, registry: str, unique_repo: str, otlp_collector: OtlpCollector
) -> None:
    skills(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    arm(runner, otlp_collector)

    runner.run("install", timeout=TIMEOUT)

    assert [r.path for r in otlp_collector.requests] == [LOGS_PATH]
    (export,) = otlp_collector.exports
    assert export["resource_logs"] == 1
    assert export["scope_logs"] == 1
    assert export["scope"]["name"] == "grim"
    assert [r["event_name"] for r in export["records"]] == [COMMAND, ARTIFACT, ARTIFACT]

    # Positive control: the second install changes nothing, so it sends the
    # command record alone, again as one POST.
    runner.run("install", timeout=TIMEOUT)
    assert len(otlp_collector.log_posts) == 2
    assert [r["event_name"] for r in otlp_collector.exports[1]["records"]] == [COMMAND]


# --------------------------------------------------------------------------- #
# Never collected
# --------------------------------------------------------------------------- #


@needs_git
def test_never_collected(
    grim_at, project_dir: Path, grim_home: Path, registry: str, unique_repo: str,
    otlp_collector: OtlpCollector,
) -> None:
    git_repo(project_dir)
    git(project_dir, "checkout", "-q", "-b", "sentinel-branch")
    git(
        project_dir,
        "-c", "user.name=Sentinel Author", "-c", "user.email=sentinel@example.com",
        "commit", "-q", "--allow-empty", "-m", "sentinel commit",
    )  # fmt: skip
    head = git(project_dir, "rev-parse", "HEAD")
    assert len(head) == 40

    config = skills(project_dir, registry, unique_repo, config_dir=project_dir / "SENTINEL-CFG")
    runner = grim_at(project_dir)
    runner.run("--config", str(config), "lock", timeout=TIMEOUT)
    arm(runner, otlp_collector, GRIM_TELEMETRY_REPOSITORY="1")

    runner.run("--config", str(config), "install", timeout=TIMEOUT)

    (export,) = otlp_collector.exports
    # Positive controls: the capture is the rich one these bans are about.
    assert export["resource"]["vcs.repository.name"] == "derived"
    assert [r["event_name"] for r in export["records"]].count(ARTIFACT) == 2
    found = wire_strings(export)
    assert len(found) > 10

    substrings = [
        str(project_dir), str(project_dir.resolve()), str(grim_home), str(grim_home.resolve()),
        "SENTINEL", "sentinel-branch", "Sentinel Author", head,
    ]  # fmt: skip
    wholes = {socket.gethostname(), getpass.getuser()}
    for value in found:
        for banned in substrings:
            assert banned not in value, f"{banned!r} leaked in {value!r}"
        assert value not in wholes, f"host or user name {value!r} leaked"
        assert "sha256:" not in value, f"artifact digest leaked in {value!r}"


# --------------------------------------------------------------------------- #
# The documented field table is the wire
# --------------------------------------------------------------------------- #


def documented_fields() -> dict[str, dict[str, str]]:
    """``{group: {key: presence}}`` from the ``{#fields}`` section of telemetry.md."""
    text = TELEMETRY_DOC.read_text()
    start = text.index("{#fields}")
    end = text.find("\n## ", start)
    section = text[start : end if end != -1 else len(text)]
    groups: dict[str, dict[str, str]] = {}
    group: str | None = None
    presence_col = 0
    for line in section.splitlines():
        if heading := re.match(r"###\s.*\{#fields-(\w+)\}", line):
            group = heading.group(1)
            groups[group] = {}
        elif group and line.startswith("|"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            if cells[0] == "Key":
                presence_col = cells.index("Presence")
            elif key := re.fullmatch(r"`([^`]+)`", cells[0]):
                groups[group][key.group(1)] = cells[presence_col]
    return groups


@needs_git
def test_wire_keys_match_documented_field_table(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str,
    otlp_collector: OtlpCollector,
) -> None:
    table = documented_fields()
    assert set(table) == {"resource", "command", "artifact"}, sorted(table)
    always = {g: {k for k, p in rows.items() if p.startswith("always")} for g, rows in table.items()}
    assert always["resource"] and always["command"] and always["artifact"]

    git_repo(project_dir)
    skills(project_dir, registry, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", timeout=TIMEOUT)
    arm(runner, otlp_collector)

    runner.run("status", timeout=TIMEOUT)
    runner.run("install", timeout=TIMEOUT)  # artifact events, clients, artifact ref
    runner.run("config", "list", timeout=TIMEOUT)  # grim.command.subcommand
    runner.env["GRIM_TELEMETRY_REPOSITORY"] = "1"
    runner.run("status", timeout=TIMEOUT)  # every vcs.* key, from a github.com origin
    empty = tmp_path / "empty"
    empty.mkdir()
    failing = arm(grim_at(empty), otlp_collector)
    assert failing.run("install", timeout=TIMEOUT, check=False).returncode == 79  # error.type

    exports = otlp_collector.exports
    assert len(exports) == 5
    commands = [r["attributes"] for e in exports for r in e["records"] if r["event_name"] == COMMAND]
    artifacts = [r["attributes"] for e in exports for r in e["records"] if r["event_name"] == ARTIFACT]
    assert len(commands) == 5 and len(artifacts) == 2

    # Both directions: every observed key is documented, and every documented
    # key was observed, so a documented key the wire never carries fails here.
    for group, captures in (
        ("resource", [e["resource"] for e in exports]),
        ("command", commands),
        ("artifact", artifacts),
    ):
        observed: set[str] = set()
        for keys in captures:
            assert set(keys) <= set(table[group]), f"undocumented {group} keys: {set(keys) - set(table[group])}"
            assert always[group] <= set(keys), f"missing always-present {group} keys: {always[group] - set(keys)}"
            observed |= set(keys)
        assert observed == set(table[group]), f"documented {group} keys never sent: {set(table[group]) - observed}"


def test_stability_page_lists_telemetry_as_unstable() -> None:
    text = STABILITY_DOC.read_text()
    start = text.index("{#unstable}")
    end = text.find("\n## ", start)
    unstable = text[start : end if end != -1 else len(text)]
    for needle in ("GRIM_TELEMETRY", "GRIM_TELEMETRY_REPOSITORY", "$GRIM_HOME/telemetry/installation-id"):
        assert needle in unstable, needle
    # Control: the whole-page search would also hit, so the section scoping
    # above is what is being proven; the Frozen section must not carry them.
    frozen = text[text.index("{#frozen}") : start]
    assert "GRIM_TELEMETRY_REPOSITORY" not in frozen
    # The frozen env-var row and the lead-in both carve the switches out.
    assert "except the [telemetry switches]" in frozen
    assert "telemetry switches" in frozen.split("| Area |")[0]
