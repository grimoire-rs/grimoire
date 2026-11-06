# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What a teammate's commit does on this machine.

``adr_settings_artifact_kind.md``, Decisions 5 and 6, and the threat table's
non-goal for a teammate who drops or bumps a package. A teammate's commit moves
the config and the lock and nothing else: a ``--force`` they ran, a consent they
gave and a record on their machine do not travel. Here the rule table decides by
this machine's file and this machine's record. A value grim wrote moves with the
bump and needs no prompt, and a value the user holds is the user's until they
say otherwise. Fixtures come from ``make_settings``; each "nothing was written"
sits beside a run that writes, in the same test.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path
from subprocess import CompletedProcess

from src.helpers import make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
CLAUDE_ONLY = {"clients": json.dumps(["claude"])}
PROMPT = "[y/N]"


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


def _workspace(grim_at, workspace: Path, sandbox: Path, packages: dict[str, PublishedArtifact]) -> GrimRunner:
    write_config(
        workspace,
        settings={name: package.fq for name, package in packages.items()},
        options=CLAUDE_ONLY,
        experimental_settings=True,
    )
    runner = grim_at(workspace)
    runner.env.update(settings_sandbox(sandbox, runner.home)[0])
    runner.run("lock")
    return runner


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


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    rows = json.loads(result.stdout)["settings_changes"]
    return sorted((row["package"], row["key"], row["element"], row["action"]) for row in rows)


def _refusal(result: CompletedProcess[str], reason: str) -> dict:
    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert (error["reason"], error["forceable"]) == (reason, True), error
    return error


def _store_files(runner: GrimRunner) -> list[Path]:
    return sorted((Path(runner.grim_home) / "settings").glob("*.json"))


def test_copied_config_refuses(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A config copied to another directory brings no record, so a held value is not grim's."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    origin = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    origin.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"model": "sonnet", "language": "en"}

    copy = tmp_path / "copy"
    copy.mkdir()
    shutil.copy(project_dir / "grimoire.toml", copy)
    shutil.copy(project_dir / "grimoire.lock", copy)
    here = grim_at(copy)
    here.env.update(origin.env)
    _put(copy / LOCAL, {"model": "opus"})
    held = (copy / LOCAL).read_bytes()

    error = _refusal(here.run("install", TRUST, format="json", check=False), "untracked-destination")

    assert [(p["pointer"], p["state"], p["current"]) for p in error["pieces"]] == [
        ("/model", "untracked-destination", "opus")
    ]
    assert (copy / LOCAL).read_bytes() == held
    assert len(_store_files(here)) == 1, "only the origin's record exists"

    # Positive control: where nothing is held, the same copy does write.
    clean = tmp_path / "clean"
    clean.mkdir()
    shutil.copy(project_dir / "grimoire.toml", clean)
    shutil.copy(project_dir / "grimoire.lock", clean)
    fresh = grim_at(clean)
    fresh.env.update(origin.env)
    written = fresh.run("install", TRUST, format="json")
    assert _file(clean / LOCAL) == {"model": "sonnet", "language": "en"}
    assert _changes(written) == [("team", "/language", None, "added"), ("team", "/model", None, "added")]


def test_pull_changed_value_refuses(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A pulled bump over a value the user edited refuses as an edit does, and nothing moves."""
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    second = _package(unique_repo, "team", {"claude": {"model": "haiku", "language": "fr"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": first})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "opus", "language": "en"})
    _repin(project_dir, first, second)
    held = target.read_bytes()
    record = _store_files(runner)[0].read_bytes()

    error = _refusal(runner.run("install", TRUST, format="json", check=False), "modified")

    assert [(p["pointer"], p["state"], p["wanted"], p["current"]) for p in error["pieces"]] == [
        ("/model", "modified", "haiku", "opus")
    ]
    assert target.read_bytes() == held, "the intact language waits for the whole phase"
    assert _store_files(runner)[0].read_bytes() == record

    # Positive control: put the value back and the same pull applies, both keys.
    _put(target, {"model": "sonnet", "language": "en"})
    result = runner.run("install", TRUST, format="json")
    assert _file(target) == {"model": "haiku", "language": "fr"}
    assert _changes(result) == [("team", "/language", None, "changed"), ("team", "/model", None, "changed")]


def test_teammate_force_refuses_here(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Nothing of a teammate's `--force` rides in the lock; the edit here still refuses."""
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    second = _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": first})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "opus"})
    _repin(project_dir, first, second)
    lock = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    config = (project_dir / "grimoire.toml").read_text(encoding="utf-8")
    assert "force" not in lock.lower() and "force" not in config.lower()
    held = target.read_bytes()

    error = _refusal(runner.run("install", TRUST, format="json", check=False), "modified")

    assert [p["pointer"] for p in error["pieces"]] == ["/model"]
    assert target.read_bytes() == held

    # Positive control: this user's own `--force` is the gesture that writes.
    forced = runner.run("install", TRUST, "--force", format="json")
    assert _file(target) == {"model": "haiku"}
    assert _changes(forced) == [("team", "/model", None, "overwritten")]


def test_teammate_bump_needs_no_prompt(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """An intact value moves with the bump: no flag, no question, the keys listed."""
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    second = _package(unique_repo, "team", {"claude": {"model": "haiku", "language": "en"}}, tag="2")
    write_config(project_dir, options=CLAUDE_ONLY, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("add", first.fq)  # the user's own consent, given once
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet", "language": "en"}
    _repin(project_dir, first, second)

    result = runner.run("install", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert PROMPT not in result.stdout + result.stderr
    assert _file(target) == {"model": "haiku", "language": "en"}
    assert _changes(result) == [("team", "/model", None, "changed")]
    assert "changed claude" in result.stderr and "/model (team)" in result.stderr

    # Positive control: the same bump over a hand-edited value does not go quietly.
    third = _package(unique_repo, "team", {"claude": {"model": "o3", "language": "en"}}, tag="3")
    _put(target, {"model": "opus", "language": "en"})
    _repin(project_dir, second, third)
    refused = runner.run("install", format="json", check=False)
    assert refused.returncode == 65, refused.stderr
    assert _file(target) == {"model": "opus", "language": "en"}


def test_dropped_package_keeps_adopted(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A teammate drops a package: what grim wrote goes, what the user already held stays."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    other = _package(unique_repo, "other", {"claude": {"spinnerTipsEnabled": False}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)
    assert _file(target) == {"model": "sonnet", "language": "en", "spinnerTipsEnabled": False}

    write_config(
        project_dir, settings={"other": other.fq}, options=CLAUDE_ONLY, experimental_settings=True
    )
    runner.run("lock")
    result = runner.run("install", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert PROMPT not in result.stdout + result.stderr
    assert _file(target) == {"model": "sonnet", "spinnerTipsEnabled": False}
    assert _changes(result) == [("team", "/language", None, "removed")]
    recorded = [
        (record["name"], output["entry"])
        for path in _store_files(runner)
        for record in json.loads(path.read_text(encoding="utf-8"))["records"]
        for output in record["outputs"]
    ]
    assert recorded == [("other", "/spinnerTipsEnabled")], "the adopted value left no record behind"
