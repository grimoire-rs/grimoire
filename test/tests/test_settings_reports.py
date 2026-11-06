# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What a settings run tells the user: changed keys, refused pieces, the closing note.

``adr_settings_artifact_kind.md``, Decisions 5 and 8. Any run that changes an
applied value lists the key, on every path that can change one: install, add,
update, uninstall, a reap and a package dropped from the lock. A run that fails
part way lists what it did change, in the error document. An old value appears
in ``pieces[].current`` and nowhere else. Fixtures come from ``make_settings``.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import make_artifact, make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact, fetch_manifest
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
SKILL_DOC = "---\nname: good\ndescription: d\n---\n# Good\n"
DENY = "Bash(rm:*)"
RESTART = "a running client may need a restart"
CHANGED_MODEL = ("team", "/model", None, "changed")
REMOVED_MODEL = ("team", "/model", None, "removed")

needs_an_unwritable_directory = pytest.mark.skipif(
    not hasattr(os, "geteuid") or os.geteuid() == 0, reason="needs a directory this process cannot write to"
)


def _package(repo: str, name: str, tables: dict, tag: str = "1") -> PublishedArtifact:
    return make_settings(f"{repo}/{name}", tag, {"description": f"{name} defaults", **tables})


def _skill(repo: str) -> PublishedArtifact:
    return make_artifact(f"{repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")


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


def _declare(home: Path, team: PublishedArtifact, clients: list[str]) -> None:
    write_config(
        home, settings={"team": team.fq}, options={"clients": json.dumps(clients)}, experimental_settings=True
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


def _repin(workspace: Path, old: PublishedArtifact, new: PublishedArtifact) -> None:
    """Move the lock the way a pulled lock moves it."""
    lock = workspace / "grimoire.lock"
    text = lock.read_text(encoding="utf-8")
    assert old.digest in text
    lock.write_text(text.replace(old.digest, new.digest), encoding="utf-8")


def _file(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _put(path: Path, values: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(values), encoding="utf-8")


def _listed(rows: list[dict]) -> list[tuple[str, str, str | None, str]]:
    """A change list by key: a test names what moved, not the phase's walk."""
    listed = [(row["package"], row["key"], row["element"], row["action"]) for row in rows]
    return sorted(listed, key=lambda row: (row[0], row[1], row[2] or ""))


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    return _listed(json.loads(result.stdout)["settings_changes"])


def _error(result: CompletedProcess[str]) -> dict:
    return json.loads(result.stdout)["error"]


def _closes_the_list(result: CompletedProcess[str], *changes: str) -> None:
    """Every named change line is on stderr, then exactly one restart note."""
    lines = result.stderr.splitlines()
    notes = [index for index, line in enumerate(lines) if RESTART in line]
    assert len(notes) == 1, result.stderr
    for change in changes:
        assert any(change in line for line in lines[: notes[0]]), (change, result.stderr)


def _conflicts(result: CompletedProcess[str]) -> list[tuple[str, str, str, str]]:
    """The conflict rows of a run that refused with 78, which name the package and its value."""
    assert result.returncode == 78, result.stderr
    return [(row["package"], row["pointer"], row["marked"], row["wanted"]) for row in _error(result)["pieces"]]


def _layer_cached(runner: GrimRunner, package: PublishedArtifact) -> bool:
    """Whether this machine fetched ``package``'s layer. Its manifest is the edge check's."""
    [layer] = fetch_manifest(package.repo, package.tag)["layers"]
    digest = layer["digest"].removeprefix("sha256:")
    return any(digest in path.name for path in (Path(runner.grim_home) / "blobs").rglob("*"))


def _store_text(runner: GrimRunner) -> str:
    """Everything grim recorded about this machine's settings, global state included."""
    home = Path(runner.grim_home)
    files = [*(home / "settings").glob("*.json"), *(home / "state").glob("*.json")]
    return "".join(path.read_text(encoding="utf-8") for path in files)


def test_refusal_pieces_in_json_and_text(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Every value the user moved refuses, one stderr line each, and a skill in the run lands."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    write_config(
        project_dir, settings={"team": team.fq}, skills={"good": _skill(unique_repo).fq}, experimental_settings=True
    )
    runner.run("lock")
    target = project_dir / LOCAL
    moved = {"model": "opus", "language": "de"}
    _put(target, moved)

    result = runner.run("install", TRUST, format="json", check=False)

    assert result.returncode == 65, result.stderr
    error = _error(result)
    assert (error["code"], error["exit"], error["reason"], error["forceable"]) == ("data", 65, "modified", True)
    pieces = [
        (row["package"], row["client"], row["file"], row["pointer"], row["marked"], row["state"], row["current"])
        for row in sorted(error["pieces"], key=lambda row: row["pointer"])
    ]
    assert pieces == [
        ("team", "claude", LOCAL, "/language", "overwrite", "modified", "de"),
        ("team", "claude", LOCAL, "/model", "overwrite", "modified", "opus"),
    ]
    assert _file(target) == moved
    # Positive control: the run went on to install what it could.
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()

    plain = runner.run("install", TRUST, check=False)

    assert plain.returncode == 65, plain.stderr
    assert f"overwrite claude {LOCAL} /language (team): modified" in plain.stderr
    assert f"overwrite claude {LOCAL} /model (team): modified" in plain.stderr
    assert plain.stderr.count("overwrite claude") == 2


def test_conflict_pieces_in_json_and_text(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A conflict has no file and no state, so its stderr line names neither."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": {"model": "opus"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"alpha": alpha, "beta": beta})

    result = runner.run("install", TRUST, format="json", check=False)

    assert result.returncode == 78, result.stderr
    pieces = [(row["package"], row["pointer"], row["marked"], row["wanted"]) for row in _error(result)["pieces"]]
    assert pieces == [("alpha", "/model", "conflict", "sonnet"), ("beta", "/model", "conflict", "opus")]
    assert "conflict claude /model (alpha)\n" in result.stderr
    assert "conflict claude /model (beta)\n" in result.stderr
    assert "overwrite claude" not in result.stderr
    assert not (project_dir / LOCAL).exists()


def test_changes_survive_a_later_refusal(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A refused skill drops the report, so the error document carries the change list."""
    old = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    new = _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="2")
    skill = _skill(unique_repo)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": old}, skills={"good": skill.fq})
    runner.run("install", TRUST)
    installed = project_dir / ".claude/skills/good/SKILL.md"
    installed.write_text(SKILL_DOC + "a local edit\n", encoding="utf-8")
    _repin(project_dir, old, new)

    result = runner.run("install", TRUST, format="json", check=False)

    assert result.returncode == 65, result.stderr
    error = _error(result)
    assert error["reason"] == "modified"
    assert _listed(error["settings_changes"]) == [CHANGED_MODEL]
    assert "pieces" not in error
    assert _file(project_dir / LOCAL) == {"model": "haiku"}
    _closes_the_list(result, f"changed claude {LOCAL} /model (team)")


def test_install_after_a_pull_lists_keys(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A teammate's bump lands with no prompt, and the run names what moved."""
    old = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    new = _package(unique_repo, "team", {"claude": {"model": "haiku", "language": "en"}}, tag="2")
    write_config(project_dir, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("add", old.fq)
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet", "language": "en"}
    _repin(project_dir, old, new)

    result = runner.run("install", format="json")

    assert _file(target) == {"model": "haiku", "language": "en"}
    assert _changes(result) == [CHANGED_MODEL]
    _closes_the_list(result, f"changed claude {LOCAL} /model (team)")


def test_add_lists_changes(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Project scope writes Claude's local file and declines the global-only client."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    write_config(project_dir, options={"clients": json.dumps(["claude", "codex"])}, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])

    result = runner.run("add", team.fq, format="json")

    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    assert "codex applies at global scope only" in result.stderr
    assert _changes(result) == [("team", "/model", None, "added")]
    _closes_the_list(result, f"added claude {LOCAL} /model (team)")


def test_uninstall_lists_changes(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"model": "sonnet", "permissions": {"deny": [DENY]}}}
    team = _package(unique_repo, "team", tables)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    assert _file(target)["model"] == "sonnet"

    result = runner.run("uninstall", "settings", "team", format="json")

    assert "model" not in _file(target)
    assert _changes(result) == [REMOVED_MODEL, ("team", "/permissions/deny", DENY, "removed")]
    _closes_the_list(result, f"removed claude {LOCAL} /model (team)")


def test_force_lists_overwritten(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "opus", "language": "en"})

    result = runner.run("install", TRUST, "--force", format="json")

    assert _file(target) == {"model": "sonnet", "language": "en"}
    assert _changes(result) == [("team", "/model", None, "overwritten")]
    _closes_the_list(result, f"overwritten claude {LOCAL} /model (team)")


def test_dropped_package_lists_removed(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """With no consent asked: what grim wrote goes, and a value it adopted stays."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet", "language": "en"}})
    other = _package(unique_repo, "other", {"claude": {"spinnerTipsEnabled": False}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    target = project_dir / LOCAL
    _put(target, {"model": "sonnet"})
    runner.run("install", TRUST)
    assert _file(target) == {"model": "sonnet", "language": "en", "spinnerTipsEnabled": False}
    write_config(project_dir, settings={"other": other.fq}, experimental_settings=True)
    runner.run("lock")

    result = runner.run("install", format="json")

    assert _file(target) == {"model": "sonnet", "spinnerTipsEnabled": False}
    assert _changes(result) == [("team", "/language", None, "removed")]
    _closes_the_list(result, f"removed claude {LOCAL} /language (team)")


def test_old_value_only_in_the_error(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Refuse, force, uninstall: the old value leaves one trace, the refusal's `current`."""
    sentinel = "the-users-own-model-3f9a"
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL
    _put(target, {"model": sentinel})

    refused = runner.run("install", TRUST, format="json", check=False)

    assert refused.returncode == 65, refused.stderr
    [piece] = _error(refused)["pieces"]
    assert piece["current"] == sentinel
    assert sentinel not in _error(refused)["message"]
    assert sentinel not in refused.stderr
    assert sentinel not in runner.run("install", TRUST, check=False).stderr

    forced = runner.run("install", TRUST, "--force", format="json")

    assert _changes(forced) == [("team", "/model", None, "overwritten")]
    recorded = _store_text(runner)
    assert "/model" in recorded, "the record exists, so its silence means something"
    assert sentinel not in recorded

    removed = runner.run("uninstall", "settings", "team", format="json")

    assert _changes(removed) == [REMOVED_MODEL]
    assert _file(target) == {}
    for run in (forced, removed):
        assert sentinel not in run.stdout + run.stderr
    assert sentinel not in _store_text(runner)


def test_current_is_bounded_and_masked(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A long, hostile or credentialed value is safe to print, whole escapes only."""
    tables = {"claude": {"model": "sonnet", "language": "en", "spinnerTipsEnabled": True}}
    team = _package(unique_repo, "team", tables)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    _put(
        project_dir / LOCAL,
        {
            "model": {"endpoint": "https://robot:hunter2@proxy.example.com/v1"},
            "language": "x" * 250,
            "spinnerTipsEnabled": "\x1b" * 250,
        },
    )

    result = runner.run("install", TRUST, format="json", check=False)

    assert result.returncode == 65, result.stderr
    current = {piece["pointer"]: piece["current"] for piece in _error(result)["pieces"]}
    assert "proxy.example.com" in current["/model"]
    assert "hunter2" not in result.stdout + result.stderr
    assert "robot" not in result.stdout + result.stderr
    assert current["/language"] == "x" * 200
    # Each control character renders as six, and the bound holds after that.
    assert current["/spinnerTipsEnabled"] == "\\u{1b}" * 33


def test_restart_note_follows_the_change_list(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """One note closes a run, after every line it lists, however many passes made them."""
    tables = {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}}
    team = _package(unique_repo, "team", tables, tag="stable")
    _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])

    installed = grim.run("--global", "install", format="json")

    _closes_the_list(installed, "added claude", "added codex")

    unchanged = grim.run("--global", "install", format="json")

    assert _changes(unchanged) == []
    assert RESTART not in unchanged.stderr

    # The apply pass moves Codex's value, then the reap removes Claude's.
    _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "o4"}}, tag="stable")
    _declare(grim_home, team, ["codex"])

    updated = grim.run("--global", "update", format="json")

    assert _changes(updated) == [CHANGED_MODEL, REMOVED_MODEL]
    _closes_the_list(updated, "changed codex", "removed claude")


def test_update_refusal_exits_65_with_pieces(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    _put(target, {"model": "opus"})
    _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="stable")

    result = runner.run("update", TRUST, format="json", check=False)

    assert result.returncode == 65, result.stderr
    report = json.loads(result.stdout)
    assert "error" not in report
    assert report["settings_changes"] == []
    pieces = [(row["pointer"], row["marked"], row["state"], row["current"]) for row in report["pieces"]]
    assert pieces == [("/model", "overwrite", "modified", "opus")]
    assert f"overwrite claude {LOCAL} /model (team): modified" in result.stderr
    assert RESTART not in result.stderr
    assert _file(target) == {"model": "opus"}


def test_update_reap_refusal_keeps_what_apply_changed(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Exit 65 carries both keys when apply changed a value and the reap then refused."""
    tables = {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}}
    team = _package(unique_repo, "team", tables, tag="stable")
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    claude = roots["claude"] / "settings.json"
    _put(claude, {"model": "opus"})
    _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "o4"}}, tag="stable")
    _declare(grim_home, team, ["codex"])

    result = grim.run("--global", "update", format="json", check=False)

    assert result.returncode == 65, result.stderr
    report = json.loads(result.stdout)
    assert _listed(report["settings_changes"]) == [CHANGED_MODEL]
    assert [piece["pointer"] for piece in report["pieces"]] == ["/model"]
    assert 'model = "o4"' in (roots["codex"] / "config.toml").read_text(encoding="utf-8")
    assert _file(claude) == {"model": "opus"}
    _closes_the_list(result, "changed codex")


@needs_an_unwritable_directory
def test_update_hard_error_keeps_changes(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """The values moved before the state write failed, so the error names them."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="stable")

    state_dir = project_dir / ".grimoire"
    state_dir.chmod(0o555)
    try:
        failed = runner.run("update", TRUST, format="json", check=False)
    finally:
        state_dir.chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(project_dir / LOCAL) == {"model": "haiku"}
    assert _listed(_error(failed)["settings_changes"]) == [CHANGED_MODEL]
    _closes_the_list(failed, f"changed claude {LOCAL} /model (team)")


@needs_an_unwritable_directory
def test_update_stopped_reap_keeps_changes(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}}
    team = _package(unique_repo, "team", tables, tag="stable")
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    _package(unique_repo, "team", {"claude": {"model": "haiku"}, "codex": {"model": "gpt"}}, tag="stable")
    _declare(grim_home, team, ["claude"])

    roots["codex"].chmod(0o555)
    try:
        failed = grim.run("--global", "update", format="json", check=False)
    finally:
        roots["codex"].chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(roots["claude"] / "settings.json") == {"model": "haiku"}
    assert 'model = "gpt"' in (roots["codex"] / "config.toml").read_text(encoding="utf-8")
    assert _listed(_error(failed)["settings_changes"]) == [CHANGED_MODEL]
    _closes_the_list(failed, "changed claude")


def test_update_client_shrink_lists_removed(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"model": "sonnet", "permissions": {"deny": [DENY]}}, "codex": {"model": "gpt"}}
    team = _package(unique_repo, "team", tables)
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    claude = roots["claude"] / "settings.json"
    assert _file(claude)["model"] == "sonnet"
    _declare(grim_home, team, ["codex"])

    result = grim.run("--global", "update", format="json")

    assert "model" not in _file(claude)
    assert _changes(result) == [REMOVED_MODEL, ("team", "/permissions/deny", DENY, "removed")]
    _closes_the_list(result, "removed claude")


@needs_an_unwritable_directory
def test_install_state_failure_keeps_changes(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A failed state write comes after the values moved, and the error says which."""
    old = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    new = _package(unique_repo, "team", {"claude": {"model": "haiku"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": old})
    runner.run("install", TRUST)
    _repin(project_dir, old, new)

    state_dir = project_dir / ".grimoire"
    state_dir.chmod(0o555)
    try:
        failed = runner.run("install", TRUST, format="json", check=False)
    finally:
        state_dir.chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(project_dir / LOCAL) == {"model": "haiku"}
    assert _listed(_error(failed)["settings_changes"]) == [CHANGED_MODEL]
    _closes_the_list(failed, f"changed claude {LOCAL} /model (team)")


@needs_an_unwritable_directory
def test_add_state_failure_keeps_changes(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})

    state_dir = project_dir / ".grimoire"
    state_dir.chmod(0o555)
    try:
        failed = runner.run("add", other.fq, format="json", check=False)
    finally:
        state_dir.chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(project_dir / LOCAL) == {"model": "sonnet", "language": "en"}
    assert _listed(_error(failed)["settings_changes"]) == [("other", "/language", None, "added")]
    _closes_the_list(failed, f"added claude {LOCAL} /language (other)")


@needs_an_unwritable_directory
def test_uninstall_stop_keeps_changes(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """Files go in path order, so Claude's value left before Codex's write failed."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "codex"])
    grim.run("--global", "install")
    claude = roots["claude"] / "settings.json"
    assert _file(claude) == {"model": "sonnet"}

    roots["codex"].chmod(0o555)
    try:
        failed = grim.run("--global", "uninstall", "settings", "team", format="json", check=False)
    finally:
        roots["codex"].chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(claude) == {}
    assert _listed(_error(failed)["settings_changes"]) == [REMOVED_MODEL]
    _closes_the_list(failed, "removed claude")


def test_uninstall_refused_dependency_keeps_changes(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The target's values left before its dependency refused, and the error holds both."""
    _package(unique_repo, "base", {"claude": {"language": "en"}})
    document = {"description": "team defaults", "claude": {"model": "sonnet"}}
    team = make_settings(f"{unique_repo}/team", "1", document, requires=[("settings", "base", "./base:1")])
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    assert _file(target) == {"model": "sonnet", "language": "en"}
    _put(target, {"model": "sonnet", "language": "de"})

    failed = runner.run("uninstall", "settings", "team", format="json", check=False)

    assert failed.returncode == 65, failed.stderr
    error = _error(failed)
    assert _listed(error["settings_changes"]) == [REMOVED_MODEL]
    assert [piece["pointer"] for piece in error["pieces"]] == ["/language"]
    assert _file(target) == {"language": "de"}
    _closes_the_list(failed, f"removed claude {LOCAL} /model (team)")


@needs_an_unwritable_directory
def test_uninstall_state_failure_keeps_changes(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)

    state_dir = project_dir / ".grimoire"
    state_dir.chmod(0o555)
    try:
        failed = runner.run("uninstall", "settings", "team", format="json", check=False)
    finally:
        state_dir.chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(project_dir / LOCAL) == {}
    assert _listed(_error(failed)["settings_changes"]) == [REMOVED_MODEL]
    _closes_the_list(failed, f"removed claude {LOCAL} /model (team)")


@needs_an_unwritable_directory
def test_uninstall_undeclare_failure_keeps_changes(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The lock write comes last, after the value already left."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    # Written through the link, so only this write meets the closed directory.
    held = tmp_path / "held"
    held.mkdir()
    lock = project_dir / "grimoire.lock"
    lock.rename(held / "grimoire.lock")
    lock.symlink_to(held / "grimoire.lock")

    held.chmod(0o555)
    try:
        failed = runner.run("uninstall", "settings", "team", format="json", check=False)
    finally:
        held.chmod(0o755)

    assert failed.returncode == 77, failed.stderr
    assert _file(project_dir / LOCAL) == {}
    assert _listed(_error(failed)["settings_changes"]) == [REMOVED_MODEL]
    _closes_the_list(failed, f"removed claude {LOCAL} /model (team)")


def test_add_conflict_leaves_the_lock(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Both forms of ``add`` refuse before the save, with the rows an install would print."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": {"model": "opus"}})
    gamma = _package(unique_repo, "gamma", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"alpha": alpha})
    runner.run("consent", "allow")
    runner.run("install")
    lock = project_dir / "grimoire.lock"
    files = [project_dir / name for name in ("grimoire.toml", "grimoire.lock", LOCAL)]
    held = [path.read_bytes() for path in files]

    for flags in ((), ("--no-install",)):
        result = runner.run("add", *flags, beta.fq, format="json", check=False)

        assert _conflicts(result) == [("alpha", "/model", "conflict", "sonnet"), ("beta", "/model", "conflict", "opus")]
        assert "conflict claude /model (beta)\n" in result.stderr
        assert [path.read_bytes() for path in files] == held

    # Positive control: a package that agrees reaches the save the refusal stopped short of.
    runner.run("add", gamma.fq)
    assert gamma.digest in lock.read_text(encoding="utf-8")
    assert _file(project_dir / LOCAL) == {"model": "sonnet", "language": "en"}


def test_lock_conflict_exits_78(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A consented workspace's ``lock`` refuses what it resolved, and keeps the lock it had."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": {"language": "en"}})
    agreed = _package(unique_repo, "beta", {"claude": {"language": "de"}}, tag="2")
    clash = _package(unique_repo, "beta", {"claude": {"model": "opus"}}, tag="3")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"alpha": alpha, "beta": beta})
    runner.run("consent", "allow")
    lock = project_dir / "grimoire.lock"

    # Positive control: past the check a package that agrees is saved.
    write_config(project_dir, settings={"alpha": alpha.fq, "beta": agreed.fq}, experimental_settings=True)
    runner.run("lock")
    assert agreed.digest in lock.read_text(encoding="utf-8")
    held = lock.read_bytes()

    write_config(project_dir, settings={"alpha": alpha.fq, "beta": clash.fq}, experimental_settings=True)
    result = runner.run("lock", format="json", check=False)

    assert _conflicts(result) == [("alpha", "/model", "conflict", "sonnet"), ("beta", "/model", "conflict", "opus")]
    assert "conflict claude /model (alpha)\n" in result.stderr
    assert lock.read_bytes() == held


@pytest.mark.parametrize(("flag", "consented"), [(False, True), (True, False)], ids=["flag-off", "fresh-clone"])
def test_lock_reads_no_layer_it_would_not_arm(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, flag: bool, consented: bool
) -> None:
    """Off, or on a clone nobody consented to, ``lock`` resolves as it did before the kind."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": {"model": "opus"}})
    write_config(project_dir, settings={"alpha": alpha.fq, "beta": beta.fq}, experimental_settings=flag)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    lock = project_dir / "grimoire.lock"
    consent = Path(runner.grim_home) / "hooks/consent"

    runner.run("lock")
    if consented:
        runner.run("consent", "allow")
    runner.run("lock")

    text = lock.read_text(encoding="utf-8")
    assert alpha.digest in text
    assert beta.digest in text
    assert not _layer_cached(runner, alpha)
    assert not _layer_cached(runner, beta)
    assert len(list(consent.glob("*.json"))) == int(consented)

    # Positive control: on and consented, the same command reads both layers and refuses.
    runner.run("config", "set", "options.experimental.settings", "true")
    runner.run("consent", "allow")
    assert _conflicts(runner.run("lock", format="json", check=False))
    assert _layer_cached(runner, alpha)
    assert _layer_cached(runner, beta)


def test_lock_with_the_flag_off_reads_no_install_state(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Off, the check never starts, so install state ``lock`` did not read before fails nothing."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    write_config(project_dir, settings={"alpha": alpha.fq})
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    state = project_dir / ".grimoire/state.json"
    state.parent.mkdir()
    state.write_text("not json", encoding="utf-8")

    runner.run("lock")

    # Positive control: on, the same command reaches the state and refuses it.
    runner.run("config", "set", "options.experimental.settings", "true")
    assert runner.run("lock", check=False).returncode == 74


def test_update_conflict_saves_nothing(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A floating tag that now clashes moves no pin and no value, and lists no change."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": {"language": "en"}}, tag="stable")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"alpha": alpha, "beta": beta})
    runner.run("install", TRUST)
    lock, local = project_dir / "grimoire.lock", project_dir / LOCAL

    # Positive control: a bump that agrees moves the pin and the value.
    agreed = _package(unique_repo, "beta", {"claude": {"language": "de"}}, tag="stable")
    runner.run("update", TRUST)
    assert agreed.digest in lock.read_text(encoding="utf-8")
    assert _file(local) == {"model": "sonnet", "language": "de"}
    held = lock.read_bytes(), local.read_bytes()

    _package(unique_repo, "beta", {"claude": {"model": "opus"}}, tag="stable")
    result = runner.run("update", TRUST, format="json", check=False)

    assert _conflicts(result) == [("alpha", "/model", "conflict", "sonnet"), ("beta", "/model", "conflict", "opus")]
    assert (lock.read_bytes(), local.read_bytes()) == held
    assert RESTART not in result.stderr


def test_hand_edited_lock_conflict_exits_78(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """``install`` meets a pulled lock's conflict first, so the skill beside it waits too."""
    alpha = _package(unique_repo, "alpha", {"claude": {"model": "sonnet"}})
    beta = _package(unique_repo, "beta", {"claude": {"language": "en"}})
    clash = _package(unique_repo, "beta", {"claude": {"model": "opus"}}, tag="2")
    skill = _skill(unique_repo)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"alpha": alpha, "beta": beta}, skills={"good": skill.fq})
    _repin(project_dir, beta, clash)
    lock, local = project_dir / "grimoire.lock", project_dir / LOCAL
    installed = project_dir / ".claude/skills/good/SKILL.md"
    held = lock.read_bytes()

    result = runner.run("install", TRUST, format="json", check=False)

    assert _conflicts(result) == [("alpha", "/model", "conflict", "sonnet"), ("beta", "/model", "conflict", "opus")]
    assert not local.exists()
    assert not installed.exists()
    assert lock.read_bytes() == held

    # Positive control: the same run over the agreed pin writes the file and the skill.
    _repin(project_dir, clash, beta)
    runner.run("install", TRUST)
    assert _file(local) == {"model": "sonnet", "language": "en"}
    assert installed.is_file()


def test_a_layer_naming_an_unknown_setting_carries_the_upgrade_hint(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"futureKey": 1}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})

    plain = runner.run("install", TRUST, check=False)
    assert plain.returncode == 65, plain.stderr
    assert "upgrade grim" in plain.stderr

    error = _error(runner.run("install", TRUST, format="json", check=False))
    assert error["reason"] == "unsupported-artifact-feature", error
    assert "claude has no setting at /futureKey" in error["message"], error
    assert "upgrade grim" in error["hint"], error
    assert not (project_dir / LOCAL).exists()


def test_a_layer_table_for_an_unknown_client_is_named_in_a_warning(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "newclient": {"a": 1}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})

    result = runner.run("install", TRUST)

    assert "'newclient'" in result.stderr, result.stderr
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
