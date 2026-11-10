# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A repository cannot hand grim a settings record through ``state.json``.

``adr_settings_artifact_kind.md``, Decision 6 and the threat table. The project
records live under ``$GRIM_HOME``, so a ``.grimoire/state.json`` that carries a
settings record is discarded with a warning, and nothing it claims is applied
or removed. The probe is a user's own ``deny`` rule and a value they hold:
honoured, a forged record would let grim overwrite the value or reap the rule
without ``--force``. The file arrives two ways: checked out as it stands, and
through a real ``git pull`` over a real state file. Fixtures come from
``make_settings``; each "nothing was written" sits beside a run that writes.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import make_artifact, make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
SKILL_DOC = "---\nname: good\ndescription: d\n---\n# Good\n"
CLAUDE_ONLY = {"clients": json.dumps(["claude"])}
USER_RULE = "Bash(curl:*)"
DENY = "Bash(rm:*)"
MINE = {"model": "opus", "permissions": {"deny": [USER_RULE]}}

needs_git = pytest.mark.skipif(shutil.which("git") is None or os.name == "nt", reason="needs git")


def _hash(value: object) -> str:
    return "sha256:" + hashlib.sha256(json.dumps(value, separators=(",", ":")).encode()).hexdigest()


def _output(pointer: str, value: object, element: str | None = None) -> dict:
    output = {
        "client": "claude",
        "target": {"anchor": "workspace", "relative": LOCAL},
        "content_hash": _hash(element if element is not None else value),
        "entry": pointer,
    }
    if element is not None:
        output["element"] = element
    return output


def _forged_records(team: PublishedArtifact) -> list[dict]:
    """The claims a hostile repository would make about the user's own values.

    ``team`` is declared and claims the held model as its own, so a record
    honoured would overwrite it without a word. ``ghost`` is declared nowhere
    and claims the user's rule, so a record honoured would reap it.
    """
    return [
        {"kind": "settings", "name": "team", "pinned": team.pinned, "outputs": [_output("/model", "opus")]},
        {
            "kind": "settings",
            "name": "ghost",
            "pinned": team.pinned,
            "outputs": [_output("/permissions/deny", USER_RULE, element=USER_RULE)],
        },
    ]


def _package(repo: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/team", tag, {"description": "team defaults", **tables})


def _skill(repo: str) -> PublishedArtifact:
    return make_artifact(f"{repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")


def _file(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _put(path: Path, values: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(values), encoding="utf-8", newline="\n")


def _store_files(runner: GrimRunner) -> list[Path]:
    return sorted((Path(runner.grim_home) / "settings").glob("*.json"))


def _refused(result: CompletedProcess[str]) -> dict:
    assert result.returncode == 65, result.stderr
    return json.loads(result.stdout)["error"]


def _discarded(result: CompletedProcess[str]) -> None:
    """The warning names each discarded record and the file it came from."""
    for name in ("team", "ghost"):
        assert f"discarded settings record '{name}'" in result.stderr, result.stderr
    assert "state.json" in result.stderr, result.stderr


def _then_the_ordinary_road_works(runner: GrimRunner, workspace: Path, skill: PublishedArtifact) -> None:
    """Positive control: with the held value gone, this machine does write and does reap."""
    _put(workspace / LOCAL, {"permissions": {"deny": [USER_RULE]}})
    written = runner.run("install", TRUST, format="json")
    assert _file(workspace / LOCAL) == {"model": "sonnet", "permissions": {"deny": [USER_RULE, DENY]}}
    assert [row["key"] for row in json.loads(written.stdout)["settings_changes"]] == ["/model", "/permissions/deny"]

    write_config(workspace, skills={"good": skill.fq}, options=CLAUDE_ONLY, experimental_settings=True)
    runner.run("lock")
    reaped = runner.run("install", format="json")
    assert _file(workspace / LOCAL) == {"permissions": {"deny": [USER_RULE]}}
    assert [row["action"] for row in json.loads(reaped.stdout)["settings_changes"]] == ["removed", "removed"]


def test_committed_forged_state_neither_applies_nor_removes(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, {"claude": {"model": "sonnet", "permissions": {"deny": [DENY]}}})
    skill = _skill(unique_repo)
    write_config(
        project_dir,
        skills={"good": skill.fq},
        settings={"team": team.fq},
        options=CLAUDE_ONLY,
        experimental_settings=True,
    )
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")
    # The skill alone gives the project a real state file to write over.
    state = project_dir / ".grimoire" / "state.json"
    _put(project_dir / LOCAL, MINE)
    state.parent.mkdir(exist_ok=True)
    state.write_text(json.dumps({"version": 2, "records": _forged_records(team)}), encoding="utf-8", newline="\n")

    result = runner.run("install", TRUST, format="json", check=False)

    error = _refused(result)
    assert error["reason"] == "untracked-destination"
    assert [(p["pointer"], p["current"]) for p in error["pieces"]] == [("/model", "opus")]
    assert _file(project_dir / LOCAL) == MINE
    assert _store_files(runner) == []
    _discarded(result)

    _then_the_ordinary_road_works(runner, project_dir, skill)


@needs_git
def test_pulled_forged_state_neither_applies_nor_removes(
    grim_at, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, {"claude": {"model": "sonnet", "permissions": {"deny": [DENY]}}})
    skill = _skill(unique_repo)
    env = {**os.environ, "HOME": str(tmp_path / "git-home"), "XDG_CONFIG_HOME": str(tmp_path / "git-home")}
    env["GIT_CONFIG_NOSYSTEM"] = "1"
    (tmp_path / "git-home").mkdir()

    def git(cwd: Path, *args: str) -> CompletedProcess[str]:
        return subprocess.run(
            ["git", *args], cwd=cwd, env=env, check=True, capture_output=True, text=True, encoding="utf-8"
        )

    def clone(name: str) -> Path:
        path = tmp_path / name
        git(tmp_path, "clone", "-q", str(tmp_path / "origin.git"), str(path))
        git(path, "config", "user.email", "t@example.test")
        git(path, "config", "user.name", "t")
        return path

    git(tmp_path, "init", "-q", "--bare", "-b", "main", str(tmp_path / "origin.git"))
    attacker = clone("attacker")

    # The first commit is an ordinary project: one skill, no settings.
    write_config(attacker, skills={"good": skill.fq}, options=CLAUDE_ONLY)
    attacker_runner = grim_at(attacker)
    attacker_runner.run("lock")
    git(attacker, "add", "grimoire.toml", "grimoire.lock")
    git(attacker, "commit", "-q", "-m", "project")
    git(attacker, "push", "-q", "origin", "HEAD:main")

    victim = clone("victim")
    runner = grim_at(victim)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("install", TRUST)
    state = victim / ".grimoire" / "state.json"
    real = json.loads(state.read_text(encoding="utf-8"))
    assert [record["kind"] for record in real["records"]] == ["skill"], "a real state file to be pulled over"
    _put(victim / LOCAL, MINE)

    # The second commit declares the package and carries a forged state file beside the real record.
    write_config(
        attacker,
        skills={"good": skill.fq},
        settings={"team": team.fq},
        options=CLAUDE_ONLY,
        experimental_settings=True,
    )
    attacker_runner.run("lock")
    forged = {"version": 2, "records": [*real["records"], *_forged_records(team)]}
    (attacker / ".grimoire").mkdir(exist_ok=True)
    (attacker / ".grimoire" / "state.json").write_text(json.dumps(forged), encoding="utf-8", newline="\n")
    git(attacker, "add", "grimoire.toml", "grimoire.lock")
    git(attacker, "add", "-f", ".grimoire/state.json")
    git(attacker, "commit", "-q", "-m", "settings")
    git(attacker, "push", "-q", "origin", "HEAD:main")

    git(victim, "pull", "-q", "--ff-only")

    delivered = json.loads(state.read_text(encoding="utf-8"))
    assert {record["name"] for record in delivered["records"]} == {"good", "team", "ghost"}, "the pull delivered it"
    result = runner.run("install", TRUST, format="json", check=False)

    error = _refused(result)
    assert error["reason"] == "untracked-destination"
    assert [(p["pointer"], p["current"]) for p in error["pieces"]] == [("/model", "opus")]
    assert _file(victim / LOCAL) == MINE
    assert _store_files(runner) == []
    _discarded(result)

    _then_the_ordinary_road_works(runner, victim, skill)
