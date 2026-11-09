# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What ``grim status`` says about a settings row.

``adr_settings_artifact_kind.md``, Decision 8: ``stale``, then a whole-artifact
gate, then lifecycle. A row carries ``arming`` exactly as a hook row does, and a
cause that stops nothing is a note beside the lifecycle state. Fixtures come from
``make_settings``. Every "nothing was written" sits beside a run that does write,
in the same test.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, make_settings, settings_sandbox, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, fetch_manifest
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
CLAUDE_ONLY = {"clients": json.dumps(["claude"])}
DENY = "Bash(rm:*)"
RULES = [{"pattern": ["rm"], "decision": "forbidden"}]
SKILL_DOC = "---\nname: good\ndescription: d\n---\n# Good\n"

# What the settings path says for each refused home: it names the settings
# record, never the hook dispatch table or an armable file, and ends with the fix.
HOME_REFUSALS = {
    "grim-home-in-workspace": "GRIM_HOME resolves inside this workspace, which would make the settings record "
    "repo-resident; set it to an absolute path outside the workspace, then re-run the command",
    "grim-home-relative": "GRIM_HOME is a relative path, so the settings record would resolve inside the workspace; "
    "set it to an absolute path outside the workspace, then re-run the command",
}
STORE_REMEDY = "make that path a file your user can read, then re-run the command"

posix_links = pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks")
unix_git = pytest.mark.skipif(shutil.which("git") is None or os.name == "nt", reason="needs git and POSIX symlinks")


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


def _workspace(
    grim_at, workspace: Path, sandbox: Path, packages: dict[str, PublishedArtifact], **config: dict
) -> GrimRunner:
    """A workspace locked with the flag off, whose flag then went on."""
    write_config(workspace, settings={name: package.fq for name, package in packages.items()}, **config)
    runner = grim_at(workspace)
    runner.env.update(settings_sandbox(sandbox, runner.home)[0])
    runner.run("lock")
    runner.run("config", "set", "options.experimental.settings", "true")
    return runner


def _installed(
    grim_at, workspace: Path, sandbox: Path, packages: dict[str, PublishedArtifact], **config: dict
) -> GrimRunner:
    """A Claude workspace the user consented to, converged."""
    config.setdefault("options", CLAUDE_ONLY)
    runner = _workspace(grim_at, workspace, sandbox, packages, **config)
    runner.run("consent", "allow")
    runner.run("install")
    return runner


def _global(grim, home: Path, sandbox: Path, team: PublishedArtifact, clients: list[str]) -> dict[str, Path]:
    write_config(
        home, settings={"team": team.fq}, options={"clients": json.dumps(clients)}, experimental_settings=True
    )
    env, roots = settings_sandbox(sandbox, grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")
    return roots


def _relative_home(grim, grim_home: Path) -> None:
    grim.cwd = grim_home.parent
    grim.env["GRIM_HOME"] = grim_home.name


def _repin(workspace: Path, old: PublishedArtifact, new: PublishedArtifact) -> None:
    lock = workspace / "grimoire.lock"
    text = lock.read_text(encoding="utf-8")
    assert old.digest in text
    lock.write_text(text.replace(old.digest, new.digest), encoding="utf-8")


def _file(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _put(path: Path, values: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(values), encoding="utf-8")


def _row(report: dict, name: str) -> dict:
    [row] = [item for item in report["items"] if item["name"] == name]
    return row


def _causes(row: dict) -> list[tuple[str, str]]:
    return [(entry["client"], entry["cause"]) for entry in row["arming"]]


def _store_files(runner: GrimRunner) -> list[Path]:
    return sorted((Path(runner.grim_home) / "settings").glob("*.json"))


def _consent_files(runner: GrimRunner) -> list[Path]:
    return sorted((Path(runner.grim_home) / "hooks" / "consent").glob("*.json"))


def _layer_cached(runner: GrimRunner, package: PublishedArtifact) -> bool:
    [layer] = fetch_manifest(package.repo, package.tag)["layers"]
    digest = layer["digest"].removeprefix("sha256:")
    return any(digest in path.name for path in (Path(runner.grim_home) / "blobs").rglob("*"))


def _seed_record(runner: GrimRunner, workspace: Path, keys: list[str]) -> None:
    key = hashlib.sha256(str(workspace).encode()).hexdigest()
    path = Path(runner.grim_home) / "hooks" / "consent" / f"{key}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    record = {"v": 1, "workspace": str(workspace), "hooks": keys, "consented_at": "2026-08-28T10:00:00Z"}
    path.write_text(json.dumps(record), encoding="utf-8")


def _key(binding: str, package: PublishedArtifact) -> str:
    return f"settings:{binding}@{REGISTRY_HOST}/{package.repo}"


def _plant(shape: str, workspace: Path, away: Path) -> Path:
    """A link at the local file, or at ``.claude``, into ``away``. Returns where a write would land."""
    away.mkdir(parents=True, exist_ok=True)
    if shape == "directory":
        shutil.rmtree(workspace / ".claude", ignore_errors=True)
        (workspace / ".claude").symlink_to(away, target_is_directory=True)
    else:
        (workspace / ".claude").mkdir(exist_ok=True)
        (workspace / LOCAL).unlink(missing_ok=True)
        (workspace / LOCAL).symlink_to(away / "settings.local.json")
    return away / "settings.local.json"


def _pull_link(shape: str, workspace: Path) -> None:
    (workspace / (".claude" if shape == "directory" else LOCAL)).unlink()


def _path_sourced(workspace: Path) -> tuple[Path, str]:
    """Swap the lock's pin for a path. Returns the lock and its pinned text."""
    lock = workspace / "grimoire.lock"
    pinned = lock.read_text(encoding="utf-8")
    edited, swapped = re.subn(
        r'^pinned = "[^"]*team[^"]*"$',
        f'path = "./team"\nhash = "sha256:{"a" * 64}"',
        pinned,
        flags=re.MULTILINE,
    )
    assert swapped == 1, pinned
    lock.write_text(edited, encoding="utf-8")
    return lock, pinned


@pytest.fixture()
def git(tmp_path: Path):
    """Git with no system or user config, so a host's excludes cannot decide a test."""
    home = tmp_path / "git-home"
    home.mkdir()
    env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1"}

    def run(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args], cwd=cwd, env=env, check=check, capture_output=True, text=True, encoding="utf-8"
        )

    return run


# ── One test per indicator ──────────────────────────────────────────────────


def test_intact_outputs_at_the_pin_read_installed(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "permissions": {"deny": [DENY]}}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})

    row = _row(runner.json("status"), "team")

    assert row["state"] == "installed"
    assert row["arming"] == []
    assert row["outputs_pending"] == []
    for output in row["outputs"]:
        assert set(output) == {"client", "path", "element"}, output
    # An array piece names its element; a scalar has none to name.
    assert sorted(str(output["element"]) for output in row["outputs"]) == sorted([DENY, "None"])


def test_a_pin_behind_the_lock_reads_outdated(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    assert _row(runner.json("status"), "team")["state"] == "installed"

    _repin(project_dir, team, retuned)

    assert _row(runner.json("status"), "team")["state"] == "outdated"


def test_a_changed_value_reads_modified(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})

    _put(project_dir / LOCAL, {"model": "opus"})

    assert _row(runner.json("status"), "team")["state"] == "modified"


def test_no_record_reads_missing(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    runner.run("consent", "allow")

    row = _row(runner.json("status"), "team")

    assert row["state"] == "missing"
    assert row["arming"] == []
    # The kind pends nothing, because status holds no document to say what would land.
    assert row["outputs_pending"] == []


def test_stale_wins_over_a_gate(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    second = _package(unique_repo, "second", {"claude": {"effortLevel": "high"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    assert _row(runner.json("status"), "team")["state"] == "gated"

    write_config(
        project_dir,
        settings={"team": team.fq, "second": second.fq},
        options=CLAUDE_ONLY,
        experimental_settings=True,
    )

    row = _row(runner.json("status"), "team")
    assert row["state"] == "stale"
    # The state is the lock's, and the gate behind it still names itself.
    assert _causes(row) == [("claude", "workspace-not-consented")]


# ── The named behaviours ────────────────────────────────────────────────────


def test_reordered_keys_read_installed(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    _put(project_dir / LOCAL, {"theme": "dark"})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    local = project_dir / LOCAL
    values = _file(local)
    assert list(values) == ["theme", "model"]

    _put(local, dict(reversed(values.items())))

    assert list(_file(local)) == ["model", "theme"]
    assert _row(runner.json("status"), "team")["state"] == "installed"


def test_zero_applicable_reads_gated(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Without a record status would read ``missing`` and could never settle."""
    team = _package(unique_repo, "team", {"codex": {"model": "gpt-5"}})
    other = _package(unique_repo, "other", {"claude": {"model": "sonnet"}})
    packages = {"team": team, "other": other}
    runner = _workspace(grim_at, project_dir, tmp_path / "s", packages, options=CLAUDE_ONLY)
    runner.run("consent", "allow")

    report = runner.json("install")

    assert _row(report, "team")["status"] == "skipped"
    status = runner.json("status")
    row = _row(status, "team")
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "no-applicable-settings")]
    assert row["outputs"] == []

    # Positive control: the package that does apply reads as its lifecycle.
    assert _row(status, "other")["state"] == "installed"
    assert _row(status, "other")["arming"] == []


def test_zero_applicable_behind_the_lock_reads_outdated(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The record says nothing applied at its pin, and the next install re-reads the package."""
    team = _package(unique_repo, "team", {"codex": {"model": "gpt-5"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="2")
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    assert _row(runner.json("status"), "team")["state"] == "gated"

    _repin(project_dir, team, retuned)

    row = _row(runner.json("status"), "team")
    assert row["state"] == "outdated"
    assert row["arming"] == []

    # Positive control: the install it promises does land the new pin.
    runner.run("install")
    assert _row(runner.json("status"), "team")["state"] == "installed"


def test_a_partial_decline_is_a_note_on_the_lifecycle_state(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Only a package nothing applies to reads gated; one declining client is a note."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt-5"}})
    both = {"clients": json.dumps(["claude", "codex"])}
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team}, options=both)

    row = _row(runner.json("status"), "team")

    assert row["state"] == "installed"
    assert _causes(row) == [("codex", "no-applicable-settings")]
    # The note ends with the step that would apply the declined client's pieces.
    assert [entry["message"] for entry in row["arming"]] == [
        (
            "codex applies at global scope only; declare the package at global scope with grim --global add, "
            "then run grim --global install"
        )
    ]


def test_a_settings_row_names_no_missing_client(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Install records a client only for the pieces a package declares, so a host it skips is not drift."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    both = {"clients": json.dumps(["claude", "opencode"])}
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team}, options=both)

    row = _row(runner.json("status"), "team")

    assert row["state"] == "installed"
    assert [output["client"] for output in row["outputs"]] == ["claude"]
    assert row["clients_missing"] == []
    assert row["clients_extra"] == []

    # Positive control: a recorded client the config dropped still reads as drift.
    write_config(
        project_dir,
        settings={"team": team.fq},
        options={"clients": json.dumps(["opencode"])},
        experimental_settings=True,
    )
    row = _row(runner.json("status"), "team")
    assert row["clients_missing"] == []
    assert row["clients_extra"] == ["claude"]


def test_a_global_settings_row_names_no_missing_client(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """At global scope codex takes settings, so only the settings-row rule keeps it out of `clients_missing`."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    skill = make_artifact(f"{unique_repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")
    write_config(
        grim_home,
        skills={"good": skill.fq},
        settings={"team": team.fq},
        options={"clients": json.dumps(["claude", "codex"])},
        experimental_settings=True,
    )
    grim.env.update(settings_sandbox(tmp_path / "s", grim.home)[0])
    grim.run("--global", "lock")
    grim.run("--global", "install", "--client", "claude")

    report = grim.json("--global", "status")

    row = _row(report, "team")
    assert [output["client"] for output in row["outputs"]] == ["claude"]
    assert row["clients_missing"] == []
    # Positive control: the skill the same run left off codex reads it as missing.
    assert _row(report, "good")["clients_missing"] == ["codex"]


def _beside_zed(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> GrimRunner:
    """A global scope selecting claude and zed: `team` applies to claude, `other` to no selected client."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"codex": {"model": "gpt"}})
    write_config(
        grim_home,
        settings={"team": team.fq, "other": other.fq},
        options={"clients": json.dumps(["claude", "zed"])},
        experimental_settings=True,
    )
    grim.env.update(settings_sandbox(tmp_path / "s", grim.home)[0])
    grim.run("--global", "lock")
    grim.run("--global", "install")
    return grim


def _verdict(row: dict, client: str) -> dict:
    [verdict] = [entry for entry in row["arming"] if entry["client"] == client]
    return verdict


def test_a_client_without_a_file_needs_no_step_once_other_clients_hold_the_values(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Changing the selection to reach zed would drop it from skills, rules and MCP too."""
    runner = _beside_zed(grim, grim_home, tmp_path, unique_repo)

    report = runner.json("--global", "status")

    zed = _verdict(_row(report, "team"), "zed")
    assert zed["cause"] == "no-applicable-settings"
    assert zed["message"] == (
        "zed has no settings file here; nothing in this package applies to zed, and other clients hold its "
        "values, so no step is needed"
    )
    # A note logs at debug, and `GRIM_LOG` is the filter grim reads.
    runner.env["GRIM_LOG"] = "debug"
    plain = runner.run("--global", "status")
    assert "on client 'zed': zed has no settings file here; nothing in this package applies to zed" in plain.stderr
    # Positive control: where nothing applied anywhere, the selection is the step.
    assert _verdict(_row(report, "other"), "zed")["message"] == (
        "zed has no settings file here; select a client that takes settings with --client <name> or [options].clients"
    )


def test_a_global_row_names_the_global_install(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    runner = _beside_zed(grim, grim_home, tmp_path, unique_repo)

    claude = _verdict(_row(runner.json("--global", "status"), "other"), "claude")

    assert claude["cause"] == "no-applicable-settings"
    assert claude["message"].endswith("then run grim --global install"), claude["message"]


def test_no_surface_anywhere_reads_gated(grim_at, tmp_path: Path, unique_repo: str) -> None:
    """No client detected selects the generic one, which has no settings file."""
    bare = tmp_path / "bare"
    bare.mkdir()
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, bare, tmp_path / "s", {"team": team})
    runner.run("consent", "allow")

    before = _row(runner.json("status"), "team")
    assert before["state"] == "gated"
    assert _causes(before) == [("agents", "no-applicable-settings")]

    runner.run("install")
    assert _row(runner.json("status"), "team")["state"] == "gated"
    assert not (bare / LOCAL).exists()

    # Positive control: once a client appears the same package lands.
    (bare / ".claude").mkdir()
    runner.run("install")
    assert _file(bare / LOCAL) == {"model": "sonnet"}
    assert _row(runner.json("status"), "team")["state"] == "installed"


@unix_git
def test_tracked_local_file_note(git, tmp_path: Path, grim_at, unique_repo: str) -> None:
    """The file is committable, which moves nothing: the values are still applied."""
    primary = tmp_path / "primary"
    primary.mkdir()
    git(primary, "init", "-q", "-b", "main")
    _put(primary / LOCAL, {"theme": "dark"})
    git(primary, "add", "-f", LOCAL)
    git(primary, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "tracked")
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, primary, tmp_path / "s", {"team": team})

    row = _row(runner.json("status"), "team")

    assert row["state"] == "installed"
    assert _causes(row) == [("claude", "local-file-tracked")]

    # Positive control: the same workspace with the file untracked carries no note.
    git(primary, "rm", "-q", "--cached", LOCAL)
    row = _row(runner.json("status"), "team")
    assert row["state"] == "installed"
    assert row["arming"] == []


@unix_git
def test_a_hung_tracked_probe_does_not_hold_status(git, tmp_path: Path, grim_at, unique_repo: str) -> None:
    """The probe's deadline bounds the process, not only the answer: status exits and reads no note."""
    primary = tmp_path / "primary"
    primary.mkdir()
    git(primary, "init", "-q", "-b", "main")
    _put(primary / LOCAL, {"theme": "dark"})
    git(primary, "add", "-f", LOCAL)
    git(primary, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "tracked")
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, primary, tmp_path / "s", {"team": team})
    real_git = shutil.which("git")
    fake = tmp_path / "fake-bin"
    fake.mkdir()
    (fake / "git").write_text(
        f'#!/bin/sh\ncase "$*" in *ls-files*) exec sleep 60 ;; esac\nexec "{real_git}" "$@"\n', encoding="utf-8"
    )
    (fake / "git").chmod(0o755)
    path = runner.env["PATH"]
    runner.env["PATH"] = f"{fake}{os.pathsep}{path}"

    row = _row(runner.json("status", timeout=30), "team")

    assert row["state"] == "installed"
    assert row["arming"] == []

    # Positive control: the same workspace with a git that answers carries the note.
    runner.env["PATH"] = path
    assert _causes(_row(runner.json("status"), "team")) == [("claude", "local-file-tracked")]


def test_fresh_clone_reports_workspace_not_consented(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(project_dir, settings={"team": team.fq}, options=CLAUDE_ONLY, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")

    result = runner.run("install")

    assert result.returncode == 0, result.stderr
    assert not (project_dir / LOCAL).exists()
    assert _store_files(runner) == []
    row = _row(runner.json("status"), "team")
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "workspace-not-consented")]
    assert "grim consent allow" in row["arming"][0]["message"]

    # Positive control: the gesture is what the row was asking for.
    runner.run("consent", "allow")
    runner.run("install")
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    assert _row(runner.json("status"), "team")["state"] == "installed"


def test_a_consented_package_converges_beside_a_drifted_one(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A package answers for itself, as it does in the run."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    waiting = _package(unique_repo, "waiting", {"claude": {"effortLevel": "high"}})
    write_config(
        project_dir,
        settings={"team": team.fq, "waiting": waiting.fq},
        options=CLAUDE_ONLY,
        experimental_settings=True,
    )
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")
    _seed_record(runner, project_dir, [_key("team", team)])

    runner.run("install")

    status = runner.json("status")
    assert _row(status, "team")["state"] == "installed"
    assert _row(status, "team")["arming"] == []
    assert _row(status, "waiting")["state"] == "gated"
    assert _causes(_row(status, "waiting")) == [("claude", "consent-drifted")]
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}


def test_a_trusted_run_leaves_the_workspace_unconsented(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The flag answers one run and writes no record, so the row says what the next run does."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)

    runner.run("install", TRUST)

    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    assert _consent_files(runner) == []
    row = _row(runner.json("status"), "team")
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "workspace-not-consented")]
    assert len(row["outputs"]) == 1

    # Positive control: the gesture settles it with nothing left to install.
    runner.run("consent", "allow")
    assert _row(runner.json("status"), "team")["state"] == "installed"


@pytest.mark.parametrize(("cause", "relative"), [("grim-home-in-workspace", False), ("grim-home-relative", True)])
def test_grim_home_refusal_reads_not_armed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, cause: str, relative: bool
) -> None:
    """A record under such a home may be a repository's, so status reads none of it."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    local = project_dir / LOCAL
    written = local.read_bytes()
    home = runner.env["GRIM_HOME"]
    nested = project_dir / ".grim-home"
    shutil.copytree(Path(home) / "settings", nested / "settings")
    shutil.copytree(Path(home) / "hooks" / "consent", nested / "hooks" / "consent")
    delivered = sorted((path, path.read_bytes()) for path in nested.rglob("*.json"))
    runner.env["GRIM_HOME"] = ".grim-home" if relative else str(nested)

    result = runner.run("install")
    assert result.returncode == 0, result.stderr
    warning = f"settings left unchanged: {HOME_REFUSALS[cause]}"
    assert warning in result.stderr
    # Malformed on purpose: status under this home must not read it.
    malformed = "this is = = not toml\n"
    (nested / "grimoire.toml").write_text(malformed, encoding="utf-8")
    report = runner.json("status")

    row = _row(report, "team")
    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", cause)]
    assert [entry["message"] for entry in row["arming"]] == [warning]
    assert row["outputs"] == []
    assert local.read_bytes() == written
    assert sorted((path, path.read_bytes()) for path in nested.rglob("*.json")) == delivered

    # Positive control: the same workspace under the real home reads its record.
    runner.env["GRIM_HOME"] = home
    row = _row(runner.json("status"), "team")
    assert row["state"] == "installed"
    assert len(row["outputs"]) == 1

    # Positive control: that same file costs a 78 under a home grim does read.
    (Path(home) / "grimoire.toml").write_text(malformed, encoding="utf-8")
    assert runner.run("status", check=False).returncode == 78


@pytest.mark.parametrize(("cause", "relative"), [("grim-home-in-workspace", False), ("grim-home-relative", True)])
def test_a_refused_home_names_its_cause_alone(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, cause: str, relative: bool
) -> None:
    """Under a refused home a package consent would withhold gets no line of its own: the home is the cause."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    home = runner.env["GRIM_HOME"]
    runner.env["GRIM_HOME"] = ".grim-home" if relative else str(project_dir / ".grim-home")

    result = runner.run("install", "--no-trust-workspace")

    assert result.returncode == 0, result.stderr
    assert result.stderr.count("settings left unchanged: ") == 1, result.stderr
    assert f"settings left unchanged: {HOME_REFUSALS[cause]}" in result.stderr
    assert "settings package 'team' not applied" not in result.stderr

    # Positive control: under the real home the same answer names the package.
    runner.env["GRIM_HOME"] = home
    assert "settings package 'team' not applied" in runner.run("install", "--no-trust-workspace").stderr


def test_a_refused_home_at_global_scope_is_not_zero_applicable(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The load empties the record's outputs, so the home must be asked first."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude"])
    grim.run("--global", "install")
    assert _file(roots["claude"] / "settings.json") == {"model": "sonnet"}
    _relative_home(grim, grim_home)

    row = _row(grim.json("--global", "status"), "team")

    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "grim-home-relative")]
    assert row["outputs"] == []

    # Positive control: the absolute home reads the record it could not.
    grim.env["GRIM_HOME"] = str(grim_home)
    row = _row(grim.json("--global", "status"), "team")
    assert row["state"] == "installed"
    assert len(row["outputs"]) == 1


def test_a_record_kept_by_a_refused_uninstall_reports_nothing(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The record is an orphan the refused home may not read, so it gets no row."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude"])
    grim.run("--global", "install")
    _relative_home(grim, grim_home)
    grim.run("--global", "uninstall", "settings", "team")

    report = grim.json("--global", "status")

    assert [item["name"] for item in report["items"]] == []
    assert _file(roots["claude"] / "settings.json") == {"model": "sonnet"}


def test_an_unreadable_store_reads_not_armed(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """The run writes nothing without its records, so the row names the store and the fix, not ``missing``."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    [store] = _store_files(runner)
    saved = store.read_bytes()
    store.unlink()
    store.mkdir()

    result = runner.run("status", format="json")

    row = _row(json.loads(result.stdout), "team")
    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "not-registered")]
    message = f"settings left unchanged: their records at {store} could not be read; {STORE_REMEDY}"
    assert [entry["message"] for entry in row["arming"]] == [message]
    assert f"ignoring the settings records at {store}" in result.stderr
    assert STORE_REMEDY in result.stderr

    # Positive control: the same records, readable again, read as installed.
    store.rmdir()
    store.write_bytes(saved)
    row = _row(runner.json("status"), "team")
    assert row["state"] == "installed"
    assert row["arming"] == []


@posix_links
@pytest.mark.parametrize("shape", ["file", "directory"])
def test_confinement_refusal_reads_not_armed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, shape: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    runner.run("consent", "allow")
    landed = _plant(shape, project_dir, tmp_path / "outside")

    result = runner.run("install")

    assert result.returncode == 0, result.stderr
    assert not landed.exists()
    row = _row(runner.json("status"), "team")
    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "not-registered")]

    # Positive control: without the link the same workspace converges.
    _pull_link(shape, project_dir)
    (project_dir / ".claude").mkdir(exist_ok=True)
    runner.run("install")
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    assert _row(runner.json("status"), "team")["state"] == "installed"


@posix_links
def test_a_withheld_pin_behind_the_lock_reads_not_armed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The run withheld the package, so ``outdated`` would promise an install that cannot land."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    landed = _plant("file", project_dir, tmp_path / "outside")
    _repin(project_dir, team, retuned)

    runner.run("install")

    assert not landed.exists()
    row = _row(runner.json("status"), "team")
    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "not-registered")]

    # Positive control: without the link that pin lands.
    _pull_link("file", project_dir)
    runner.run("install")
    assert _file(project_dir / LOCAL) == {"model": "opus"}
    assert _row(runner.json("status"), "team")["state"] == "installed"


@posix_links
def test_an_empty_withheld_record_behind_the_lock_reads_not_armed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A refused surface outranks a zero-output record behind the lock: ``outdated`` promises an install."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    runner.run("consent", "allow")
    landed = _plant("file", project_dir, tmp_path / "outside")
    runner.run("install")
    _repin(project_dir, team, retuned)

    row = _row(runner.json("status"), "team")

    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "not-registered")]
    assert not landed.exists()

    # Positive control: the same record reads behind the lock once the link is
    # gone, so the refusal alone moved the row, and that pin then lands.
    _pull_link("file", project_dir)
    assert _row(runner.json("status"), "team")["state"] == "outdated"
    runner.run("install")
    assert _file(project_dir / LOCAL) == {"model": "opus"}
    assert _row(runner.json("status"), "team")["state"] == "installed"


def test_flag_off_reads_gated(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    added = _package(unique_repo, "added", {"claude": {"effortLevel": "high"}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    written = (project_dir / LOCAL).read_bytes()
    consents = _consent_files(runner)
    assert _layer_cached(runner, team)
    runner.run("config", "set", "options.experimental.settings", "false")

    runner.run("add", "--kind", "settings", added.fq)
    runner.run("install")

    assert added.repo in (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert not _layer_cached(runner, added)
    assert _consent_files(runner) == consents
    assert (project_dir / LOCAL).read_bytes() == written
    status = runner.json("status")
    for name in ("team", "added"):
        assert _row(status, name)["state"] == "gated"
        assert _causes(_row(status, name)) == [("claude", "feature-flag-off")]
    assert "grim config set options.experimental.settings true" in _row(status, "team")["arming"][0]["message"]
    assert "feature-flag-off" in runner.run("status").stdout


def test_flag_off_names_the_flag_of_its_own_scope(
    grim_at, project_dir: Path, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The project flag does not cover `--global`, so a gated global row names the global one."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(project_dir, settings={"team": team.fq}, options=CLAUDE_ONLY)
    write_config(grim_home, settings={"team": team.fq}, options=CLAUDE_ONLY)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")
    runner.run("--global", "lock")

    row = _row(runner.json("--global", "status"), "team")

    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "feature-flag-off")]
    assert row["arming"][0]["message"] == (
        "settings are gated; enable them with `grim --global config set options.experimental.settings true`"
    )
    # Positive control: the project row, gated by the same cause, keeps the project sentence word for word.
    project = _row(runner.json("status"), "team")
    assert project["state"] == "gated"
    assert _causes(project) == [("claude", "feature-flag-off")]
    assert project["arming"][0]["message"] == (
        "settings are gated; enable them with `grim config set options.experimental.settings true`"
    )


def test_flag_off_names_the_config_it_ran_under(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Followed from where it was printed, the remedy sets the flag of the project the row is in."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    config = write_config(project_dir, settings={"team": team.fq}, options=CLAUDE_ONLY)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")
    away = tmp_path / "away"
    away.mkdir()
    elsewhere = grim_at(away)
    elsewhere.env = runner.env

    row = _row(elsewhere.json("--config", str(config), "status"), "team")

    assert _causes(row) == [("claude", "feature-flag-off")]
    assert row["arming"][0]["message"] == (
        f"settings are gated; enable them with `grim --config {config} config set options.experimental.settings true`"
    )


def test_cursor_absent_file_note(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "cursor": {"approvalMode": "allowlist"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "cursor"])

    grim.run("--global", "install")

    row = _row(grim.json("--global", "status"), "team")
    assert row["state"] == "installed"
    assert _causes(row) == [("cursor", "client-config-absent")]
    assert not (roots["cursor"] / "cli-config.json").exists()

    # Positive control: once Cursor has written its file the piece lands and the note goes.
    _put(roots["cursor"] / "cli-config.json", {})
    grim.run("--global", "install")
    row = _row(grim.json("--global", "status"), "team")
    assert row["state"] == "installed"
    assert row["arming"] == []
    assert _file(roots["cursor"] / "cli-config.json") == {"approvalMode": "allowlist"}


def test_a_lone_absent_cursor_file_reads_gated(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """No piece applies across the selected clients, and the note says which file is short."""
    team = _package(unique_repo, "team", {"cursor": {"approvalMode": "allowlist"}})
    _global(grim, grim_home, tmp_path / "s", team, ["cursor"])

    grim.run("--global", "install")

    row = _row(grim.json("--global", "status"), "team")
    assert row["state"] == "gated"
    assert _causes(row) == [("cursor", "client-config-absent")]


def test_revoke_keeps_values_and_reads_gated(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})
    assert _row(runner.json("status"), "team")["state"] == "installed"

    runner.run("consent", "revoke")

    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    row = _row(runner.json("status"), "team")
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "workspace-not-consented")]
    assert len(row["outputs"]) == 1

    # The undo needs no consent.
    runner.run("uninstall", "settings", "team")
    assert _file(project_dir / LOCAL) == {}
    assert runner.json("status")["items"] == []


def test_harness_dropped_value_reads_missing(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _installed(grim_at, project_dir, tmp_path / "s", {"team": team})

    _put(project_dir / LOCAL, {"theme": "dark"})

    assert _row(runner.json("status"), "team")["state"] == "missing"

    # The next install restores it, and asks for no --force.
    runner.run("install")
    assert _file(project_dir / LOCAL) == {"theme": "dark", "model": "sonnet"}
    assert _row(runner.json("status"), "team")["state"] == "installed"


def test_an_own_file_has_no_piece_detail(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """The row answers for the whole file: its one output names no element."""
    team = _package(unique_repo, "team", {"codex": {"rules": RULES}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["codex"])
    grim.run("--global", "install")
    rendered = roots["codex"] / "rules" / "grim-team.rules"

    row = _row(grim.json("--global", "status"), "team")
    assert row["state"] == "installed"
    assert [(output["client"], output["element"]) for output in row["outputs"]] == [("codex", None)]

    rendered.write_text("hand edited\n", encoding="utf-8")
    assert _row(grim.json("--global", "status"), "team")["state"] == "modified"

    rendered.unlink()
    assert _row(grim.json("--global", "status"), "team")["state"] == "missing"


def test_a_path_source_reads_not_armed(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """No digest pins it, so no flag and no consent could change the answer."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    runner.run("consent", "allow")
    lock, pinned = _path_sourced(project_dir)

    row = _row(runner.json("status"), "team")
    assert row["state"] == "not-armed"
    assert _causes(row) == [("claude", "not-registered")]

    # Positive control: the same package under its pin reads as its lifecycle.
    lock.write_text(pinned, encoding="utf-8")
    assert _row(runner.json("status"), "team")["state"] == "missing"


def test_a_bundle_member_reads_the_same_gate(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A member reaches status by a second route, which the gate must not miss."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    pack = make_bundle(f"{unique_repo}/pack", [("settings", "team", team.fq)], tag="1")
    write_config(project_dir, bundles={"pack": pack.fq}, options=CLAUDE_ONLY, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")

    row = _row(runner.json("status"), "team")
    assert row["state"] == "gated"
    assert _causes(row) == [("claude", "workspace-not-consented")]

    # Positive control: the gesture opens the gate for the member too.
    runner.run("consent", "allow")
    runner.run("install")
    row = _row(runner.json("status"), "team")
    assert row["state"] == "installed"
    assert row["arming"] == []
