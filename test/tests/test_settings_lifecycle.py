# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""How a settings piece lives on: offline runs, seats, removal and partial failure.

``adr_settings_artifact_kind.md``, Decision 5. The offline table comes first: a
run that changes nothing needs no layer, a removal needs none, and a plan that
needs a layer it cannot reach fails the settings rows and nothing else. Then
the lifecycle: orphans, ``grim uninstall``, a client set that grows or shrinks,
a relocated root, and a run stopped between two files. Fixtures come from
``make_settings``; each "nothing was written" sits beside a run that does.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import sys
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import (
    SETTINGS_LAYER_MEDIA_TYPE,
    make_artifact,
    make_bundle,
    make_settings,
    settings_sandbox,
    write_config,
)
from src.registry import REGISTRY_HOST, PublishedArtifact, fetch_manifest, push_manifest, retag, tag_digest
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
SKILL_DOC = "---\nname: good\ndescription: d\n---\n# Good\n"
DENY = "Bash(rm:*)"
ASK = "Read(./notes)"
USER_RULE = "Bash(curl:*)"


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


def _workspace(
    grim_at, workspace: Path, sandbox: Path, packages: dict[str, PublishedArtifact], **config: dict
) -> GrimRunner:
    """A Claude workspace locked with the flag off, whose flag then went on."""
    write_config(workspace, settings={name: package.fq for name, package in packages.items()}, **config)
    runner = grim_at(workspace)
    runner.env.update(settings_sandbox(sandbox, runner.home)[0])
    runner.run("lock")
    runner.run("config", "set", "options.experimental.settings", "true")
    return runner


def _offline(runner: GrimRunner, *flags: str, check: bool = True) -> CompletedProcess[str]:
    return runner.run("--offline", "install", TRUST, *flags, format="json", check=check)


def _forget_cache(runner: GrimRunner) -> None:
    """Drop every cached manifest and layer; the settings records stay."""
    shutil.rmtree(Path(runner.grim_home) / "blobs")


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    rows = json.loads(result.stdout)["settings_changes"]
    return [(row["package"], row["key"], row["element"], row["action"]) for row in rows]


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


def _skill(repo: str) -> PublishedArtifact:
    return make_artifact(f"{repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")


def _listed(rows: list[dict]) -> list[tuple[str, str, str | None, str]]:
    """A change list by key: a test names what moved, not the phase's walk."""
    listed = [(row["package"], row["key"], row["element"], row["action"]) for row in rows]
    return sorted(listed, key=lambda row: (row[0], row[1], row[2] or ""))


def _abandoned(rows: list[dict]) -> list[tuple[Path, str, str | None]]:
    return sorted((Path(row["path"]).resolve(), row["pointer"], row["element"]) for row in rows)


def _row(report: dict, name: str) -> dict:
    [row] = [item for item in report["items"] if item["name"] == name]
    return row


def _store_files(runner: GrimRunner) -> list[Path]:
    """The project records on this machine, or the global state when there are none."""
    project = sorted((Path(runner.grim_home) / "settings").glob("*.json"))
    if project:
        return project
    state = Path(runner.grim_home) / "state" / "global.json"
    return [state] if state.exists() else []


def _records(runner: GrimRunner) -> dict[tuple[str, str, str, str | None], bool]:
    """Each recorded piece, `(package, client, pointer, element)`, and its `adopted`."""
    return {
        (record["name"], output["client"], output["entry"], output.get("element")): output.get("adopted", False)
        for path in _store_files(runner)
        for record in json.loads(path.read_text(encoding="utf-8"))["records"]
        if record["kind"] == "settings"
        for output in record["outputs"]
    }


def _declare(home: Path, team: PublishedArtifact, clients: list[str], flag: bool = True) -> None:
    write_config(
        home, settings={"team": team.fq}, options={"clients": json.dumps(clients)}, experimental_settings=flag
    )


def _global(
    grim: GrimRunner, home: Path, sandbox: Path, team: PublishedArtifact, clients: list[str]
) -> dict[str, Path]:
    """A global scope naming `clients` and locking `team`, with nothing installed yet."""
    _declare(home, team, clients)
    env, roots = settings_sandbox(sandbox, grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")
    return roots


def _printed(stderr: str, purpose: str) -> list[str]:
    """The arguments after `grim` in the step a warning prints, ending in `purpose`."""
    match = re.search(rf"run `grim ([^`]+)` {purpose}", stderr)
    assert match, stderr
    return match.group(1).split()


def _relative_home(grim: GrimRunner, home: Path) -> None:
    """The same home, spelled so that it follows the cwd: the guard refuses it."""
    grim.cwd = home.parent
    grim.env["GRIM_HOME"] = home.name


def test_offline_intact_is_a_no_op(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    held = target.read_bytes()
    _forget_cache(runner)

    result = _offline(runner)

    assert _changes(result) == []
    assert "not cached" not in result.stderr
    assert target.read_bytes() == held

    # Positive control: with a piece drifted the same state needs the layer.
    target.write_text(json.dumps({"model": "opus"}), encoding="utf-8")
    result = _offline(runner, check=False)
    assert result.returncode == 81, result.stderr
    assert _file(target) == {"model": "opus"}


def test_offline_orphan_removal_needs_no_layer(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    write_config(project_dir, experimental_settings=True)
    runner.run("lock")
    _forget_cache(runner)
    target = project_dir / LOCAL

    # Positive control: declined, the orphan stays where it is.
    runner.run("--offline", "install", "--no-trust-workspace")
    assert _file(target) == {"model": "sonnet"}

    result = _offline(runner)

    assert _changes(result) == [("team", "/model", None, "removed")]
    assert _file(target) == {}


def test_offline_new_client_uncached_warns_and_exits_0(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}}
    team = _package(unique_repo, "team", tables)
    write_config(grim_home, settings={"team": team.fq}, experimental_settings=True)
    sandbox, roots = settings_sandbox(tmp_path / "s", grim.home)
    grim.env.update(sandbox)
    grim.run("--global", "lock")
    grim.run("--global", "install", "--client", "claude")
    claude, codex = roots["claude"] / "settings.json", roots["codex"] / "config.toml"
    assert _file(claude) == {"model": "sonnet"}
    assert not codex.exists()
    _forget_cache(grim)

    result = grim.run("--global", "--offline", "install", "--client", "claude", "--client", "codex", format="json")

    assert result.returncode == 0
    assert _changes(result) == []
    assert "team" in result.stderr and "codex" in result.stderr
    assert not codex.exists()

    # Positive control: online, the same run plans from the layer and writes.
    grim.run("--global", "install", "--client", "claude", "--client", "codex")
    assert 'model = "gpt"' in codex.read_text(encoding="utf-8")


def test_offline_uncached_does_not_claim_values_for_an_uncovered_client(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"codex": {"model": "gpt"}})
    _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    _forget_cache(grim)

    result = grim.run("--global", "--offline", "install", format="json")

    assert result.returncode == 0, result.stderr
    # Claude has no piece, and offline nothing on this machine says so.
    assert "not applied for claude" not in result.stderr
    assert "could not check" in result.stderr
    assert "run `grim --global install` online" in result.stderr


def test_offline_changed_plan_with_cached_layers_is_a_normal_run(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    clash = _package(unique_repo, "other", {"claude": {"model": "sonnet"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": first, "other": other})
    target = project_dir / LOCAL

    # Three online runs put all four layers in the cache, none of them refused.
    runner.run("install", TRUST)
    _repin(project_dir, other, clash)
    runner.run("install", TRUST)
    _repin(project_dir, clash, other)
    _repin(project_dir, first, second)
    runner.run("install", TRUST)
    assert _file(target) == {"model": "opus", "language": "en"}

    # A moved pin whose layer is cached is an ordinary run.
    _repin(project_dir, second, first)
    assert _changes(_offline(runner)) == [("team", "/model", None, "changed")]
    assert _file(target) == {"model": "sonnet", "language": "en"}

    # Including the conflict check, over the cached documents.
    _repin(project_dir, first, second)
    _repin(project_dir, other, clash)
    result = _offline(runner, check=False)
    assert result.returncode == 78, result.stderr
    assert _file(target) == {"model": "sonnet", "language": "en"}


def test_offline_uncached_layer_exits_81_for_settings_only(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    skill = make_artifact(f"{unique_repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": first}, skills={"good": skill.fq})
    runner.run("install", TRUST)
    target, installed = project_dir / LOCAL, project_dir / ".claude/skills/good"
    held = target.read_bytes()

    # A pin this machine never fetched, with the skill to reinstall from cache.
    _repin(project_dir, first, _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2"))
    shutil.rmtree(installed)

    result = _offline(runner, check=False)

    assert result.returncode == 81, result.stderr
    assert first.repo in json.loads(result.stdout)["error"]["message"]
    assert target.read_bytes() == held
    # Positive control: the failure is the settings rows', so the skill is back.
    assert (installed / "SKILL.md").is_file()

    # Positive control: online, the same plan writes.
    runner.run("install", TRUST)
    assert _file(target) == {"model": "opus"}


def test_a_grim_home_inside_the_workspace_writes_no_setting(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Records cannot live there, so the phase stops before it reads one."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    nested = project_dir / ".grim-home"
    nested.mkdir()
    home = runner.env["GRIM_HOME"]
    runner.env["GRIM_HOME"] = str(nested)

    result = runner.run("install", TRUST)

    assert "GRIM_HOME" in result.stderr
    assert not (project_dir / LOCAL).exists()
    assert not (nested / "settings").exists()

    # Positive control: the same lock under a machine-local home writes.
    runner.env["GRIM_HOME"] = home
    runner.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}


def test_orphan_removal_is_listed_on_update_with_no_consent(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    write_config(project_dir, settings={"other": other.fq}, experimental_settings=True)
    runner.run("lock")
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet", "language": "en"}

    report = runner.json("update")

    assert _file(target) == {"language": "en"}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]
    assert _row(report, "team")["action"] == "removed"
    assert [key[0] for key in _records(runner)] == ["other"]


def test_orphan_removal_is_listed_on_add_with_no_consent(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    write_config(project_dir, experimental_settings=True)
    runner.run("lock")
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet"}

    report = runner.json("add", _skill(unique_repo).fq)

    assert _file(target) == {}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]
    assert _records(runner) == {}


def test_a_named_update_leaves_an_orphan_outside_its_closure(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A named update prunes at the previous lock's closure, settings values included."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    good = _skill(unique_repo)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, skills={"good": good.fq})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet"}
    runner.run("remove", "settings", "team")

    result = runner.run("update", "good", format="json")

    assert _file(target) == {"model": "sonnet"}
    assert json.loads(result.stdout)["settings_changes"] == []
    assert [key[0] for key in _records(runner)] == ["team"]
    assert "the lock no longer holds settings 'team'" in result.stderr, result.stderr

    # Positive control: a bare update prunes the orphan.
    report = runner.json("update")
    assert _file(target) == {}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]


def _team_beside_good(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> tuple[GrimRunner, Path]:
    """`team` setting Claude's model beside the skill `good`, both installed."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    good = _skill(unique_repo)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, skills={"good": good.fq})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet"}
    return runner, target


# Each named update below passes the flag: without it a package nothing consented
# to is withheld, and the run leaves its value alone whatever its closure.
def test_a_named_update_leaves_an_unnamed_settings_edit(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, target = _team_beside_good(grim_at, project_dir, tmp_path, unique_repo)
    _put(target, {"model": "opus"})

    result = runner.run("update", TRUST, "good", format="json", check=False)

    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert (report["pieces"], report["settings_changes"]) == ([], [])
    assert _file(target) == {"model": "opus"}

    # Positive control: the whole lock's run refuses the same edit.
    assert runner.run("install", TRUST, check=False).returncode == 65


def test_force_on_a_named_update_leaves_an_unnamed_settings_edit(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, target = _team_beside_good(grim_at, project_dir, tmp_path, unique_repo)
    _put(target, {"model": "opus"})

    result = runner.run("update", TRUST, "--force", "good", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["settings_changes"] == []
    assert _file(target) == {"model": "opus"}

    # Positive control: a forced run over the whole lock takes the value back.
    runner.run("install", TRUST, "--force")
    assert _file(target) == {"model": "sonnet"}


def test_a_named_update_leaves_a_deleted_unnamed_settings_value(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, target = _team_beside_good(grim_at, project_dir, tmp_path, unique_repo)
    target.unlink()

    report = runner.json("update", TRUST, "good")

    assert not target.exists()
    assert report["settings_changes"] == []

    # Positive control: the whole lock's run restores it.
    report = runner.json("install", TRUST)
    assert _file(target) == {"model": "sonnet"}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "added")]


def test_a_named_update_writes_its_closure_beside_an_unnamed_edit(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet", "language": "de"})
    _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="stable")

    result = runner.run("update", TRUST, "team", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert _file(target) == {"model": "opus", "language": "de"}
    assert _listed(json.loads(result.stdout)["settings_changes"]) == [("team", "/model", None, "changed")]


def test_a_named_update_refuses_an_edit_inside_its_closure(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "opus"})
    _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="stable")

    result = runner.run("update", TRUST, "team", format="json", check=False)

    assert result.returncode == 65, result.stderr
    report = json.loads(result.stdout)
    assert _row(report, "team")["refused"] is True
    assert [piece["pointer"] for piece in report["pieces"]] == ["/model"]
    assert _file(target) == {"model": "opus"}

    report = runner.json("update", TRUST, "--force", "team")

    assert _file(target) == {"model": "haiku"}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "overwritten")]


def test_a_named_update_refuses_a_conflict_outside_its_closure(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    target, lock = project_dir / LOCAL, project_dir / "grimoire.lock"
    held = lock.read_bytes()
    _package(unique_repo, "team", {"claude": {"language": "de"}}, tag="stable")

    result = runner.run("update", TRUST, "team", format="json", check=False)

    assert result.returncode == 78, result.stderr
    assert "other" in result.stdout
    assert lock.read_bytes() == held
    assert _file(target) == {"model": "sonnet", "language": "en"}


def test_a_named_update_keeps_an_element_a_package_outside_it_holds(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    shared = {"permissions": {"deny": [DENY]}}
    team = _package(unique_repo, "team", {"claude": {**shared, "model": "sonnet"}}, tag="stable")
    other = _package(unique_repo, "other", {"claude": shared})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")

    report = runner.json("update", TRUST, "team")

    assert _file(target) == {"model": "sonnet", **shared}
    assert report["settings_changes"] == []
    held = {key[0] for key in _records(runner) if key[2] == "/permissions/deny"}
    assert held == {"other"}


def _team_dropping_model_beside_other(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, *consent: str
) -> tuple[GrimRunner, Path]:
    """`team` installed setting model and language, `other` declared with the same
    model and no record, then `team`'s tag moved to a version without the model.

    ``consent`` is run after the install and before `other` is declared."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    if consent:
        runner.run(*consent)
    other = _package(unique_repo, "other", {"claude": {"model": "sonnet"}})
    runner.run("add", "--kind", "settings", "--name", "other", other.fq, "--no-install")
    _package(unique_repo, "team", {"claude": {"language": "en"}}, tag="stable")
    return runner, project_dir / LOCAL


def test_a_named_update_keeps_a_value_an_armed_package_outside_it_declares(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, target = _team_dropping_model_beside_other(grim_at, project_dir, tmp_path, unique_repo)

    result = runner.run("update", TRUST, "team", format="json")

    assert _file(target) == {"model": "sonnet", "language": "en"}
    report = json.loads(result.stdout)
    assert (report["pieces"], report["settings_changes"]) == ([], [])
    assert [item["name"] for item in report["items"]] == ["team"]
    assert "other" not in {key[0] for key in _records(runner)}
    kept = (
        "settings package 'team' drops /model, but the value stays: 'other' outside this update also sets it; "
        f"run `grim install {TRUST}` to apply 'other'"
    )
    assert kept in result.stderr, result.stderr

    # The whole lock's run hands the value to `other` as grim's own, not adopted.
    report = runner.json("install", TRUST)
    assert _file(target) == {"model": "sonnet", "language": "en"}
    assert report["settings_changes"] == []
    assert _records(runner) == {
        ("team", "claude", "/language", None): False,
        ("other", "claude", "/model", None): False,
    }


def test_a_named_update_removes_a_value_only_an_unconsented_package_declares(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The record covers `team` alone, so `other` is not armed and holds nothing."""
    runner, target = _team_dropping_model_beside_other(
        grim_at, project_dir, tmp_path, unique_repo, "consent", "allow"
    )

    report = runner.json("update", "team")

    assert _file(target) == {"language": "en"}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]


def _pack_dropping_team(
    grim_at,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
    *,
    beside_other: bool,
    skills: dict[str, str] | None = None,
) -> tuple[GrimRunner, Path]:
    """Bundle `pack` holding settings `team` (model) and skill `good`, installed,
    then `pack:1` moved to a version without `team`. With ``beside_other``,
    `other` declares the same model, added with no install and no record.
    ``skills`` are declared beside `pack` and installed with it."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    good = _skill(unique_repo)
    members = [("settings", "team", team.fq), ("skill", "good", good.fq)]
    pack = make_bundle(f"{unique_repo}/pack", members, tag="1")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {}, bundles={"pack": pack.fq}, skills=skills or {})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet"}
    if beside_other:
        other = _package(unique_repo, "other", {"claude": {"model": "sonnet"}})
        runner.run("add", "--kind", "settings", "--name", "other", other.fq, "--no-install")
    retag(f"{unique_repo}/pack", "kept", pack.digest)
    make_bundle(f"{unique_repo}/pack", [("skill", "good", good.fq)], tag="1")
    return runner, target


def test_a_named_update_keeps_a_value_the_one_armed_package_outside_it_declares(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A bundle that drops a settings member left a value only that member held,
    while the one other armed package, outside the update, declares it. The
    value stays until `grim install` hands it over, with no window where the
    file lacks it."""
    runner, target = _pack_dropping_team(grim_at, project_dir, tmp_path, unique_repo, beside_other=True)

    result = runner.run("update", TRUST, "pack", format="json")

    assert _file(target) == {"model": "sonnet"}
    assert json.loads(result.stdout)["settings_changes"] == []
    assert "'other'" in result.stderr, result.stderr
    assert f"run `grim install {TRUST}`" in result.stderr, result.stderr
    # One remedy for the kept record: uninstalling it would remove what `other` declares.
    assert "uninstall settings team" not in result.stderr, result.stderr

    report = runner.json("install", TRUST)
    assert _file(target) == {"model": "sonnet"}
    assert report["settings_changes"] == []
    assert _records(runner) == {("other", "claude", "/model", None): False}


def test_the_kept_value_remedy_repeats_the_answer_that_armed_the_package_outside(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """No consent record and no terminal: only `--trust-workspace` armed `other`.
    The printed install has to carry that answer, or it applies nothing for
    `other` and removes the value the warning said stays."""
    runner, target = _pack_dropping_team(grim_at, project_dir, tmp_path, unique_repo, beside_other=True)

    result = runner.run("update", TRUST, "pack", format="json")

    printed = _printed(result.stderr, "to apply 'other'")
    assert printed == ["install", TRUST], result.stderr
    runner.run(*printed)
    assert _file(target) == {"model": "sonnet"}
    assert _records(runner) == {("other", "claude", "/model", None): False}


def test_the_kept_value_remedy_clears_a_value_changed_outside_grim_first(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The kept value was edited by hand, so `other`'s install refuses it without
    `--force`. The printed step deletes it first, then runs that install as
    printed: it applies `other` and keeps the hand edit of the unrelated skill
    `u`. The warning names `--force` as the other route, with all it reaches.
    Found by review: it printed `--force` alone, which also removes the edited
    values of packages that left the lock."""
    u = make_artifact(f"{unique_repo}/u", "skill", {"u/SKILL.md": "---\nname: u\ndescription: d\n---\n# U\n"}, tag="1")
    runner, target = _pack_dropping_team(
        grim_at, project_dir, tmp_path, unique_repo, beside_other=True, skills={"u": u.fq}
    )
    skill = project_dir / ".claude" / "skills" / "u" / "SKILL.md"
    skill.write_text(skill.read_text(encoding="utf-8") + "\nmy edit\n", encoding="utf-8")
    target.write_text(json.dumps({"model": "opus"}))

    result = runner.run("update", TRUST, "pack", format="json")

    assert _file(target) == {"model": "opus"}
    [warning] = [line for line in result.stderr.splitlines() if "to apply 'other'" in line]
    assert "delete or restore it to the value grim wrote, then run" in warning, warning
    assert "removes those of artifacts that left the lock" in warning, warning
    printed = _printed(result.stderr, "to apply 'other'")
    assert printed == ["install", TRUST], warning
    target.write_text(json.dumps({}))
    followed = runner.run(*printed, check=False)
    assert followed.returncode == 65, followed.stderr
    assert "skill 'u'" in followed.stderr or "/u" in followed.stderr, followed.stderr
    assert _file(target) == {"model": "sonnet"}
    assert _records(runner) == {("other", "claude", "/model", None): False}
    assert skill.read_text(encoding="utf-8").endswith("my edit\n"), "the printed step overwrote another edit"


@pytest.mark.parametrize("left", [False, True], ids=["in-lock", "left-the-lock"])
def test_the_kept_value_remedy_says_another_edited_value_holds_the_install_back(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, left: bool
) -> None:
    """Package `x`'s value at the same scope was also edited by hand, with `x`
    in the lock or gone from it. The printed install refuses that value and
    writes no settings, so the step's deleted value stays empty. The warning
    says so, and once `x`'s value is restored to the one grim wrote, the
    install applies `other`. Found by review: nothing said why the value
    the user deleted was not replaced."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    good = _skill(unique_repo)
    pack = make_bundle(f"{unique_repo}/pack", [("settings", "team", team.fq), ("skill", "good", good.fq)], tag="1")
    x = _package(unique_repo, "x", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"x": x}, bundles={"pack": pack.fq})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet", "language": "en"}
    if left:
        config = project_dir / "grimoire.toml"
        config.write_text(re.sub(r"(?m)^x\s*=.*\n", "", config.read_text(encoding="utf-8")), encoding="utf-8")
        runner.run("lock")
    other = _package(unique_repo, "other", {"claude": {"model": "haiku"}})
    runner.run("add", "--kind", "settings", "--name", "other", other.fq, "--no-install")
    retag(f"{unique_repo}/pack", "kept", pack.digest)
    make_bundle(f"{unique_repo}/pack", [("skill", "good", good.fq)], tag="1")
    target.write_text(json.dumps({"model": "opus", "language": "fr"}))

    result = runner.run("update", TRUST, "pack", format="json")

    [warning] = [line for line in result.stderr.splitlines() if "to apply 'other'" in line]
    assert "restore it to the value grim wrote" in warning, warning
    assert "writes no settings while it refuses any other settings value at this scope" in warning, warning
    printed = _printed(result.stderr, "to apply 'other'")
    target.write_text(json.dumps({"language": "fr"}))
    held = runner.run(*printed, check=False)
    assert held.returncode == 65, held.stderr
    assert _file(target) == {"language": "fr"}
    target.write_text(json.dumps({"language": "en"}))
    runner.run(*printed)
    assert _file(target) == ({"model": "haiku"} if left else {"model": "haiku", "language": "en"})


def _drop_stored_layer(runner: GrimRunner, repo: str, tag: str) -> None:
    """Delete any stored copy of `repo:tag`'s layer, keeping its manifest."""
    layer = fetch_manifest(repo, tag)["layers"][0]["digest"].split(":")[1]
    for root, _, names in os.walk(runner.grim_home):
        for name in names:
            if layer in name:
                (Path(root) / name).unlink()


def test_an_offline_named_update_keeps_a_value_an_unread_package_outside_it_may_set(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Offline, the one armed package outside the update has no stored layer, so
    nothing says whether it sets the value the dropped member held. The run
    stops with 81 and keeps the value, where it used to remove it at exit 0."""
    runner, target = _pack_dropping_team(grim_at, project_dir, tmp_path, unique_repo, beside_other=True)
    # Another project on the same home reads the moved tag, so the offline run follows it.
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    write_config(elsewhere, bundles={"pack": f"{REGISTRY_HOST}/{unique_repo}/pack:1"})
    GrimRunner(runner.binary, Path(runner.grim_home), cwd=elsewhere).run("lock")
    _drop_stored_layer(runner, f"{unique_repo}/other", "1")

    result = runner.run("--offline", "update", TRUST, "pack", format="json", check=False)

    assert result.returncode == 81, result.stderr
    assert _file(target) == {"model": "sonnet"}
    assert "settings package 'team' keeps /model: 'other' outside this update" in result.stderr, result.stderr
    # One remedy for the kept record: uninstalling it would remove what `other` may set.
    assert _printed(result.stderr, "once its layer can be") == ["install", TRUST], result.stderr
    assert "uninstall settings team" not in result.stderr, result.stderr

    # Positive control: online, the install reads the layer and hands the value over.
    report = runner.json("install", TRUST)
    assert _file(target) == {"model": "sonnet"}
    assert report["settings_changes"] == []
    assert _records(runner) == {("other", "claude", "/model", None): False}


def test_a_named_update_keeps_a_value_a_package_outside_it_with_an_unreadable_layer_may_set(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Online, the registry serves `other`'s manifest but no reader accepts its
    layer. The run stops on that layer's own error and keeps the value, as the
    whole-lock run does, where it used to remove it at exit 0."""
    runner, target = _pack_dropping_team(grim_at, project_dir, tmp_path, unique_repo, beside_other=True)
    unreadable = push_manifest(
        f"{unique_repo}/other",
        "broken",
        [(SETTINGS_LAYER_MEDIA_TYPE, b"{not json")],
        {"com.grimoire.kind": "settings"},
    )
    lock = project_dir / "grimoire.lock"
    intact = lock.read_text(encoding="utf-8")
    pinned = tag_digest(f"{unique_repo}/other", "1")
    assert pinned in intact
    lock.write_text(intact.replace(pinned, unreadable), encoding="utf-8")

    result = runner.run("update", TRUST, "pack", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert "other" in json.loads(result.stdout)["error"]["message"]
    assert _file(target) == {"model": "sonnet"}
    assert "settings package 'team' keeps /model: 'other' outside this update" in result.stderr, result.stderr
    assert "uninstall settings team" not in result.stderr, result.stderr


def test_a_named_force_removes_an_edited_value_its_bundle_dropped(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A named `--force` reaches the values the run prunes, as it reaches the
    files it prunes: an edited value of a dropped member goes on the first run."""
    runner, target = _pack_dropping_team(grim_at, project_dir, tmp_path, unique_repo, beside_other=False)
    _put(target, {"model": "opus"})

    result = runner.run("update", TRUST, "--force", "pack", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert _file(target) == {}
    assert _listed(json.loads(result.stdout)["settings_changes"]) == [("team", "/model", None, "removed")]
    assert _records(runner) == {}


def test_a_named_refusal_names_the_uninstall_that_clears_a_dropped_value(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The run saved the lock without the member, so a named rerun cannot reach
    the refused value; the warning names the command that can."""
    runner, target = _pack_dropping_team(grim_at, project_dir, tmp_path, unique_repo, beside_other=False)
    _put(target, {"model": "opus"})

    result = runner.run("update", TRUST, "pack", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert _file(target) == {"model": "opus"}
    remedy = re.search(r"`grim (uninstall --force settings team)`", result.stderr)
    assert remedy, result.stderr
    # One remedy: without `--force` the uninstall refuses the edited value too,
    # and a named rerun with `--force` no longer prunes `team`.
    assert "`grim uninstall settings team`" not in result.stderr, result.stderr
    assert "rerun with --force" not in result.stderr, result.stderr

    # The printed command clears it, and moves no pin.
    lock = (project_dir / "grimoire.lock").read_bytes()
    runner.run(*remedy.group(1).split())
    assert _file(target) == {}
    assert _records(runner) == {}
    assert (project_dir / "grimoire.lock").read_bytes() == lock


def test_a_projected_install_leaves_other_packages_alone(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """`add` installs one projection of the lock, and the phase plans over the whole lock."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    pending = _package(unique_repo, "pending", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    held = target.read_bytes()
    recorded = _records(runner)

    runner.run("add", pending.fq, "--no-install")
    report = runner.json("add", _skill(unique_repo).fq)

    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()
    assert report["settings_changes"] == []
    assert target.read_bytes() == held
    assert _records(runner) == recorded

    # Positive control: the workspace's own answer arms what `--no-install` left.
    runner.run("install", TRUST)
    assert _file(target) == {"model": "sonnet", "language": "en"}


def test_dev_refresh_never_plans_settings(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A dev seat works from a one-entry lock, so a plan would read every package as an orphan."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    held = target.read_bytes()
    recorded = _records(runner)
    source = project_dir / "dev-skill" / "SKILL.md"
    source.parent.mkdir()
    source.write_text("---\nname: dev-skill\ndescription: d\n---\n# One\n", encoding="utf-8")

    runner.run("install", "./dev-skill", "--client", "claude")

    rendered = project_dir / ".claude/skills/dev-skill/SKILL.md"
    assert "# One" in rendered.read_text(encoding="utf-8")
    assert target.read_bytes() == held
    assert _records(runner) == recorded

    source.write_text("---\nname: dev-skill\ndescription: d\n---\n# Two\n", encoding="utf-8")
    report = runner.json("update", "--client", "claude")

    assert "# Two" in rendered.read_text(encoding="utf-8")
    assert report["settings_changes"] == []
    assert target.read_bytes() == held
    assert _records(runner) == recorded


def test_adopted_value_survives_uninstall(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)
    assert _records(runner)[("team", "claude", "/model", None)] is True

    report = runner.json("uninstall", "settings", "team")

    assert report["status"] == "uninstalled"
    # Positive control: the piece grim wrote left, in the same run.
    assert _listed(report["settings_changes"]) == [("team", "/language", None, "removed")]
    assert _file(target) == {"model": "sonnet"}
    assert _abandoned(report["abandoned_entries"]) == [(target.resolve(), "/model", None)]
    assert _records(runner) == {}
    assert "team" not in (project_dir / "grimoire.toml").read_text(encoding="utf-8")


def test_force_leaves_an_intact_adopted_value(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)

    report = runner.json("uninstall", "settings", "team", "--force")

    # Positive control: the piece grim wrote left, in the same run.
    assert _listed(report["settings_changes"]) == [("team", "/language", None, "removed")]
    assert _file(target) == {"model": "sonnet"}
    assert _abandoned(report["abandoned_entries"]) == [(target.resolve(), "/model", None)]


def test_forced_piece_is_removed_on_uninstall(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    _put(target, {"model": "opus"})
    assert runner.run("install", TRUST, check=False).returncode == 65
    runner.run("install", TRUST, "--force")
    assert _file(target) == {"model": "sonnet"}

    report = runner.json("uninstall", "settings", "team")

    assert _file(target) == {}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]
    assert report["abandoned_entries"] == []


def test_uninstall_drifted_refuses_and_force_removes(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    tables = {"claude": {"model": "sonnet", "language": "en", "spinnerTipsEnabled": True}}
    team = _package(unique_repo, "team", tables)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    drifted = {"model": "opus", "language": "de", "spinnerTipsEnabled": True}
    _put(target, drifted)
    declared = (project_dir / "grimoire.toml").read_bytes()
    recorded = _records(runner)

    result = runner.run("uninstall", "settings", "team", format="json", check=False)

    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert (error["reason"], error["forceable"]) == ("modified", True)
    assert sorted(piece["pointer"] for piece in error["pieces"]) == ["/language", "/model"]
    # All of it refuses together, the intact piece included.
    assert _file(target) == drifted
    assert _records(runner) == recorded
    assert (project_dir / "grimoire.toml").read_bytes() == declared

    report = runner.json("uninstall", "settings", "team", "--force")

    assert _file(target) == {}
    assert [row[1] for row in _listed(report["settings_changes"])] == ["/language", "/model", "/spinnerTipsEnabled"]
    assert _records(runner) == {}


def test_uninstall_refuses_a_drifted_adopted_piece(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)
    _put(target, {"model": "opus"})

    result = runner.run("uninstall", "settings", "team", format="json", check=False)

    assert result.returncode == 65, result.stderr
    assert [piece["pointer"] for piece in json.loads(result.stdout)["error"]["pieces"]] == ["/model"]
    assert _file(target) == {"model": "opus"}

    report = runner.json("uninstall", "settings", "team", "--force")

    assert _file(target) == {}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]
    assert report["abandoned_entries"] == []


def test_uninstall_works_with_the_flag_off(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    target = project_dir / LOCAL

    # Positive control: the flag is off, so a dropped package keeps its value.
    write_config(project_dir, settings={"team": team.fq}, experimental_settings=False)
    runner.run("lock")
    runner.run("install")
    assert _file(target) == {"model": "sonnet", "language": "en"}

    report = runner.json("uninstall", "settings", "team")

    assert _file(target) == {"language": "en"}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "removed")]


def test_uninstall_deep_pointer_and_element(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"permissions": {"deny": [DENY], "ask": [ASK]}, "sandbox": {"enabled": True}}}
    team = _package(unique_repo, "team", tables)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    users = {"theme": "dark", "permissions": {"deny": [USER_RULE]}}
    _put(target, users)
    runner.run("install", TRUST)
    assert _file(target)["sandbox"] == {"enabled": True}
    assert sorted(_file(target)["permissions"]["deny"]) == sorted([DENY, USER_RULE])

    report = runner.json("uninstall", "settings", "team")

    # What grim emptied goes, up to what still holds a value of the user's.
    assert _file(target) == users
    assert _listed(report["settings_changes"]) == [
        ("team", "/permissions/ask", ASK, "removed"),
        ("team", "/permissions/deny", DENY, "removed"),
        ("team", "/sandbox/enabled", None, "removed"),
    ]


def test_shared_element_survives_one_uninstall(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    shared = {"language": "en", "permissions": {"deny": [DENY]}}
    alpha = _package(unique_repo, "alpha", {"claude": {**shared, "model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": shared})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"alpha": alpha, "beta": beta})
    runner.run("install", TRUST)
    target = project_dir / LOCAL

    report = runner.json("uninstall", "settings", "alpha")

    assert _file(target) == shared
    assert _listed(report["settings_changes"]) == [("alpha", "/model", None, "removed")]
    assert report["abandoned_entries"] == []
    assert {key[0] for key in _records(runner)} == {"beta"}

    # Positive control: the last declarer takes the shared pieces with it.
    report = runner.json("uninstall", "settings", "beta")
    assert _file(target) == {}
    assert [row[1] for row in _listed(report["settings_changes"])] == ["/language", "/permissions/deny"]


def test_uninstall_of_an_unreadable_file_drops_the_record(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    target.write_text("{ not json", encoding="utf-8")

    report = runner.json("uninstall", "settings", "team")

    assert target.read_text(encoding="utf-8") == "{ not json"
    assert _abandoned(report["abandoned_entries"]) == [(target.resolve(), "/model", None)]
    assert _records(runner) == {}


def test_global_uninstall_under_a_refused_home_keeps_the_record(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Nothing is stranded: a later run under a good home judges the value through the rule."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude"])
    grim.run("--global", "install")
    target = roots["claude"] / "settings.json"
    store = grim_home / "state" / "global.json"
    held = store.read_bytes()
    assert _file(target) == {"model": "sonnet"}
    _relative_home(grim, grim_home)

    result = grim.run("--global", "uninstall", "settings", "team")

    assert result.returncode == 0, result.stderr
    assert "GRIM_HOME" in result.stderr and "left" in result.stderr
    assert "team" not in (grim_home / "grimoire.toml").read_text(encoding="utf-8")
    assert _file(target) == {"model": "sonnet"}
    assert store.read_bytes() == held

    # Positive control: the record is an orphan now, and the good home removes the value.
    grim.env["GRIM_HOME"] = str(grim_home)
    grim.run("--global", "install", "--client", "claude")
    assert _file(target) == {}
    assert _records(grim) == {}


def test_client_added_later_gets_its_pieces(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"model": "sonnet"}, "codex": {"model": "gpt", "model_reasoning_effort": "high"}}
    team = _package(unique_repo, "team", tables)
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude"])
    grim.run("--global", "install")
    codex = roots["codex"] / "config.toml"
    assert not codex.exists()
    codex.write_text('model_reasoning_effort = "high"\n', encoding="utf-8")
    _declare(grim_home, team, ["claude", "codex"])

    result = grim.run("--global", "install", format="json")

    assert _changes(result) == [("team", "/model", None, "added")]
    assert 'model = "gpt"' in codex.read_text(encoding="utf-8")
    recorded = _records(grim)
    assert recorded[("team", "codex", "/model", None)] is False
    assert recorded[("team", "codex", "/model_reasoning_effort", None)] is True


def test_dropped_client_keeps_user_elements(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {
        "claude": {"model": "sonnet", "permissions": {"deny": [DENY]}},
        "codex": {"model": "gpt"},
    }
    team = _package(unique_repo, "team", tables)
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    claude, codex = roots["claude"] / "settings.json", roots["codex"] / "config.toml"
    users = {"theme": "dark", "permissions": {"deny": [USER_RULE]}}
    _put(claude, users)
    grim.run("--global", "install")
    assert sorted(_file(claude)["permissions"]["deny"]) == sorted([DENY, USER_RULE])
    _declare(grim_home, team, ["codex"])

    report = grim.json("--global", "update")

    assert _file(claude) == users
    assert 'model = "gpt"' in codex.read_text(encoding="utf-8")
    assert _listed(report["settings_changes"]) == [
        ("team", "/model", None, "removed"),
        ("team", "/permissions/deny", DENY, "removed"),
    ]
    row = _row(report, "team")
    assert (row["reaped_clients"], row["kept_modified_clients"]) == (["claude"], [])
    assert {key[1] for key in _records(grim)} == {"codex"}


def test_dropped_client_reports_its_adopted_value(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    codex = roots["codex"] / "config.toml"
    codex.write_text('model = "gpt"\n', encoding="utf-8")
    grim.run("--global", "install")
    _declare(grim_home, team, ["claude"])

    report = grim.json("--global", "update")

    assert codex.read_text(encoding="utf-8") == 'model = "gpt"\n'
    assert report["settings_changes"] == []
    row = _row(report, "team")
    assert row["reaped_clients"] == ["codex"]
    assert _abandoned(row["abandoned_entries"]) == [(codex.resolve(), "/model", None)]
    assert {key[1] for key in _records(grim)} == {"claude"}


def test_dropped_client_refuses_a_drifted_piece(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    claude = roots["claude"] / "settings.json"
    _put(claude, {"model": "opus"})
    _declare(grim_home, team, ["codex"])

    result = grim.run("--global", "update", format="json", check=False)

    assert result.returncode == 65, result.stderr
    report = json.loads(result.stdout)
    assert [piece["pointer"] for piece in report["pieces"]] == ["/model"]
    row = _row(report, "team")
    assert (row["reaped_clients"], row["kept_modified_clients"], row["refused"]) == ([], ["claude"], True)
    assert _file(claude) == {"model": "opus"}

    report = grim.json("--global", "update", "--force")

    assert _file(claude) == {}
    assert _row(report, "team")["reaped_clients"] == ["claude"]


def test_flag_off_client_shrink_touches_nothing(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    claude = roots["claude"] / "settings.json"
    recorded = _records(grim)
    _declare(grim_home, team, ["codex"], flag=False)

    report = grim.json("--global", "update")

    assert _file(claude) == {"model": "sonnet"}
    assert _records(grim) == recorded
    assert _row(report, "team")["reaped_clients"] == []

    # Positive control: the same narrowing with the flag on removes the value.
    _declare(grim_home, team, ["codex"])
    assert _row(grim.json("--global", "update"), "team")["reaped_clients"] == ["claude"]
    assert _file(claude) == {}


def test_a_refused_home_reaps_no_client(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    claude = roots["claude"] / "settings.json"
    _declare(grim_home, team, ["codex"])
    _relative_home(grim, grim_home)

    result = grim.run("--global", "update")

    assert result.returncode == 0, result.stderr
    assert "GRIM_HOME" in result.stderr
    assert _file(claude) == {"model": "sonnet"}

    # Positive control: every record survived, so a good home reaps the client.
    grim.env["GRIM_HOME"] = str(grim_home)
    grim.run("--global", "update")
    assert _file(claude) == {}


def test_a_relocated_root_leaves_the_old_value_named(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"cursor": {"approvalMode": "allowlist"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["cursor"])
    value = {"approvalMode": "allowlist"}
    old = Path(grim.env["XDG_CONFIG_HOME"]) / "cursor" / "cli-config.json"
    _put(old, {})
    result = grim.run("--global", "install")
    if sys.platform != "linux":
        # Only Linux falls back to the XDG file, so elsewhere the root never moves from it.
        assert _file(old) == {}
        assert "client-config-absent" in result.stderr
        return
    assert _file(old) == value
    new = roots["cursor"] / "cli-config.json"
    _put(new, {})

    report = grim.json("--global", "update")

    assert _file(new) == value
    assert _file(old) == value
    assert _listed(report["settings_changes"]) == [("team", "/approvalMode", None, "added")]
    assert _abandoned(_row(report, "team")["abandoned_entries"]) == [(old.resolve(), "/approvalMode", None)]
    assert len(_records(grim)) == 1

    # Once named it is the user's: the next run has nothing left to say.
    again = grim.json("--global", "update")
    assert _row(again, "team")["abandoned_entries"] == []
    assert again["settings_changes"] == []


def test_a_narrowed_install_after_a_pin_roll_keeps_the_record_on_the_old_pin(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", first, ["claude", "codex"])
    grim.run("--global", "install")
    claude, codex = roots["claude"] / "settings.json", roots["codex"] / "config.toml"
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}, "codex": {"model": "gpt-5"}}, tag="2")
    _repin(grim_home, first, second)

    result = grim.run("--global", "install", "--client", "claude", format="json")

    assert result.returncode == 0, result.stderr
    assert _file(claude) == {"model": "opus"}
    assert codex.read_text(encoding="utf-8") == 'model = "gpt"\n'
    # Codex still holds the old pin's value, so the row cannot read as current.
    assert _row(grim.json("--global", "status"), "team")["state"] == "outdated"
    assert "settings package 'team': codex still holds the previous version's values" in result.stderr
    remedy = _printed(result.stderr, "to move them")
    assert remedy == ["--global", "install", "--client", "codex"]

    # Positive control: the printed step, typed as shown away from any project,
    # moves codex and the record, and sends nobody back to claude.
    grim.cwd = tmp_path
    again = grim.run(*remedy)
    assert codex.read_text(encoding="utf-8") == 'model = "gpt-5"\n'
    assert "--client claude" not in again.stderr
    assert _row(grim.json("--global", "status"), "team")["state"] == "installed"


def test_a_narrowed_install_names_every_client_left_behind_in_one_step(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    tables = {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}, "copilot": {"model": "m1"}}
    first = _package(unique_repo, "team", tables)
    _global(grim, grim_home, tmp_path / "s", first, ["claude", "codex", "copilot"])
    grim.run("--global", "install")
    tables = {"claude": {"model": "opus"}, "codex": {"model": "gpt-5"}, "copilot": {"model": "m2"}}
    second = _package(unique_repo, "team", tables, tag="2")
    _repin(grim_home, first, second)

    result = grim.run("--global", "install", "--client", "claude")

    assert "settings package 'team': codex, copilot still hold the previous version's values" in result.stderr
    remedy = _printed(result.stderr, "to move them")
    assert remedy == ["--global", "install", "--client", "codex", "--client", "copilot"]


def test_a_narrowed_install_moves_the_record_when_the_others_keep_their_values(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", first, ["claude", "codex"])
    grim.run("--global", "install")
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}, "codex": {"model": "gpt"}}, tag="2")
    _repin(grim_home, first, second)

    result = grim.run("--global", "install", "--client", "claude")

    # Codex already holds what the new pin writes, so nothing is left to move.
    assert "still hold" not in result.stderr
    assert (roots["codex"] / "config.toml").read_text(encoding="utf-8") == 'model = "gpt"\n'
    assert _row(grim.json("--global", "status"), "team")["state"] == "installed"
    assert grim.json("--global", "install")["settings_changes"] == []


def test_a_client_dropped_from_the_config_is_sent_to_update(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", first, ["claude", "codex"])
    grim.run("--global", "install")
    # The same tag, so the update the warning names resolves to the same pin.
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}, "codex": {"model": "gpt-5"}})
    _declare(grim_home, first, ["claude"])
    _repin(grim_home, first, second)

    result = grim.run("--global", "install")

    assert (
        "settings package 'team': codex still holds the previous version's values but is no longer in "
        "`[options].clients`" in result.stderr
    )
    assert "--client codex" not in result.stderr
    remedy = _printed(result.stderr, "to take them out")
    assert remedy == ["--global", "update"]

    # Positive control: the printed step reaps codex, and the record moves.
    grim.cwd = tmp_path
    update = grim.run(*remedy, format="json")
    assert _row(json.loads(update.stdout), "team")["reaped_clients"] == ["codex"]
    assert _file(roots["claude"] / "settings.json") == {"model": "opus"}
    assert _row(grim.json("--global", "status"), "team")["state"] == "installed"


def test_an_undetected_client_with_unchanged_values_does_not_hold_the_pin(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    write_config(grim_home, settings={"team": first.fq}, experimental_settings=True)
    env, roots = settings_sandbox(tmp_path / "s", grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")
    grim.run("--global", "install")
    assert (roots["codex"] / "config.toml").exists()
    # Autodetect: codex's root is gone, so this run no longer selects codex.
    shutil.rmtree(roots["codex"])
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}, "codex": {"model": "gpt"}}, tag="2")
    _repin(grim_home, first, second)

    result = grim.run("--global", "install")

    assert "still hold" not in result.stderr
    assert _file(roots["claude"] / "settings.json") == {"model": "opus"}
    assert _row(grim.json("--global", "status"), "team")["state"] == "installed"


def test_a_pin_roll_that_drops_a_client_moves_the_record(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", first, ["claude", "codex"])
    grim.run("--global", "install")
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}, "codex": {"model": "gpt-5"}}, tag="2")
    _declare(grim_home, second, ["claude"])

    result = grim.run("--global", "update", format="json")

    # The reap after the pass takes codex out, so nothing stays on the old version.
    assert _row(json.loads(result.stdout), "team")["reaped_clients"] == ["codex"]
    assert "--client codex" not in result.stderr
    assert _file(roots["claude"] / "settings.json") == {"model": "opus"}
    assert _row(grim.json("--global", "status"), "team")["state"] == "installed"


def test_a_named_update_keeps_the_pin_a_dropped_client_holds(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", first, ["claude", "codex"])
    grim.run("--global", "install")
    codex = roots["codex"] / "config.toml"
    # The same tag, so the declaration still matches the lock a named run refreshes.
    _package(unique_repo, "team", {"claude": {"model": "opus"}, "codex": {"model": "gpt-5"}})
    _declare(grim_home, first, ["claude"])

    result = grim.run("--global", "update", "team", format="json")

    # A named run reaps nothing, so codex keeps the old value and the record its pin.
    assert _row(json.loads(result.stdout), "team")["reaped_clients"] == []
    assert codex.read_text(encoding="utf-8") == 'model = "gpt"\n'
    assert "codex still holds the previous version's values but is no longer in" in result.stderr
    assert _row(grim.json("--global", "status"), "team")["state"] == "outdated"

    # Positive control: with codex selected again, the record's old pin moves codex too.
    _declare(grim_home, first, ["claude", "codex"])
    grim.run("--global", "install")
    assert codex.read_text(encoding="utf-8") == 'model = "gpt-5"\n'


def test_zero_output_record_converges_when_the_cursor_file_appears(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"cursor": {"approvalMode": "allowlist"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["cursor"])

    report = grim.json("--global", "install")

    assert _row(report, "team")["status"] == "skipped"
    assert report["settings_changes"] == []
    assert _records(grim) == {}

    target = roots["cursor"] / "cli-config.json"
    _put(target, {"version": 1})

    report = grim.json("--global", "install")

    assert _file(target) == {"version": 1, "approvalMode": "allowlist"}
    assert _row(report, "team")["status"] == "installed"
    assert _listed(report["settings_changes"]) == [("team", "/approvalMode", None, "added")]
    assert len(_records(grim)) == 1


def test_add_refuses_a_conflict_before_it_saves(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    clash = _package(unique_repo, "clash", {"claude": {"model": "opus"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("hook", "allow")
    runner.run("install")
    target = project_dir / LOCAL
    held = target.read_bytes()
    config, lock = project_dir / "grimoire.toml", project_dir / "grimoire.lock"
    config_before, lock_before = config.read_bytes(), lock.read_bytes()
    consent = sorted(path.read_bytes() for path in (Path(runner.grim_home) / "hooks/consent").glob("*.json"))

    result = runner.run("add", clash.fq, format="json", check=False)

    assert result.returncode == 78, result.stderr
    assert "clash" in result.stdout
    assert config.read_bytes() == config_before
    assert lock.read_bytes() == lock_before
    assert target.read_bytes() == held
    after = sorted(path.read_bytes() for path in (Path(runner.grim_home) / "hooks/consent").glob("*.json"))
    assert after == consent

    # Positive control: declined, nothing is armed, so the same add declares it.
    runner.run("add", clash.fq, "--no-trust-workspace")
    assert "clash" in config.read_text(encoding="utf-8")
    assert target.read_bytes() == held


def test_update_refuses_a_conflict_before_it_saves_or_prunes(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    skill = _skill(unique_repo)
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"language": "en"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other}, skills={"good": skill.fq})
    runner.run("install", TRUST)
    target, installed = project_dir / LOCAL, project_dir / ".claude/skills/good/SKILL.md"
    assert installed.is_file()
    settings = {"team": team.fq, "other": other.fq}
    write_config(project_dir, settings=settings, experimental_settings=True)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"
    held = lock.read_bytes()
    _package(unique_repo, "other", {"claude": {"model": "opus"}}, tag="stable")

    result = runner.run("update", TRUST, format="json", check=False)

    assert result.returncode == 78, result.stderr
    assert installed.is_file()
    assert lock.read_bytes() == held
    assert _file(target) == {"model": "sonnet", "language": "en"}

    # Positive control: with the clash gone the same run moves the pin and prunes.
    _package(unique_repo, "other", {"claude": {"language": "de"}}, tag="stable")
    runner.run("update", TRUST)
    assert _file(target) == {"model": "sonnet", "language": "de"}
    assert lock.read_bytes() != held
    assert not installed.exists()


def test_update_refusal_marks_the_row_and_exits_65(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "opus"})
    _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="stable")

    result = runner.run("update", TRUST, format="json", check=False)

    assert result.returncode == 65, result.stderr
    report = json.loads(result.stdout)
    assert _row(report, "team")["refused"] is True
    assert [piece["pointer"] for piece in report["pieces"]] == ["/model"]
    assert _file(target) == {"model": "opus"}

    report = runner.json("update", TRUST, "--force")

    assert _file(target) == {"model": "haiku"}
    assert _listed(report["settings_changes"]) == [("team", "/model", None, "overwritten")]


def test_update_names_the_value_a_new_version_stops_tracking(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    tables = {"claude": {"model": "sonnet", "language": "en"}}
    team = _package(unique_repo, "team", tables, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)
    _package(unique_repo, "team", {"claude": {"language": "en"}}, tag="stable")

    report = runner.json("update", TRUST)

    assert _file(target) == {"model": "sonnet", "language": "en"}
    assert _abandoned(_row(report, "team")["abandoned_entries"]) == [(target.resolve(), "/model", None)]
    assert list(_records(runner)) == [("team", "claude", "/language", None)]


def test_update_names_the_value_an_orphan_leaves(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)
    write_config(project_dir, settings={"other": other.fq}, experimental_settings=True)
    runner.run("lock")

    report = runner.json("update")

    assert _file(target) == {"model": "sonnet", "language": "en"}
    row = _row(report, "team")
    assert row["action"] == "removed"
    assert _abandoned(row["abandoned_entries"]) == [(target.resolve(), "/model", None)]
    assert report["settings_changes"] == []
    assert _abandoned(_row(report, "other")["abandoned_entries"]) == []


@pytest.mark.skipif(
    not hasattr(os, "geteuid") or os.geteuid() == 0, reason="needs a directory this process cannot write to"
)
def test_retry_after_partial_failure_converges(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    claude, codex = roots["claude"] / "settings.json", roots["codex"] / "config.toml"

    # Files apply in path order, so Claude's lands before Codex's write fails.
    roots["codex"].chmod(0o555)
    try:
        failed = grim.run("--global", "install", format="json", check=False)
    finally:
        roots["codex"].chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(claude) == {"model": "sonnet"}
    assert not codex.exists()
    assert list(_records(grim)) == [("team", "claude", "/model", None)]
    written = claude.stat().st_mtime_ns

    result = grim.run("--global", "install", format="json")

    assert _changes(result) == [("team", "/model", None, "added")]
    assert 'model = "gpt"' in codex.read_text(encoding="utf-8")
    assert claude.stat().st_mtime_ns == written
    assert len(_records(grim)) == 2


def test_crash_between_write_and_record_reads_as_adoption(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    second = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": first})
    runner.run("install", TRUST)
    [store] = _store_files(runner)
    before = store.read_bytes()
    _repin(project_dir, first, second)
    assert _changes(runner.run("install", TRUST, format="json")) == [("team", "/model", None, "changed")]

    # The process died here: the file holds the new value, the record the old one.
    store.write_bytes(before)
    result = runner.run("install", TRUST, format="json")

    assert result.returncode == 0, result.stderr
    assert _changes(result) == []
    assert _file(project_dir / LOCAL) == {"model": "opus"}
    assert _records(runner) == {("team", "claude", "/model", None): True}


def test_harness_dropped_value_converges_without_force(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    tables = {"claude": {"model": "sonnet", "permissions": {"deny": [DENY]}}}
    team = _package(unique_repo, "team", tables)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    written = _file(target)
    _put(target, {"permissions": {"deny": []}})

    result = runner.run("install", TRUST, format="json")

    assert _file(target) == written
    assert sorted(_changes(result), key=lambda row: row[1]) == [
        ("team", "/model", None, "added"),
        ("team", "/permissions/deny", DENY, "added"),
    ]

    # Positive control: a value changed rather than dropped still needs `--force`.
    _put(target, {**written, "model": "opus"})
    assert runner.run("install", TRUST, check=False).returncode == 65
