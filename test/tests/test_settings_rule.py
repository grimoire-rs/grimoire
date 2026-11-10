# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What stops a settings run, and what it leaves behind.

Fixtures come from ``make_settings``, never from the grim under test. Each
workspace is locked with the flag off and turned on after, the road a package
takes when it arms later: ``grim install`` is the slot that catches it, whatever
``add`` and ``lock`` refuse themselves (``adr_settings_artifact_kind.md``,
Decision 5). Each "nothing was written" sits beside a run that does write.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import (
    SETTINGS_LAYER_MEDIA_TYPE,
    make_artifact,
    make_settings,
    settings_layer,
    settings_sandbox,
    write_config,
)
from src.registry import PublishedArtifact, push_manifest
from src.runner import GrimRunner

TRUST = "--trust-workspace"
FEATURES_KEY = "com.grimoire.requires-features"
SKILL_DOC = "---\nname: good\ndescription: d\n---\n# Good\n"
BASE_DOC = "---\nname: base\ndescription: b\n---\n# Base\n"
REQUIRES_BLOCK = re.compile(r'\n\[\[settings\.requires\]\]\nkind = "[^"]*"\nname = "[^"]*"\nid = "[^"]*"\n')
LOCAL = ".claude/settings.local.json"
DENY = "Bash(rm:*)"


def _skill(repo: str) -> PublishedArtifact:
    return make_artifact(f"{repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


def _armed_later(
    grim_at, workspace: Path, sandbox: Path, skill: PublishedArtifact, packages: dict[str, PublishedArtifact]
) -> GrimRunner:
    """A Claude workspace locked with the flag off, whose flag then went on."""
    (workspace / ".claude").mkdir(parents=True, exist_ok=True)
    write_config(workspace, skills={"good": skill.fq}, settings={name: p.fq for name, p in packages.items()})
    runner = grim_at(workspace)
    runner.env.update(settings_sandbox(sandbox, runner.home)[0])
    runner.run("lock")
    runner.run("config", "set", "options.experimental.settings", "true")
    return runner


def _conflict_rows(result: CompletedProcess[str]) -> list[dict]:
    """The conflict rows of a run that must have refused as a conflict does."""
    assert result.returncode == 78, result.stderr
    error = json.loads(result.stdout)["error"]
    assert error["exit"] == 78, error
    assert "conflict" in error["message"], error
    assert "reason" not in error and "forceable" not in error, error
    for row in error["pieces"]:
        assert (row["file"], row["state"], row["marked"]) == (None, None, "conflict"), row
    return error["pieces"]


def _pointers(rows: list[dict]) -> list[tuple[str, str, str]]:
    return [(row["package"], row["client"], row["pointer"]) for row in rows]


def _refusal(result: CompletedProcess[str], reason: str) -> dict:
    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert (error["exit"], error["reason"]) == (65, reason), error
    return error


def _team(grim_at, workspace: Path, tmp_path: Path, repo: str, tables: dict) -> tuple[GrimRunner, PublishedArtifact]:
    """One armed package, `team`, declaring `tables`, with nothing installed."""
    team = _package(repo, "team", tables)
    return _armed_later(grim_at, workspace, tmp_path / "sandbox", _skill(repo), {"team": team}), team


def _install(runner: GrimRunner, *flags: str, check: bool = True) -> CompletedProcess[str]:
    return runner.run("install", TRUST, *flags, format="json", check=check)


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    """The change list, by key: a test names what moved, not the phase's walk."""
    rows = json.loads(result.stdout)["settings_changes"]
    listed = [(row["package"], row["key"], row["element"], row["action"]) for row in rows]
    return sorted(listed, key=lambda row: (row[1], row[2] or ""))


def _file(workspace: Path) -> dict:
    return json.loads((workspace / LOCAL).read_text(encoding="utf-8"))


def _put(workspace: Path, values: dict) -> None:
    (workspace / LOCAL).write_text(json.dumps(values), encoding="utf-8", newline="\n")


def _stored(runner: GrimRunner) -> list[Path]:
    return sorted((Path(runner.grim_home) / "settings").glob("*.json"))


def _records(runner: GrimRunner) -> dict[tuple[str, str, str | None], bool]:
    """Each recorded piece, `(package, pointer, element)`, and its `adopted`."""
    return {
        (record["name"], output["entry"], output.get("element")): output.get("adopted", False)
        for path in _stored(runner)
        for record in json.loads(path.read_text(encoding="utf-8"))["records"]
        for output in record["outputs"]
    }


def _repin(workspace: Path, old: PublishedArtifact, new: PublishedArtifact) -> None:
    """Move one pin, as a pulled lock does: `lock` itself never moves one."""
    lock = workspace / "grimoire.lock"
    text = lock.read_text(encoding="utf-8")
    assert old.digest in text
    lock.write_text(text.replace(old.digest, new.digest), encoding="utf-8", newline="\n")


def test_conflict_exits_78_on_install(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    beta = _package(unique_repo, "beta", {"claude": {"model": "opus"}})
    runner = _armed_later(
        grim_at,
        project_dir,
        tmp_path / "sandbox",
        _skill(unique_repo),
        {"alpha": _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}}), "beta": beta},
    )
    skill = project_dir / ".claude/skills/good/SKILL.md"

    rows = _conflict_rows(runner.run("install", TRUST, format="json", check=False))

    assert _pointers(rows) == [("alpha", "claude", "/model"), ("beta", "claude", "/model")]
    assert [row["wanted"] for row in rows] == ["sonnet", "opus"]
    assert _conflict_rows(runner.run("install", TRUST, "--force", format="json", check=False)) == rows
    assert not skill.exists()
    assert not (project_dir / LOCAL).exists()

    # Positive control: unarmed, the same lock compares nothing and installs.
    runner.run("install", "--no-trust-workspace")
    assert skill.is_file()
    assert not (project_dir / LOCAL).exists()

    # Positive control: packages that agree write what they declare.
    _repin(project_dir, beta, _package(unique_repo, "beta", {"claude": {"model": "sonnet"}}, tag="2"))
    _install(runner)
    assert _file(project_dir) == {"model": "sonnet"}


def test_prefix_conflict(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    skill = _skill(unique_repo)
    whole = _package(unique_repo, "whole", {"claude": {"model": "sonnet"}, "opencode": {"permission": "ask"}})
    tool = _package(unique_repo, "tool", {"opencode": {"permission": {"bash": "ask"}}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "sandbox", skill, {"whole": whole, "tool": tool})

    rows = _conflict_rows(runner.run("install", TRUST, format="json", check=False))

    # This workspace has no OpenCode: documents are compared, not clients.
    assert not (project_dir / ".opencode").exists()
    assert _pointers(rows) == [("whole", "opencode", "/permission"), ("tool", "opencode", "/permission/bash")]
    assert not (project_dir / ".claude/skills/good").exists()
    assert not (project_dir / LOCAL).exists()

    # Positive control: without the prefix the Claude key lands.
    _repin(project_dir, tool, _package(unique_repo, "tool", {"opencode": {"permission": "ask"}}, tag="2"))
    _install(runner)
    assert _file(project_dir) == {"model": "sonnet"}

    # Positive control: two keys that share text, not a pointer segment.
    other = tmp_path / "other"
    control = _armed_later(
        grim_at,
        other,
        tmp_path / "other-sandbox",
        skill,
        {
            "short": _package(unique_repo, "short", {"codex": {"model": "gpt"}}),
            "long": _package(unique_repo, "long", {"codex": {"model_reasoning_effort": "high"}}),
        },
    )
    control.run("install", TRUST)
    assert (other / ".claude/skills/good/SKILL.md").is_file()


def test_codex_exclusion_across_packages(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    skill = _skill(unique_repo)
    named = _package(unique_repo, "named", {"codex": {"default_permissions": "team"}})
    mode = _package(unique_repo, "mode", {"claude": {"language": "en"}, "codex": {"sandbox_mode": "read-only"}})
    write = _package(unique_repo, "write", {"codex": {"sandbox_workspace_write": {"network_access": True}}})

    runner = _armed_later(grim_at, project_dir, tmp_path / "s1", skill, {"named": named, "mode": mode})
    rows = _conflict_rows(runner.run("install", TRUST, format="json", check=False))
    assert _pointers(rows) == [("named", "codex", "/default_permissions"), ("mode", "codex", "/sandbox_mode")]
    assert not (project_dir / ".claude/skills/good").exists()
    assert not (project_dir / LOCAL).exists()

    second = tmp_path / "second"
    runner = _armed_later(grim_at, second, tmp_path / "s2", skill, {"named": named, "write": write})
    rows = _conflict_rows(runner.run("install", TRUST, format="json", check=False))
    assert _pointers(rows) == [
        ("named", "codex", "/default_permissions"),
        ("write", "codex", "/sandbox_workspace_write/network_access"),
    ]

    # Positive control: the two sandbox keys exclude only `default_permissions`.
    third = tmp_path / "third"
    runner = _armed_later(grim_at, third, tmp_path / "s3", skill, {"mode": mode, "write": write})
    runner.run("install", TRUST)
    assert (third / ".claude/skills/good/SKILL.md").is_file()
    assert _file(third) == {"language": "en"}


def test_required_feature_and_stale_edge_still_refuse(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    skill = _skill(unique_repo)
    document = {"description": "Team defaults", "claude": {"model": "sonnet"}}
    stand_in = make_settings(f"{unique_repo}/team", "1", document)
    runner = _armed_later(grim_at, project_dir, tmp_path / "s1", skill, {"team": stand_in})
    # `lock` refuses this manifest, so only a lock pinned earlier reaches it.
    future = push_manifest(
        f"{unique_repo}/team",
        "2",
        [(SETTINGS_LAYER_MEDIA_TYPE, settings_layer(document))],
        {"com.grimoire.kind": "settings", FEATURES_KEY: "future-thing"},
    )
    lock = project_dir / "grimoire.lock"
    lock.write_text(lock.read_text(encoding="utf-8").replace(stand_in.digest, future), encoding="utf-8", newline="\n")

    # Gated, the layer is never fetched, so nothing reads the feature.
    runner.run("install", "--no-trust-workspace")

    error = _refusal(runner.run("install", TRUST, format="json", check=False), "unsupported-artifact-feature")
    assert "future-thing" in error["message"], error
    assert not (project_dir / LOCAL).exists()
    # Positive control: the failure is the package's, and the skill still landed.
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()
    # Positive control: the pin this grim can read writes the same document.
    lock.write_text(lock.read_text(encoding="utf-8").replace(future, stand_in.digest), encoding="utf-8", newline="\n")
    _install(runner)
    assert _file(project_dir) == {"model": "sonnet"}

    # The edge check needs no layer: gated or armed, a stale lock refuses first.
    stale = tmp_path / "stale"
    dependent = make_settings(f"{unique_repo}/dep", "1", document, requires=[("skill", "base", "./base:1")])
    make_artifact(f"{unique_repo}/base", "skill", {"base/SKILL.md": BASE_DOC}, tag="1")
    runner = _armed_later(grim_at, stale, tmp_path / "s2", skill, {"dep": dependent})
    lock = stale / "grimoire.lock"
    intact = lock.read_text(encoding="utf-8")
    edited = REQUIRES_BLOCK.sub("", intact)
    assert edited != intact
    lock.write_text(edited, encoding="utf-8", newline="\n")

    for answer in (TRUST, "--no-trust-workspace"):
        error = _refusal(runner.run("install", answer, format="json", check=False), "stale-lock")
        assert "settings 'dep'" in error["message"], error
    assert not (stale / ".claude/skills/good").exists()
    assert not (stale / LOCAL).exists()

    # Positive control: the lock as written installs.
    lock.write_text(intact, encoding="utf-8", newline="\n")
    runner.run("install", TRUST)
    assert (stale / ".claude/skills/good/SKILL.md").is_file()
    assert _file(stale) == {"model": "sonnet"}


# ── One test per row of the rule, Decision 5 ─────────────────────────────


def test_rule_row_1(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """The lock declares a value the file lacks: written, and grim's."""
    marketplace = {"source": {"source": "github", "repo": "acme/tools"}}
    tables = {"claude": {"model": "sonnet", "extraKnownMarketplaces": {"acme": marketplace}}}
    runner, _ = _team(grim_at, project_dir, tmp_path, unique_repo, tables)
    entry = "/extraKnownMarketplaces/acme"

    result = _install(runner)

    assert _changes(result) == [("team", entry, None, "added"), ("team", "/model", None, "added")]
    assert json.loads(result.stdout)["settings_changes"][0]["file"] == LOCAL
    assert _file(project_dir) == {"model": "sonnet", "extraKnownMarketplaces": {"acme": marketplace}}
    recorded = {("team", "/model", None): False, ("team", entry, None): False}
    assert _records(runner) == recorded

    # The record reads back: a lost one would re-adopt, and `adopted` would flip.
    assert _changes(_install(runner)) == []
    assert _records(runner) == recorded


def test_rule_row_2(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """The file holds the lock's value: never a write, never listed."""
    model, language = ("team", "/model", None), ("team", "/language", None)
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"team": first})
    _put(project_dir, {"model": "sonnet"})

    # 2a: no record, so the value is the user's; `language` shows this run writes.
    assert _changes(_install(runner)) == [("team", "/language", None, "added")]
    assert _records(runner) == {model: True, language: False}

    # 2b: as recorded, and `adopted` is kept.
    held = (project_dir / LOCAL).read_bytes()
    assert _changes(_install(runner)) == []
    assert _records(runner) == {model: True, language: False}
    assert (project_dir / LOCAL).read_bytes() == held

    # 2c: the user made the file match where the lock then moved.
    _put(project_dir, {"model": "sonnet", "language": "de"})
    second = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "de"}}, tag="2")
    _repin(project_dir, first, second)
    held = (project_dir / LOCAL).read_bytes()
    assert _changes(_install(runner)) == []
    assert _records(runner) == {model: True, language: True}
    assert (project_dir / LOCAL).read_bytes() == held

    # The record moved to hash(L): a stale one would refuse here as modified.
    _repin(project_dir, second, first)
    assert _changes(_install(runner)) == [("team", "/language", None, "changed")]
    assert _file(project_dir) == {"model": "sonnet", "language": "en"}


def test_rule_row_3(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """The file is as recorded, so the lock moves it or lets it go."""
    first = _package(
        unique_repo,
        "team",
        {"claude": {"model": "sonnet", "language": "en", "effortLevel": "high", "outputStyle": "terse"}},
    )
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"team": first})
    _put(project_dir, {"effortLevel": "high"})
    _install(runner)
    assert _records(runner)[("team", "/effortLevel", None)] is True
    written = _file(project_dir)
    del written["outputStyle"]
    _put(project_dir, written)

    second = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    _repin(project_dir, first, second)
    result = _install(runner)

    assert _changes(result) == [
        ("team", "/language", None, "removed"),  # 3b
        ("team", "/model", None, "changed"),  # 3a
    ]
    # 3c leaves the adopted value and names it; 3d has nothing left to name.
    assert _file(project_dir) == {"model": "opus", "effortLevel": "high"}
    assert "/effortLevel" in result.stderr
    assert "/outputStyle" not in result.stderr
    assert _records(runner) == {("team", "/model", None): False}


def test_rule_row_4(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """An overwrite refuses until `--force`, which only then writes."""
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"team": first})

    # 4a, no record: `language` is clean and still waits for the whole phase.
    _put(project_dir, {"model": "opus"})
    held = (project_dir / LOCAL).read_bytes()
    error = _refusal(_install(runner, check=False), "untracked-destination")
    assert error["forceable"] is True
    assert error["pieces"] == [
        {
            "package": "team",
            "client": "claude",
            "file": LOCAL,
            "pointer": "/model",
            "element": None,
            "marked": "overwrite",
            "state": "untracked-destination",
            "wanted": "sonnet",
            "current": "opus",
        }
    ]
    assert (project_dir / LOCAL).read_bytes() == held
    assert _records(runner) == {}

    forced = _install(runner, "--force")
    assert _changes(forced) == [("team", "/language", None, "added"), ("team", "/model", None, "overwritten")]
    assert _file(project_dir) == {"model": "sonnet", "language": "en"}
    assert _records(runner) == {("team", "/model", None): False, ("team", "/language", None): False}

    # 4a and 4b, with a record: `model` still declared, `language` dropped.
    _put(project_dir, {"model": "haiku", "language": "de"})
    second = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="2")
    _repin(project_dir, first, second)
    held = (project_dir / LOCAL).read_bytes()
    error = _refusal(_install(runner, check=False), "modified")
    assert [(p["pointer"], p["state"], p["wanted"], p["current"]) for p in error["pieces"]] == [
        ("/language", "modified", None, "de"),
        ("/model", "modified", "sonnet", "haiku"),
    ]
    assert (project_dir / LOCAL).read_bytes() == held

    forced = _install(runner, "--force")
    assert _changes(forced) == [("team", "/language", None, "removed"), ("team", "/model", None, "overwritten")]
    assert _file(project_dir) == {"model": "sonnet"}
    assert _records(runner) == {("team", "/model", None): False}


def test_rule_row_5(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A value of another type stands in the way, and the refusal names it."""
    first = _package(unique_repo, "team", {"claude": {"permissions": {"deny": [DENY]}}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"team": first})
    _put(project_dir, {"permissions": "locked"})

    error = _refusal(_install(runner, check=False), "untracked-destination")

    [piece] = error["pieces"]
    assert (piece["pointer"], piece["element"], piece["current"]) == ("/permissions", DENY, "locked")
    assert _file(project_dir) == {"permissions": "locked"}

    forced = _install(runner, "--force")
    assert _changes(forced) == [("team", "/permissions/deny", DENY, "overwritten")]
    assert _file(project_dir) == {"permissions": {"deny": [DENY]}}
    assert _records(runner) == {("team", "/permissions/deny", DENY): False}

    # Dropped and forced: the blocker is no piece, so it stays and nothing is listed.
    _put(project_dir, {"permissions": "locked"})
    second = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="2")
    _repin(project_dir, first, second)
    error = _refusal(_install(runner, check=False), "modified")
    assert [piece["pointer"] for piece in error["pieces"]] == ["/permissions"]

    forced = _install(runner, "--force")
    assert _changes(forced) == [("team", "/model", None, "added")]
    assert _file(project_dir) == {"permissions": "locked", "model": "sonnet"}
    assert _records(runner) == {("team", "/model", None): False}


def test_rule_row_6(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Two declarers reach one decision, listed once, recorded on both."""
    shared = {"claude": {"language": "en", "permissions": {"deny": [DENY]}}}
    skill = _skill(unique_repo)
    alpha = _package(unique_repo, "alpha", shared)
    beta = _package(unique_repo, "beta", shared)
    gamma = _package(unique_repo, "gamma", shared)
    packages = {"alpha": alpha, "beta": beta, "gamma": gamma}
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", skill, packages)
    pieces = {"language": "en", "permissions": {"deny": [DENY]}}

    result = _install(runner)

    assert [row[1:] for row in _changes(result)] == [
        ("/language", None, "added"),
        ("/permissions/deny", DENY, "added"),
    ]
    assert _file(project_dir) == pieces
    assert _records(runner) == {
        (name, pointer, element): False
        for name in packages
        for pointer, element in (("/language", None), ("/permissions/deny", DENY))
    }

    # `beta` rewrites its record without them: the lock still declares both.
    slim = _package(unique_repo, "beta", {"claude": {"model": "sonnet"}}, tag="2")
    _repin(project_dir, beta, slim)
    result = _install(runner)

    assert _changes(result) == [("beta", "/model", None, "added")]
    assert _file(project_dir) == {**pieces, "model": "sonnet"}

    # `gamma` leaves the lock, and no layer is read: a record that survives decides.
    write_config(
        project_dir,
        skills={"good": skill.fq},
        settings={"alpha": alpha.fq, "beta": slim.fq},
        experimental_settings=True,
    )
    runner.run("lock")
    result = _install(runner)

    assert _changes(result) == []
    assert _file(project_dir) == {**pieces, "model": "sonnet"}
    assert _records(runner) == {
        ("alpha", "/language", None): False,
        ("alpha", "/permissions/deny", DENY): False,
        ("beta", "/model", None): False,
    }


def test_rule_row_7(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A file that does not parse is never replaced, whatever `--force` says."""
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"team": first})
    target = project_dir / LOCAL
    target.write_text("{not json", encoding="utf-8", newline="\n")

    # 7a: the whole phase refuses, with no reason and no way to force it.
    for flags in ((), ("--force",)):
        result = _install(runner, *flags, check=False)
        assert result.returncode == 65, result.stderr
        error = json.loads(result.stdout)["error"]
        assert "reason" not in error and "forceable" not in error, error
        assert [piece["pointer"] for piece in error["pieces"]] == ["/language", "/model"]
        assert target.read_text(encoding="utf-8") == "{not json"
        assert _records(runner) == {}
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()

    # Positive control: the same run writes a file that parses.
    target.unlink()
    _install(runner)
    assert _file(project_dir) == {"model": "sonnet", "language": "en"}

    # 7b: only removals reach it, so the records go and the bytes stay.
    target.write_text("{not json", encoding="utf-8", newline="\n")
    second = _package(unique_repo, "team", {"codex": {"model": "gpt"}}, tag="2")
    _repin(project_dir, first, second)

    result = _install(runner)

    assert _changes(result) == []
    assert target.read_text(encoding="utf-8") == "{not json"
    assert _records(runner) == {}
    assert "/model" in result.stderr and "/language" in result.stderr


def test_rule_row_8(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Neither the lock nor a record names it, so grim never compares it."""
    first = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"team": first})
    mine = {"theme": "dark", "language": "de"}
    _put(project_dir, mine)

    assert _changes(_install(runner)) == [("team", "/model", None, "added")]
    assert _file(project_dir) == {**mine, "model": "sonnet"}

    _repin(project_dir, first, _package(unique_repo, "team", {"claude": {"effortLevel": "high"}}, tag="2"))
    result = _install(runner)

    assert _changes(result) == [("team", "/effortLevel", None, "added"), ("team", "/model", None, "removed")]
    assert _file(project_dir) == {**mine, "effortLevel": "high"}
    assert _records(runner) == {("team", "/effortLevel", None): False}


def test_deleted_record_adopts_identical_and_refuses_different(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    runner, _ = _team(grim_at, project_dir, tmp_path, unique_repo, {"claude": {"model": "sonnet"}})
    _install(runner)
    assert _records(runner) == {("team", "/model", None): False}

    # A record never authorised the value, so losing it refuses nothing here.
    [record] = _stored(runner)
    record.unlink()
    assert _changes(_install(runner)) == []
    assert _records(runner) == {("team", "/model", None): True}

    [record] = _stored(runner)
    record.unlink()
    _put(project_dir, {"model": "opus"})
    error = _refusal(_install(runner, check=False), "untracked-destination")
    assert [piece["state"] for piece in error["pieces"]] == ["untracked-destination"]
    assert _file(project_dir) == {"model": "opus"}


@pytest.mark.parametrize("route", ["update", "pulled-lock"])
def test_two_declarers_dropped_together_remove_the_piece(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, route: str
) -> None:
    """One rewrites its record without the piece, the other leaves the config."""
    shared = {"claude": {"language": "en", "permissions": {"deny": [DENY]}}}
    skill = _skill(unique_repo)
    alpha = _package(unique_repo, "alpha", shared)
    beta = _package(unique_repo, "beta", shared)
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", skill, {"alpha": alpha, "beta": beta})
    _install(runner)
    assert _file(project_dir) == {"language": "en", "permissions": {"deny": [DENY]}}

    write_config(project_dir, skills={"good": skill.fq}, settings={"alpha": alpha.fq}, experimental_settings=True)
    if route == "update":
        _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
        result = runner.run("update", TRUST, format="json")
    else:
        runner.run("lock")
        _repin(project_dir, alpha, _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}}, tag="2"))
        result = _install(runner)

    assert [row[1:] for row in _changes(result)] == [
        ("/language", None, "removed"),
        ("/model", None, "added"),
        ("/permissions/deny", DENY, "removed"),
    ]
    assert _file(project_dir) == {"model": "sonnet"}
    assert _records(runner) == {("alpha", "/model", None): False}


# ── Refusal and force ────────────────────────────────────────────────────


def test_refusal_lists_every_piece_across_packages(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    skill = _skill(unique_repo)
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet", "language": "en"}})
    beta = _package(
        unique_repo,
        "beta",
        {"claude": {"effortLevel": "high", "outputStyle": "terse", "spinnerTipsEnabled": True}},
    )
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", skill, {"alpha": alpha, "beta": beta})
    theirs = {"model": "opus", "language": "de", "effortLevel": "low", "outputStyle": "plain"}
    _put(project_dir, theirs)
    held = (project_dir / LOCAL).read_bytes()

    error = _refusal(_install(runner, check=False), "untracked-destination")

    assert error["forceable"] is True
    assert [(p["package"], p["pointer"]) for p in error["pieces"]] == [
        ("beta", "/effortLevel"),
        ("alpha", "/language"),
        ("alpha", "/model"),
        ("beta", "/outputStyle"),
    ]
    assert {piece["marked"] for piece in error["pieces"]} == {"overwrite"}
    # `/spinnerTipsEnabled` was clean: a refusal judges first and writes nothing.
    assert (project_dir / LOCAL).read_bytes() == held
    assert _records(runner) == {}
    # Positive control: the failure is the phase's, and the skill landed.
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()

    text = runner.run("install", TRUST, check=False)
    assert text.returncode == 65
    for package, pointer in (("alpha", "/model"), ("alpha", "/language"), ("beta", "/effortLevel")):
        line = f"overwrite claude {LOCAL} {pointer} ({package}): untracked-destination"
        assert line in text.stderr

    # Positive control: forced, every piece of every package lands.
    forced = _install(runner, "--force")
    assert len(_changes(forced)) == 5
    assert _file(project_dir) == {
        "model": "sonnet",
        "language": "en",
        "effortLevel": "high",
        "outputStyle": "terse",
        "spinnerTipsEnabled": True,
    }

    # A piece with a record makes the run's reason `modified`; each keeps its state.
    gamma = _package(unique_repo, "gamma", {"claude": {"alwaysThinkingEnabled": True}})
    write_config(
        project_dir,
        skills={"good": skill.fq},
        settings={"alpha": alpha.fq, "beta": beta.fq, "gamma": gamma.fq},
        experimental_settings=True,
    )
    runner.run("lock")
    _put(project_dir, {**_file(project_dir), "model": "opus", "alwaysThinkingEnabled": False})
    error = _refusal(_install(runner, check=False), "modified")
    assert {(piece["pointer"], piece["state"]) for piece in error["pieces"]} == {
        ("/model", "modified"),
        ("/alwaysThinkingEnabled", "untracked-destination"),
    }


def _add(runner: GrimRunner, package: PublishedArtifact, *flags: str) -> CompletedProcess[str]:
    return runner.run("add", "--kind", "settings", TRUST, *flags, package.fq, format="json", check=False)


def test_add_force_scope(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """`add --force` reaches the pieces of what it added, and no others."""
    skill = _skill(unique_repo)
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s1", skill, {"alpha": alpha})
    _install(runner)

    # In the set: the added package's own piece overwrites.
    _put(project_dir, {**_file(project_dir), "effortLevel": "low"})
    beta = _package(unique_repo, "beta", {"claude": {"effortLevel": "high"}})
    plain = _add(runner, beta)
    assert plain.returncode == 65, plain.stderr
    assert _file(project_dir)["effortLevel"] == "low"
    forced = _add(runner, beta, "--force")
    assert forced.returncode == 0, forced.stderr
    assert _file(project_dir)["effortLevel"] == "high"

    # Shared: a piece is forced if any declaring package is in the set.
    _put(project_dir, {**_file(project_dir), "language": "de"})
    gamma = _package(unique_repo, "gamma", {"claude": {"language": "en"}})
    forced = _add(runner, gamma, "--force")
    assert forced.returncode == 0, forced.stderr
    assert _file(project_dir)["language"] == "en"

    # Outside it: `alpha`'s drift refuses the whole phase, naming the piece.
    _put(project_dir, {**_file(project_dir), "model": "opus"})
    delta = _package(unique_repo, "delta", {"claude": {"outputStyle": "terse"}})
    result = _add(runner, delta, "--force")
    error = _refusal(result, "modified")
    assert ("alpha", "/model") in [(piece["package"], piece["pointer"]) for piece in error["pieces"]]
    assert "/model" in error["message"]
    assert "outputStyle" not in _file(project_dir)
    # Positive control: `install --force` reaches it, and the added package lands.
    _install(runner, "--force")
    assert _file(project_dir)["model"] == "sonnet"
    assert _file(project_dir)["outputStyle"] == "terse"


def test_add_force_cannot_reach_an_orphan(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    skill = _skill(unique_repo)
    alpha = _package(unique_repo, "alpha", {"claude": {"language": "en"}})
    runner = _armed_later(grim_at, project_dir, tmp_path / "s", skill, {"alpha": alpha})
    _install(runner)

    # The record outlives its lock entry, and the user edited the value.
    write_config(project_dir, skills={"good": skill.fq}, experimental_settings=True)
    runner.run("lock")
    _put(project_dir, {"language": "de"})
    beta = _package(unique_repo, "beta", {"claude": {"model": "sonnet"}})

    result = _add(runner, beta, "--force")

    error = _refusal(result, "modified")
    assert [(piece["package"], piece["pointer"]) for piece in error["pieces"]] == [("alpha", "/language")]
    assert _file(project_dir) == {"language": "de"}
    # Positive control: the orphan's key comes from a run that forces all.
    _install(runner, "--force")
    assert _file(project_dir) == {"model": "sonnet"}


def test_unloadable_layer_fails_its_row_only(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    healthy = _package(unique_repo, "healthy", {"claude": {"model": "sonnet"}})
    broken = _package(unique_repo, "broken", {"claude": {"language": "en"}})
    runner = _armed_later(
        grim_at, project_dir, tmp_path / "s", _skill(unique_repo), {"healthy": healthy, "broken": broken}
    )
    unreadable = push_manifest(
        f"{unique_repo}/broken",
        "2",
        [(SETTINGS_LAYER_MEDIA_TYPE, b"{not json")],
        {"com.grimoire.kind": "settings"},
    )
    lock = project_dir / "grimoire.lock"
    intact = lock.read_text(encoding="utf-8")
    lock.write_text(intact.replace(broken.digest, unreadable), encoding="utf-8", newline="\n")

    result = _install(runner, check=False)

    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert "reason" not in error and "forceable" not in error, error
    assert "broken" in error["message"], error
    assert not (project_dir / LOCAL).exists()
    assert _records(runner) == {}
    # Positive control: the failure is the row's, so the skill landed.
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()

    # Positive control: with the layer readable both packages apply.
    lock.write_text(intact, encoding="utf-8", newline="\n")
    _install(runner)
    assert _file(project_dir) == {"model": "sonnet", "language": "en"}
