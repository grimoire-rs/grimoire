# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Repository context (`vcs.*`) of the opt-in usage telemetry.

`GRIM_TELEMETRY_REPOSITORY=1` adds the redacted `origin` of the project's git
repository to the resource. The tests read the `GRIM_LOG=debug` payload line,
so no collector is involved: what matters is which attributes grim builds and
whether it ran `git` at all.
"""
from __future__ import annotations

import os
import shutil
import stat
import subprocess
import time
from pathlib import Path

import pytest

from src.otlp_fixture import telemetry_payload
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="git not on PATH")

ORIGIN = "https://user:tok@Host/O/R.git?x=1#f"


def _git(cwd: Path, *args: str) -> None:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, env=env, timeout=10)


def _repo(path: Path, origin: str | None = None) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    _git(path, "init", "-q")
    if origin:
        _git(path, "remote", "add", "origin", origin)
    return path


def _runner(grim_binary: Path, grim_home: Path, cwd: Path, *, repository: bool = True) -> GrimRunner:
    """A runner armed for telemetry with a debug log and no endpoint."""
    runner = GrimRunner(grim_binary, grim_home, cwd=cwd)
    runner.env.update({"GRIM_LOG": "debug", "GRIM_TELEMETRY": "1"})
    if repository:
        runner.env["GRIM_TELEMETRY_REPOSITORY"] = "1"
    return runner


def _resource(runner: GrimRunner, *args: str) -> dict:
    """Resource of the one payload line a `grim status` run logs."""
    result = runner.run(*(args or ("status",)), timeout=10)
    assert result.returncode == 0, result.stderr
    return telemetry_payload(result.stderr)["resource"]


def _init_project(runner: GrimRunner) -> None:
    """Gives the workspace a config so scope resolves to project (not `no-config`)."""
    GrimRunner(runner.binary, runner.grim_home, cwd=runner.cwd).run("init", timeout=10)


def _alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def _has_vcs(resource: dict) -> bool:
    return any(key.startswith("vcs.") for key in resource)


def test_vcs_from_credentialed_origin(grim_binary: Path, grim_home: Path, tmp_path: Path) -> None:
    project = _repo(tmp_path / "project", ORIGIN)
    runner = _runner(grim_binary, grim_home, project)
    _init_project(runner)

    result = runner.run("status", timeout=10)
    resource = telemetry_payload(result.stderr)["resource"]

    assert resource["vcs.repository.url.full"] == "https://host/o/r"
    assert resource["vcs.owner.name"] == "o"
    assert resource["vcs.repository.name"] == "r"
    (line,) = [ln for ln in result.stderr.splitlines() if "telemetry payload" in ln]
    for secret in ("tok", "x=1", "#f", "user:"):
        assert secret not in line, secret
    # Every other stderr line is clean of it too.
    for secret in ("tok@", "x=1"):
        assert secret not in result.stderr


def test_no_vcs_without_the_repository_switch(grim_binary: Path, grim_home: Path, tmp_path: Path) -> None:
    project = _repo(tmp_path / "project", ORIGIN)
    runner = _runner(grim_binary, grim_home, project, repository=False)
    _init_project(runner)

    assert not _has_vcs(_resource(runner))


@pytest.mark.skipif(os.name == "nt", reason="the git shim is a POSIX shell script")
def test_no_vcs_and_no_git_spawn_without_opt_in(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    project = _repo(tmp_path / "project", ORIGIN)
    marker = tmp_path / "git-ran"
    shim_dir = tmp_path / "shim"
    shim_dir.mkdir()
    shim = shim_dir / "git"
    shim.write_text(f'#!/bin/sh\necho spawned >> "{marker}"\nexec "{shutil.which("git")}" "$@"\n')
    shim.chmod(shim.stat().st_mode | stat.S_IXUSR)

    runner = _runner(grim_binary, grim_home, project, repository=False)
    _init_project(runner)
    runner.env["PATH"] = f"{shim_dir}{os.pathsep}{runner.env['PATH']}"

    assert not _has_vcs(_resource(runner))
    assert not marker.exists(), "git was spawned without the repository opt-in"

    # Positive control: the same shim is reached once the opt-in is set.
    runner.env["GRIM_TELEMETRY_REPOSITORY"] = "1"
    assert _has_vcs(_resource(runner))
    assert marker.exists(), "the shim was never reached, so the negative leg proved nothing"


def test_no_vcs_for_global_scope(grim_binary: Path, grim_home: Path, tmp_path: Path) -> None:
    project = _repo(tmp_path / "project", ORIGIN)
    # A global run records `$GRIM_HOME` as its workspace; make that a
    # repository with an origin so only the project-scope gate can hide it.
    _repo(grim_home, "https://github.com/acme/home-repo.git")
    runner = _runner(grim_binary, grim_home, project)
    _init_project(runner)

    payload = telemetry_payload(runner.run("status", "--global", timeout=10).stderr)

    assert payload["events"][0]["attributes"]["grim.scope"] == "global"
    assert not _has_vcs(payload["resource"])

    # Positive leg, same environment: the project command reports the project's repository.
    resource = _resource(runner)
    assert resource["vcs.repository.url.full"] == "https://host/o/r"


def test_no_vcs_for_home_repo(grim_binary: Path, grim_home: Path, tmp_path: Path) -> None:
    probe = GrimRunner(grim_binary, grim_home)
    # HOME itself is the repository; the project lives inside it.
    _repo(probe.home, ORIGIN)
    project = probe.home / "project"
    project.mkdir()
    runner = _runner(grim_binary, grim_home, project)
    _init_project(runner)

    assert not _has_vcs(_resource(runner))

    # Positive leg: a repository nested below HOME is not HOME.
    nested = _repo(probe.home / "work" / "proj", "https://github.com/acme/tool.git")
    nested_runner = _runner(grim_binary, grim_home, nested)
    _init_project(nested_runner)
    resource = _resource(nested_runner)
    assert resource["vcs.repository.url.full"] == "https://github.com/acme/tool"
    assert resource["vcs.provider.name"] == "github"


def test_no_vcs_without_git_on_path(grim_binary: Path, grim_home: Path, tmp_path: Path) -> None:
    project = _repo(tmp_path / "project", ORIGIN)
    empty = tmp_path / "empty-path"
    empty.mkdir()

    armed = _runner(grim_binary, grim_home, project)
    armed.env["PATH"] = str(empty)
    _init_project(armed)
    assert not _has_vcs(_resource(armed))

    # At the default log level the armed run prints what an unarmed one does.
    armed.env.pop("GRIM_LOG")
    unarmed = GrimRunner(grim_binary, grim_home, cwd=project)
    unarmed.env["PATH"] = str(empty)
    armed_run = armed.run("status", timeout=10)
    unarmed_run = unarmed.run("status", timeout=10)
    assert armed_run.stderr == unarmed_run.stderr
    assert armed_run.returncode == unarmed_run.returncode == 0


def test_git_environment_does_not_redirect_the_lookup(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    project = _repo(tmp_path / "project", "https://github.com/real/repo.git")
    decoy = _repo(tmp_path / "decoy", "https://evil.example/decoy/repo.git")
    decoy_config = tmp_path / "decoy.config"
    decoy_config.write_text('[remote "origin"]\n\turl = https://evil.example/decoy/repo.git\n')
    runner = _runner(grim_binary, grim_home, project)
    _init_project(runner)
    # What a git hook exports: a lookup that trusted these would read the decoy.
    runner.env.update(
        {
            "GIT_DIR": str(decoy / ".git"),
            "GIT_WORK_TREE": str(decoy),
            "GIT_COMMON_DIR": str(decoy / ".git"),
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "remote.origin.url",
            "GIT_CONFIG_VALUE_0": "https://evil.example/injected/repo",
            "GIT_CONFIG": str(decoy_config),
        }
    )

    resource = _resource(runner)

    assert resource["vcs.repository.url.full"] == "https://github.com/real/repo"


@pytest.mark.parametrize("variable", ["GIT_CONFIG", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM", "GIT_CONFIG_COUNT"])
def test_git_config_injection_cannot_supply_an_origin(
    variable: str, grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    # No origin in the repository's own config, so any URL reported came from
    # an injected config source.
    project = _repo(tmp_path / "project")
    decoy_config = tmp_path / "decoy.config"
    decoy_config.write_text('[remote "origin"]\n\turl = https://evil.example/decoy/repo.git\n')
    runner = _runner(grim_binary, grim_home, project)
    _init_project(runner)
    if variable == "GIT_CONFIG_COUNT":
        runner.env.update(
            {
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "remote.origin.url",
                "GIT_CONFIG_VALUE_0": "https://evil.example/injected/repo",
            }
        )
    else:
        runner.env[variable] = str(decoy_config)

    assert not _has_vcs(_resource(runner))


@pytest.mark.skipif(os.name == "nt", reason="the git shim is a POSIX shell script")
def test_slow_git_yields_no_vcs_and_is_killed(
    grim_binary: Path, grim_home: Path, tmp_path: Path
) -> None:
    project = _repo(tmp_path / "project", ORIGIN)
    late = tmp_path / "git-finished"
    pid_file = tmp_path / "git-pid"
    shim_dir = tmp_path / "shim"
    shim_dir.mkdir()
    shim = shim_dir / "git"
    shim.write_text(
        f'#!/bin/sh\necho $$ > "{pid_file}"\nsleep 3\necho late >> "{late}"\nexec "{shutil.which("git")}" "$@"\n'
    )
    shim.chmod(shim.stat().st_mode | stat.S_IXUSR)
    runner = _runner(grim_binary, grim_home, project)
    _init_project(runner)
    runner.env["PATH"] = f"{shim_dir}{os.pathsep}{runner.env['PATH']}"

    start = time.monotonic()
    resource = _resource(runner)
    elapsed = time.monotonic() - start

    assert not _has_vcs(resource)
    assert elapsed < 2.5, f"a hung git held the command for {elapsed:.1f}s"
    # The timed-out child was killed: its pid goes away well before the
    # shim's own `sleep 3` would have ended, and it never writes the late marker.
    pid = int(pid_file.read_text())
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline and _alive(pid):
        time.sleep(0.05)
    assert not _alive(pid), "the timed-out git kept running after grim exited"
    assert not late.exists(), "the timed-out git reached the line after `sleep`"
