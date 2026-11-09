# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Where a global settings file may sit, judged by the directory it hangs off.

A vendor variable places a client's global settings file, and a repository can
set that variable, so a global run refuses a relative config directory and one
inside the repository it runs in. Each refusal exits 0, writes nothing, names
its cause once on every run, and reads ``not-armed`` in ``status``; an absolute
directory outside the repository is the positive control. Every run has a
scratch ``HOME``.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from src.helpers import make_settings, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="needs git")

KIRO = {"chat.defaultModel": "m"}
COPILOT = {"model": "m"}
INSIDE = "(settings-root-in-workspace)"
FROM_OUTSIDE = "run grim --global install from outside this repository"


def _package(repo: str) -> PublishedArtifact:
    return make_settings(f"{repo}/team", "1", {"description": "team defaults", "kiro": KIRO, "copilot": COPILOT})


@pytest.fixture()
def repo(tmp_path: Path) -> Path:
    """A git work tree beside the runner's home, initialised under a scratch git config."""
    checkout = tmp_path / "repo"
    checkout.mkdir()
    home = tmp_path / "git-home"
    home.mkdir()
    # An inherited GIT_DIR or GIT_WORK_TREE would point `git init` at the outer repository.
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env |= {"HOME": str(home), "XDG_CONFIG_HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1"}
    subprocess.run(["git", "init", "-q"], cwd=checkout, env=env, check=True, capture_output=True, timeout=60)
    return checkout


def _locked(grim_binary: Path, grim_home: Path, repo: Path, team: PublishedArtifact, *clients: str) -> GrimRunner:
    """A global scope selecting `clients` and locking `team`, run from inside `repo`."""
    options = {"clients": json.dumps(list(clients))}
    write_config(grim_home, settings={"team": team.fq}, options=options, experimental_settings=True)
    runner = GrimRunner(grim_binary, grim_home, cwd=repo)
    runner.run("--global", "lock")
    return runner


def _row(runner: GrimRunner) -> dict:
    [row] = [item for item in runner.json("--global", "status")["items"] if item["name"] == "team"]
    return row


def _causes(row: dict) -> list[tuple[str, str]]:
    return [(entry["client"], entry["cause"]) for entry in row["arming"]]


def _install_row(result: subprocess.CompletedProcess[str]) -> dict:
    [row] = [item for item in json.loads(result.stdout)["items"] if item["name"] == "team"]
    return row


def _moved_inside(runner: GrimRunner, repo: Path, tmp_path: Path) -> Path:
    """Install with ``KIRO_HOME`` outside, then point it at an intact copy inside ``repo``."""
    outside = tmp_path / "kiro-out"
    outside.mkdir()
    runner.env["KIRO_HOME"] = str(outside)
    runner.run("--global", "install")
    planted = repo / "kiro-home" / "settings" / "cli.json"
    planted.parent.mkdir(parents=True)
    planted.write_text(json.dumps(KIRO), encoding="utf-8")
    runner.env["KIRO_HOME"] = str(repo / "kiro-home")
    return planted


def _records(runner: GrimRunner) -> list[dict]:
    state = Path(runner.grim_home) / "state" / "global.json"
    records = json.loads(state.read_text(encoding="utf-8"))["records"] if state.exists() else []
    return [record for record in records if record["kind"] == "settings"]


def test_a_relative_root_is_refused(grim_binary: Path, grim_home: Path, repo: Path, unique_repo: str) -> None:
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    runner.env["KIRO_HOME"] = "kiro-rel"

    result = runner.run("--global", "install", "--client", "kiro")

    assert result.returncode == 0, result.stderr
    assert "(settings-root-relative); set KIRO_HOME to an absolute path, or unset it" in result.stderr
    assert not (repo / "kiro-rel").exists()
    row = _row(runner)
    assert row["state"] == "not-armed"
    assert _causes(row) == [("kiro", "settings-root-relative")]
    assert row["outputs"] == []


def test_a_root_inside_the_repository_is_refused(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    inside = repo / "kiro-home"
    inside.mkdir()
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    runner.env["KIRO_HOME"] = str(inside)

    remedy = (
        "set KIRO_HOME to a path outside this repository, or unset it, then run grim --global install, "
        f"or {FROM_OUTSIDE}"
    )

    result = runner.run("--global", "install", "--client", "kiro")

    assert result.returncode == 0, result.stderr
    assert f"which lies inside the repository this command runs in {INSIDE}; {remedy}" in result.stderr
    assert list(inside.iterdir()) == []
    row = _row(runner)
    assert row["state"] == "not-armed"
    assert _causes(row) == [("kiro", "settings-root-in-workspace")]
    [message] = [entry["message"] for entry in row["arming"]]
    assert message.endswith(f"which lies inside the repository this command runs in; {remedy}")
    assert remedy in runner.plain("--global", "status").stderr

    # Positive control: the same run with the root outside the repository writes.
    outside = tmp_path / "kiro-out"
    outside.mkdir()
    runner.env["KIRO_HOME"] = str(outside)
    result = runner.run("--global", "install", "--client", "kiro")
    assert "settings-root" not in result.stderr
    assert json.loads((outside / "settings" / "cli.json").read_text(encoding="utf-8")) == KIRO
    assert not [cause for _, cause in _causes(_row(runner)) if cause.startswith("settings-root")]


def test_every_run_under_a_refused_root_names_its_cause_once(
    grim_binary: Path, grim_home: Path, repo: Path, unique_repo: str
) -> None:
    """The cause is named on each run, not only on the run that first meets the root."""
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    runner.env["KIRO_HOME"] = "kiro-rel"

    for command in ("install", "install", "update"):
        result = runner.run("--global", command)

        assert result.returncode == 0, result.stderr
        assert result.stderr.count("(settings-root-relative)") == 1, (command, result.stderr)
        assert "(settings-root-relative); set KIRO_HOME to an absolute path, or unset it" in result.stderr
        # The root refusal is the cause, so the nothing-applies note stays out.
        assert "no-applicable-settings" not in result.stderr
    assert not (repo / "kiro-rel").exists()
    row = _row(runner)
    assert row["state"] == "not-armed"
    assert _causes(row) == [("kiro", "settings-root-relative")]


def test_an_intact_value_under_a_refused_root_is_skipped_on_every_run(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A recorded value whose file now lies inside the repository is never reported ``unchanged``."""
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    planted = _moved_inside(runner, repo, tmp_path)

    for _ in range(2):
        result = runner.run("--global", "install", format="json")

        assert result.returncode == 0, result.stderr
        assert _install_row(result)["status"] == "skipped"
        assert result.stderr.count(INSIDE) == 1, result.stderr
    assert json.loads(planted.read_text(encoding="utf-8")) == KIRO
    assert _row(runner)["state"] == "not-armed"


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="needs a FIFO")
def test_a_file_the_fence_refuses_is_never_read(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Install and status answer a refused global file from the fence alone, before any read of it."""
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    planted = _moved_inside(runner, repo, tmp_path)
    planted.unlink()
    os.mkfifo(planted)

    result = runner.run("--global", "install", timeout=30)

    assert result.returncode == 0, result.stderr
    assert result.stderr.count(INSIDE) == 1, result.stderr
    [row] = [item for item in runner.json("--global", "status", timeout=30)["items"] if item["name"] == "team"]
    assert row["state"] == "not-armed"
    assert _causes(row) == [("kiro", "settings-root-in-workspace")]


@pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks")
def test_a_root_linked_into_the_repository_names_the_link(
    grim_binary: Path, grim_home: Path, repo: Path, unique_repo: str
) -> None:
    """No variable put the file inside, so the remedy names none and says where to run from."""
    team = make_settings(f"{unique_repo}/team", "1", {"description": "team defaults", "claude": {"model": "sonnet"}})
    runner = _locked(grim_binary, grim_home, repo, team, "claude")
    dotfiles = repo / "dotfiles" / ".claude"
    dotfiles.mkdir(parents=True)
    (runner.home / ".claude").symlink_to(dotfiles, target_is_directory=True)
    decline = f"which links into the repository this command runs in {INSIDE}; {FROM_OUTSIDE}"

    result = runner.run("--global", "install")

    assert result.returncode == 0, result.stderr
    assert result.stderr.count(INSIDE) == 1, result.stderr
    assert decline in result.stderr
    assert "CLAUDE_CONFIG_DIR" not in result.stderr
    assert list(dotfiles.iterdir()) == []
    row = _row(runner)
    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "settings-root-in-workspace")]
    [message] = [entry["message"] for entry in row["arming"]]
    assert message.endswith(f"which links into the repository this command runs in; {FROM_OUTSIDE}")
    plain = runner.plain("--global", "status").stderr
    assert FROM_OUTSIDE in plain
    assert "CLAUDE_CONFIG_DIR" not in plain


def test_a_dropped_client_the_fence_withholds_is_not_kept_modified(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A narrowing update leaves a fenced client's value recorded, and reports no user edit for it."""
    team = _package(unique_repo)
    both = {"clients": json.dumps(["kiro", "copilot"])}
    write_config(grim_home, settings={"team": team.fq}, options=both, experimental_settings=True)
    runner = GrimRunner(grim_binary, grim_home, cwd=repo)
    copilot = tmp_path / "copilot-out"
    copilot.mkdir()
    runner.env["COPILOT_HOME"] = str(copilot)
    runner.run("--global", "lock")
    planted = _moved_inside(runner, repo, tmp_path)
    narrowed = {"clients": json.dumps(["copilot"])}
    write_config(grim_home, settings={"team": team.fq}, options=narrowed, experimental_settings=True)

    result = runner.run("--global", "update", format="json")

    assert result.returncode == 0, result.stderr
    assert result.stderr.count(INSIDE) == 1, result.stderr
    row = _install_row(result)
    assert (row["reaped_clients"], row["kept_modified_clients"]) == ([], [])
    assert json.loads(planted.read_text(encoding="utf-8")) == KIRO
    assert "kiro" in [output["client"] for record in _records(runner) for output in record["outputs"]]


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="needs a FIFO")
def test_status_never_reads_a_refused_file_of_a_client_no_longer_selected(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A recorded client dropped from the selection is held to the fence too, before any read of its file."""
    team = _package(unique_repo)
    both = {"clients": json.dumps(["kiro", "copilot"])}
    write_config(grim_home, settings={"team": team.fq}, options=both, experimental_settings=True)
    runner = GrimRunner(grim_binary, grim_home, cwd=repo)
    copilot = tmp_path / "copilot-out"
    copilot.mkdir()
    runner.env["COPILOT_HOME"] = str(copilot)
    runner.run("--global", "lock")
    planted = _moved_inside(runner, repo, tmp_path)
    narrowed = {"clients": json.dumps(["copilot"])}
    write_config(grim_home, settings={"team": team.fq}, options=narrowed, experimental_settings=True)
    runner.run("--global", "update")
    assert "kiro" in [output["client"] for record in _records(runner) for output in record["outputs"]]
    planted.unlink()
    os.mkfifo(planted)

    [row] = [item for item in runner.json("--global", "status", timeout=30)["items"] if item["name"] == "team"]
    plain = runner.plain("--global", "status", timeout=30)

    assert row["state"] == "not-armed"
    assert _causes(row) == [("kiro", "settings-root-in-workspace")]
    assert plain.returncode == 0, plain.stderr
    assert "not-armed" in plain.stdout


def test_a_refused_client_is_named_once_when_its_package_is_read(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Another client's missing value makes the run read the package; the refused client is still named once."""
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "copilot", "kiro")
    copilot = tmp_path / "copilot-out"
    copilot.mkdir()
    runner.env["COPILOT_HOME"] = str(copilot)
    planted = _moved_inside(runner, repo, tmp_path)
    (copilot / "settings.json").unlink()

    result = runner.run("--global", "install")

    assert result.returncode == 0, result.stderr
    assert result.stderr.count(INSIDE) == 1, result.stderr
    assert json.loads((copilot / "settings.json").read_text(encoding="utf-8")) == COPILOT
    assert json.loads(planted.read_text(encoding="utf-8")) == KIRO


@pytest.mark.parametrize(
    ("client", "variable", "file", "document"),
    [("copilot", "COPILOT_HOME", "settings.json", COPILOT), ("kiro", "KIRO_HOME", "settings/cli.json", KIRO)],
)
def test_a_recorded_value_stays_when_its_root_moves_into_the_repository(
    grim_binary: Path,
    grim_home: Path,
    repo: Path,
    tmp_path: Path,
    unique_repo: str,
    client: str,
    variable: str,
    file: str,
    document: dict,
) -> None:
    outside = tmp_path / f"{client}-out"
    outside.mkdir()
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), client)
    runner.env[variable] = str(outside)
    runner.run("--global", "install", "--client", client)
    assert json.loads((outside / file).read_text(encoding="utf-8")) == document
    [before] = _records(runner)
    inside = repo / f"{client}-home"
    planted = inside / file
    planted.parent.mkdir(parents=True)
    planted.write_text("{}", encoding="utf-8")
    runner.env[variable] = str(inside)

    result = runner.run("--global", "install", "--client", client)

    assert result.returncode == 0, result.stderr
    assert f"{INSIDE}; set {variable} to a path outside this repository, or unset it" in result.stderr
    assert result.stderr.count(INSIDE) == 1, result.stderr
    assert planted.read_text(encoding="utf-8") == "{}"
    assert _records(runner) == [before]
    row = _row(runner)
    assert row["state"] == "not-armed"
    assert _causes(row) == [(client, "settings-root-in-workspace")]
    assert [output["client"] for output in row["outputs"]] == [client]


def test_uninstall_leaves_a_file_inside_the_repository(
    grim_binary: Path, grim_home: Path, repo: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Uninstall takes the fence too: a value its record names under a root moved into the repository stays."""
    outside = tmp_path / "kiro-out"
    outside.mkdir()
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    runner.env["KIRO_HOME"] = str(outside)
    runner.run("--global", "install", "--client", "kiro")
    planted = repo / "kiro-home" / "settings" / "cli.json"
    planted.parent.mkdir(parents=True)
    planted.write_text(json.dumps(KIRO), encoding="utf-8")
    runner.env["KIRO_HOME"] = str(repo / "kiro-home")

    result = runner.run("--global", "uninstall", "settings", "team")

    assert result.returncode == 0, result.stderr
    assert "(settings-root-in-workspace); set KIRO_HOME to a path outside this repository" in result.stderr
    assert json.loads(planted.read_text(encoding="utf-8")) == KIRO


def test_a_change_line_names_the_resolved_path_when_home_lies_in_the_repository(
    grim_binary: Path, grim_home: Path, repo: Path, unique_repo: str
) -> None:
    """A ``~`` form would hide that a write under a ``$HOME`` inside the work tree lands in the repository."""
    runner = _locked(grim_binary, grim_home, repo, _package(unique_repo), "kiro")
    home = repo / "home"
    home.mkdir()
    runner.env["HOME"] = str(home)
    runner.env["USERPROFILE"] = str(home)

    result = runner.run("--global", "install", "--client", "kiro")

    written = Path(os.path.realpath(home / ".kiro" / "settings" / "cli.json"))
    assert json.loads(written.read_text(encoding="utf-8")) == KIRO
    assert f"added kiro {written.as_posix()} /chat.defaultModel (team)" in result.stderr
    assert "~/" not in result.stderr
