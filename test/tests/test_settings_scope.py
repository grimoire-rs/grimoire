# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Where a settings piece lands, and what declines when it cannot land.

``adr_settings_artifact_kind.md``, Decisions 3, 8 and 9. A decline is a warning
and exit 0: a value a project may not take, a client whose file is absent, a
package nothing here applies to. At project scope Claude's file is the
per-developer ``settings.local.json``, every write passes the workspace
confinement, and the git exclude rule stays while either kind uses that file.
Fixtures come from ``make_settings``; each "nothing was written" sits beside a
run that does write, in the same test.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import make_artifact, make_hook, make_settings, settings_sandbox, write_config
from src.registry import PublishedArtifact
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
TRACKED = ".claude/settings.json"
MARKER = "hook-dispatcher"
DENY = "Bash(rm:*)"
MATCHING = '{"model": "sonnet", "theme": "dark"}'
HELD = '{"model": "opus", "theme": "dark"}'
GARBAGE = "not json at all"

CLAUDE_ONLY = {"clients": json.dumps(["claude"])}

unix_git = pytest.mark.skipif(
    shutil.which("git") is None or os.name == "nt", reason="needs git and POSIX symlinks"
)
# What the far end of a link holds: a value grim wrote, a value it did not, or
# a file it cannot parse. None of them may fail the run or come back written.
far_ends = pytest.mark.parametrize("held", [MATCHING, HELD, GARBAGE], ids=["matching", "differing", "unparseable"])


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


def _global(
    grim: GrimRunner, home: Path, sandbox: Path, team: PublishedArtifact, clients: list[str]
) -> dict[str, Path]:
    """A global scope naming `clients` and locking `team`, with nothing installed yet."""
    write_config(
        home, settings={"team": team.fq}, options={"clients": json.dumps(clients)}, experimental_settings=True
    )
    env, roots = settings_sandbox(sandbox, grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")
    return roots


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    rows = json.loads(result.stdout)["settings_changes"]
    return sorted((row["package"], row["key"], row["element"], row["action"]) for row in rows)


def _file(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _put(path: Path, values: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(values), encoding="utf-8")


def _row(report: dict, name: str) -> dict:
    [row] = [item for item in report["items"] if item["name"] == name]
    return row


def _abandoned(rows: list[dict]) -> list[tuple[Path, str, str | None]]:
    return sorted((Path(row["path"]).resolve(), row["pointer"], row["element"]) for row in rows)


def _settings_records(runner: GrimRunner) -> list[dict]:
    """Every settings record on this machine, from either store."""
    project = sorted((Path(runner.grim_home) / "settings").glob("*.json"))
    state = Path(runner.grim_home) / "state" / "global.json"
    stores = project or ([state] if state.exists() else [])
    records = [record for path in stores for record in json.loads(path.read_text(encoding="utf-8"))["records"]]
    return [record for record in records if record["kind"] == "settings"]


def _pieces(runner: GrimRunner) -> list[tuple[str, str, str]]:
    """Each recorded piece as `(package, client, pointer)`."""
    return sorted(
        (record["name"], output["client"], output["entry"])
        for record in _settings_records(runner)
        for output in record["outputs"]
    )


# ── Git checkouts ────────────────────────────────────────────────────────


@pytest.fixture()
def git(tmp_path: Path):
    """Git isolated from the developer's own configuration."""
    home = tmp_path / "git-home"
    home.mkdir()
    env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1"}

    def run(cwd: Path, *args: str, check: bool = True) -> CompletedProcess[str]:
        return subprocess.run(
            ["git", *args], cwd=cwd, env=env, check=check, capture_output=True, text=True, encoding="utf-8"
        )

    return run


@pytest.fixture()
def checkouts(tmp_path: Path, git) -> tuple[Path, Path]:
    """A primary checkout and one worktree linked to it."""
    primary = tmp_path / "primary"
    primary.mkdir()
    git(primary, "init", "-q", "-b", "main")
    git(primary, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "seed")
    linked = tmp_path / "linked"
    git(primary, "worktree", "add", "-q", str(linked), "-b", "topic")
    (primary / ".claude").mkdir()
    (linked / ".claude").mkdir()
    return primary, linked


def _rule_source(git, checkout: Path) -> Path | None:
    """The file whose rule hides the local settings file, or None."""
    found = git(checkout, "check-ignore", "-v", LOCAL, check=False)
    assert found.returncode in (0, 1), found.stderr
    if found.returncode == 1:
        return None
    source = found.stdout.split("\t")[0].rsplit(":", 2)[0]
    return (checkout / source).resolve()


def _common_exclude(primary: Path) -> Path:
    return (primary / ".git" / "info" / "exclude").resolve()


# ── Where a project writes ───────────────────────────────────────────────


def test_project_add_writes_the_local_file(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "codex": {"model": "gpt-5"}})
    # A support directory is what makes grim write the tracked file at all.
    files = {"style.md": "# Style\n", "style/notes.md": "# Notes\n"}
    rule = make_artifact(f"{unique_repo}/style", "rule", files, tag="1")
    write_config(
        project_dir,
        rules={"style": rule.fq},
        options={"clients": json.dumps(["claude", "codex"])},
        experimental_settings=True,
    )
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")
    runner.run("install")

    result = runner.run("add", team.fq, format="json")

    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    assert _file(project_dir / TRACKED) == {"claudeMdExcludes": ["**/.claude/rules/style/**"]}
    assert "codex applies at global scope only" in result.stderr
    assert _changes(result) == [("team", "/model", None, "added")]
    assert f"added claude {LOCAL} /model (team)" in result.stderr


def test_project_default_mode_declines(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    tables = {"claude": {"model": "sonnet", "permissions": {"defaultMode": "auto"}}}
    team = _package(unique_repo, "team", tables)
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    target = project_dir / LOCAL

    result = runner.run("install", TRUST)

    assert result.returncode == 0, result.stderr
    assert "/permissions/defaultMode does not take this value in a project" in result.stderr
    assert _file(target) == {"model": "sonnet"}
    assert _pieces(runner) == [("team", "claude", "/model")]

    # Positive control: a value a project takes lands at that same key.
    tables = {"claude": {"model": "sonnet", "permissions": {"defaultMode": "plan"}}}
    better = _package(unique_repo, "team", tables, tag="2")
    write_config(project_dir, settings={"team": better.fq}, options=CLAUDE_ONLY, experimental_settings=True)
    runner.run("update", TRUST)
    assert _file(target) == {"model": "sonnet", "permissions": {"defaultMode": "plan"}}


def test_add_then_uninstall_leaves_no_empty_containers(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"permissions": {"deny": [DENY]}, "sandbox": {"enabled": True}}})
    target = project_dir / LOCAL
    _put(target, {"theme": "dark"})
    write_config(project_dir, options=CLAUDE_ONLY, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])

    runner.run("add", team.fq)

    assert _file(target) == {"theme": "dark", "permissions": {"deny": [DENY]}, "sandbox": {"enabled": True}}

    runner.run("uninstall", "settings", "team")

    assert _file(target) == {"theme": "dark"}


# ── Declines ─────────────────────────────────────────────────────────────


def test_cursor_absent_file_declines(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}, "cursor": {"approvalMode": "allowlist"}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["claude", "cursor"])
    xdg = Path(grim.env["XDG_CONFIG_HOME"]) / "cursor" / "cli-config.json"
    # Only Linux falls back to the XDG file; elsewhere the decline names `~/.cursor`'s.
    named = xdg if sys.platform == "linux" else roots["cursor"] / "cli-config.json"

    result = grim.run("--global", "install")

    assert result.returncode == 0, result.stderr
    assert "client-config-absent" in result.stderr
    assert "(client-config-absent); start cursor once so it creates that file, then run grim --global install" in (
        result.stderr
    )
    assert str(named) in result.stderr
    assert not xdg.exists()
    assert not (roots["cursor"] / "cli-config.json").exists()
    assert _file(roots["claude"] / "settings.json") == {"model": "sonnet"}
    assert _pieces(grim) == [("team", "claude", "/model")]


def test_cursor_required_pointers_stay(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(
        unique_repo, "team", {"cursor": {"permissions": {"allow": ["Shell(ls)"]}, "editor": {"vimMode": True}}}
    )
    roots = _global(grim, grim_home, tmp_path / "s", team, ["cursor"])
    target = roots["cursor"] / "cli-config.json"
    _put(target, {"version": 1, "permissions": {"allow": [], "deny": []}})

    grim.run("--global", "install")

    written = {"allow": ["Shell(ls)"], "deny": []}
    assert _file(target) == {"version": 1, "permissions": written, "editor": {"vimMode": True}}

    report = grim.json("--global", "uninstall", "settings", "team")

    assert _file(target) == {"version": 1, "permissions": {"allow": [], "deny": []}, "editor": {"vimMode": True}}
    assert _abandoned(report["abandoned_entries"]) == [(target.resolve(), "/editor/vimMode", None)]


def test_zero_applicable_exits_0(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"codex": {"model": "gpt-5"}})
    other = _package(unique_repo, "other", {"claude": {"model": "sonnet"}})
    runner = _workspace(
        grim_at, project_dir, tmp_path / "s", {"team": team, "other": other}, options=CLAUDE_ONLY
    )

    result = runner.run("install", TRUST, format="json")

    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert _row(report, "team")["status"] == "skipped"
    assert "settings package 'team'" in result.stderr
    assert "no-applicable-settings" in result.stderr
    # Codex takes settings at global scope only, so selecting it here would not help.
    assert "declares values for codex" in result.stderr
    assert "grim --global install" in result.stderr
    [record] = [record for record in _settings_records(runner) if record["name"] == "team"]
    assert record["outputs"] == []
    # Positive control: the package beside it applies, in the same run.
    assert _row(report, "other")["status"] == "installed"
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}

    # A run with nothing to do tells the same story.
    again = runner.run("install", TRUST, format="json")
    assert _row(json.loads(again.stdout), "team")["status"] == "skipped"
    assert "no-applicable-settings" in again.stderr


def test_zero_applicable_names_the_declared_harness(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    warp = {"agents": {"warp_agent": {"other": {"thinking_display_mode": "always_show"}}}}
    team = _package(unique_repo, "team", {"warp": warp})
    _global(grim, grim_home, tmp_path / "s", team, ["claude"])

    result = grim.run("--global", "install", format="json")

    assert result.returncode == 0, result.stderr
    assert _row(json.loads(result.stdout), "team")["status"] == "skipped"
    assert "no-applicable-settings" in result.stderr
    assert "declares values for warp" in result.stderr
    assert "--client <name>" in result.stderr
    assert "[options].clients" in result.stderr


def test_no_client_detected_exits_0(grim_at, tmp_path: Path, unique_repo: str) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    bare = tmp_path / "bare"
    bare.mkdir()
    runner = _workspace(grim_at, bare, tmp_path / "s", {"team": team})

    for _ in range(2):
        result = runner.run("install", TRUST, format="json")

        assert result.returncode == 0, result.stderr
        assert _row(json.loads(result.stdout), "team")["status"] == "skipped"
        assert not (bare / ".claude").exists()

    # Positive control: the client appears, and the same declaration applies.
    (bare / ".claude").mkdir()
    runner.run("install", TRUST)
    assert _file(bare / LOCAL) == {"model": "sonnet"}


# ── The exclude rule ─────────────────────────────────────────────────────


@unix_git
def test_linked_worktree_writes_its_own_file(
    grim_at, git, checkouts: tuple[Path, Path], tmp_path: Path, unique_repo: str
) -> None:
    primary, linked = checkouts
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, linked, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)

    runner.run("install", TRUST)

    assert _file(linked / LOCAL) == {"model": "sonnet"}
    assert not (primary / LOCAL).exists()
    assert _rule_source(git, linked) == _common_exclude(primary)

    # One rule serves every worktree, so a linked one never drops it.
    runner.run("uninstall", "settings", "team")
    assert _rule_source(git, linked) == _common_exclude(primary)

    # Positive control: the checkout that owns the file does drop it.
    owner = _workspace(grim_at, primary, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    owner.run("install", TRUST)
    assert _file(primary / LOCAL) == {"model": "sonnet"}
    owner.run("uninstall", "settings", "team")
    assert _rule_source(git, primary) is None


def _both_kinds(grim_at, checkout: Path, sandbox: Path, unique_repo: str) -> GrimRunner:
    """A checkout where a hook and a settings package each use the local file."""
    hook = make_hook(f"{unique_repo}/guard", "guard")
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(checkout, hooks={"guard": hook.fq}, settings={"team": team.fq}, options=CLAUDE_ONLY)
    runner = grim_at(checkout)
    runner.env.update(settings_sandbox(sandbox, runner.home)[0])
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("config", "set", "options.experimental.settings", "true")
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    text = (checkout / LOCAL).read_text(encoding="utf-8")
    assert MARKER in text
    assert _file(checkout / LOCAL)["model"] == "sonnet"
    return runner


@unix_git
def test_hook_removal_keeps_the_exclude_rule_settings_need(
    grim_at, git, checkouts: tuple[Path, Path], tmp_path: Path, unique_repo: str
) -> None:
    primary, _ = checkouts
    runner = _both_kinds(grim_at, primary, tmp_path / "s", unique_repo)
    assert _rule_source(git, primary) == _common_exclude(primary)

    runner.run("remove", "hook", "guard")
    runner.run("install")

    assert MARKER not in (primary / LOCAL).read_text(encoding="utf-8")
    assert _rule_source(git, primary) == _common_exclude(primary)

    # Positive control: with neither kind left, the rule goes.
    runner.run("uninstall", "settings", "team")
    assert _rule_source(git, primary) is None


@unix_git
def test_settings_removal_keeps_the_exclude_rule_hooks_need(
    grim_at, git, checkouts: tuple[Path, Path], tmp_path: Path, unique_repo: str
) -> None:
    primary, _ = checkouts
    runner = _both_kinds(grim_at, primary, tmp_path / "s", unique_repo)

    runner.run("uninstall", "settings", "team")

    assert "model" not in _file(primary / LOCAL)
    assert _rule_source(git, primary) == _common_exclude(primary)

    # Positive control: with neither kind left, the rule goes.
    runner.run("remove", "hook", "guard")
    runner.run("install")
    assert _rule_source(git, primary) is None


@unix_git
def test_tracked_local_file_warns(
    grim_at, git, checkouts: tuple[Path, Path], tmp_path: Path, unique_repo: str
) -> None:
    primary, _ = checkouts
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    target = primary / LOCAL
    _put(target, {"theme": "dark"})
    git(primary, "add", "-f", LOCAL)
    git(primary, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "tracked")
    runner = _workspace(grim_at, primary, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)

    result = runner.run("install", TRUST)

    assert result.returncode == 0, result.stderr
    assert "local-file-tracked" in result.stderr
    assert _file(target) == {"theme": "dark", "model": "sonnet"}
    assert _rule_source(git, primary) is None

    # Positive control: untracked, the same run adds the rule and stops warning.
    git(primary, "rm", "-q", "--cached", LOCAL)
    quiet = runner.run("install", TRUST)
    assert "local-file-tracked" not in quiet.stderr
    assert _rule_source(git, primary) == _common_exclude(primary)


# ── Confinement ──────────────────────────────────────────────────────────


def _plant(shape: str, workspace: Path, away: Path) -> Path:
    """A link at the local file, or at `.claude`, into `away`. Returns where a write would land."""
    away.mkdir(parents=True, exist_ok=True)
    if shape == "directory":
        shutil.rmtree(workspace / ".claude", ignore_errors=True)
        (workspace / ".claude").symlink_to(away, target_is_directory=True)
    else:
        (workspace / ".claude").mkdir(exist_ok=True)
        (workspace / LOCAL).symlink_to(away / "settings.local.json")
    return away / "settings.local.json"


@pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks")
@pytest.mark.parametrize(
    ("shape", "held"),
    [("file", HELD), ("file", GARBAGE), ("directory", HELD), ("dangling", None)],
    ids=["file", "unparseable-file", "directory", "dangling"],
)
def test_confinement_refusal_writes_nothing(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, shape: str, held: str | None
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    landing = _plant(shape, project_dir, tmp_path / "outside")
    if held is not None:
        landing.write_text(held, encoding="utf-8")

    result = runner.run("install", TRUST, format="json")

    assert result.returncode == 0, result.stderr
    assert "not-registered" in result.stderr
    assert _row(json.loads(result.stdout), "team")["status"] == "skipped"
    if held is None:
        assert not landing.exists()
    else:
        assert landing.read_text(encoding="utf-8") == held
    assert _pieces(runner) == []

    # Positive control: the same shape, leading inside the workspace, writes through.
    twin = tmp_path / "twin"
    twin.mkdir()
    control = _workspace(grim_at, twin, tmp_path / "s", {"team": team}, options=CLAUDE_ONLY)
    inside = _plant("directory" if shape == "directory" else "file", twin, twin / "dotfiles")
    inside.write_text("{}", encoding="utf-8")
    control.run("install", TRUST)
    assert _file(inside) == {"model": "sonnet"}


def _installed_twins(
    grim_at, project_dir: Path, tmp_path: Path, team: PublishedArtifact, clients: list[str], held: str
) -> tuple[GrimRunner, GrimRunner, Path]:
    """Two installed workspaces, the first with its recorded file now a link out.

    Returns `(linked, plain, outside)`. The link is planted after the install,
    as a later pull would, so a record sits on a file that now leads elsewhere.
    """
    options = {"clients": json.dumps(clients)}
    plain_dir = tmp_path / "plain"
    plain_dir.mkdir()
    linked = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team}, options=options)
    plain = _workspace(grim_at, plain_dir, tmp_path / "s", {"team": team}, options=options)
    linked.run("install", TRUST)
    plain.run("install", TRUST)
    outside = tmp_path / "outside" / "settings.local.json"
    outside.parent.mkdir()
    outside.write_text(held, encoding="utf-8")
    (project_dir / LOCAL).unlink()
    (project_dir / LOCAL).symlink_to(outside)
    return linked, plain, outside


@pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks")
@far_ends
def test_converge_writes_nothing_through_a_link_out(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, held: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    moved = _package(unique_repo, "team", {"claude": {"language": "en"}}, tag="2")
    linked, plain, outside = _installed_twins(grim_at, project_dir, tmp_path, team, ["claude"], held)

    for workspace in (project_dir, tmp_path / "plain"):
        write_config(workspace, settings={"team": moved.fq}, options=CLAUDE_ONLY, experimental_settings=True)
    refused = linked.run("update", TRUST, format="json")

    assert refused.returncode == 0, refused.stderr
    assert "not-registered" in refused.stderr
    assert outside.read_text(encoding="utf-8") == held
    assert ("team", "claude", "/model") in _pieces(linked)

    # A later run reads the same refusal, with a row that says so.
    again = linked.run("install", TRUST, format="json")
    assert _row(json.loads(again.stdout), "team")["status"] == "skipped"
    assert outside.read_text(encoding="utf-8") == held

    # Positive control: the same bump moves the plain workspace's file.
    plain.run("update", TRUST)
    assert _file(tmp_path / "plain" / LOCAL) == {"language": "en"}

    # The record never moved its pin, so the first good run finishes the job.
    (project_dir / LOCAL).unlink()
    _put(project_dir / LOCAL, {"model": "sonnet"})
    linked.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"language": "en"}
    assert outside.read_text(encoding="utf-8") == held


@pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks")
@far_ends
def test_reap_writes_nothing_through_a_link_out(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, held: str
) -> None:
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    linked, plain, outside = _installed_twins(grim_at, project_dir, tmp_path, team, ["claude", "codex"], held)

    narrowed = {"clients": json.dumps(["codex"])}
    for workspace in (project_dir, tmp_path / "plain"):
        write_config(workspace, settings={"team": team.fq}, options=narrowed, experimental_settings=True)
    refused = linked.run("update", TRUST, format="json")

    assert refused.returncode == 0, refused.stderr
    assert "not-registered" in refused.stderr
    assert outside.read_text(encoding="utf-8") == held
    row = _row(json.loads(refused.stdout), "team")
    assert (row["reaped_clients"], row["kept_modified_clients"]) == ([], ["claude"])

    # Positive control: the same narrowing reaps the plain workspace's file.
    reaped = plain.run("update", TRUST, format="json")
    assert _file(tmp_path / "plain" / LOCAL) == {}
    assert _row(json.loads(reaped.stdout), "team")["reaped_clients"] == ["claude"]


@pytest.mark.skipif(os.name == "nt", reason="needs POSIX symlinks")
@far_ends
def test_uninstall_writes_nothing_through_a_link_out(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, held: str
) -> None:
    """Uninstall reads neither the flag nor consent, so nothing before the write stops it."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    linked, plain, outside = _installed_twins(grim_at, project_dir, tmp_path, team, ["claude"], held)
    for workspace in (project_dir, tmp_path / "plain"):
        write_config(workspace, settings={"team": team.fq}, options=CLAUDE_ONLY, experimental_settings=False)

    refused = linked.run("uninstall", "settings", "team")

    assert refused.returncode == 0, refused.stderr
    assert "not-registered" in refused.stderr
    assert outside.read_text(encoding="utf-8") == held
    assert ("team", "claude", "/model") in _pieces(linked)

    # Positive control: the same command, flag off, empties the plain workspace's file.
    plain.run("uninstall", "settings", "team")
    assert _file(tmp_path / "plain" / LOCAL) == {}

    # The record stayed, so a good run judges the orphan and takes its value out.
    (project_dir / LOCAL).unlink()
    _put(project_dir / LOCAL, {"model": "sonnet"})
    write_config(project_dir, options=CLAUDE_ONLY, experimental_settings=True)
    linked.run("lock")
    linked.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {}


# ── Codex's rules file ───────────────────────────────────────────────────

RULES = [
    {"pattern": ["npm", "test"], "decision": "allow", "justification": 'say "hi" \\ done'},
    {"pattern": ["rm"], "decision": "forbidden"},
]
RENDERED = """\
prefix_rule(
    pattern = ["npm", "test"],
    decision = "allow",
    justification = "say \\"hi\\" \\\\ done",
)
prefix_rule(
    pattern = ["rm"],
    decision = "forbidden",
)
"""


def _state(grim: GrimRunner, name: str) -> str:
    [item] = [item for item in grim.json("--global", "status")["items"] if item["name"] == name]
    return item["state"]


def test_codex_rules_file_lifecycle(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """A package's rules are one whole file of its own, judged by its bytes.

    ``default.rules`` is Codex's and stays byte-identical through every run. The
    rule table holds for this file as for a key: a drifted file refuses until
    ``--force``, and a file that holds any bytes at all is a file, never one that
    fails to parse. The uninstall takes the file and the record with it.
    """
    team = _package(unique_repo, "team", {"codex": {"model": "gpt-5", "rules": RULES}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["codex"])
    config = roots["codex"] / "config.toml"
    rendered = roots["codex"] / "rules" / "grim-team.rules"
    default = roots["codex"] / "rules" / "default.rules"
    default.parent.mkdir()
    default.write_bytes(b'prefix_rule(pattern = ["ls"], decision = "allow")\n# mine\n')
    codex_owned = default.read_bytes()

    installed = grim.run("--global", "install", format="json")

    assert _changes(installed) == [("team", "/model", None, "added"), ("team", "/rules", None, "added")]
    assert rendered.read_text(encoding="utf-8") == RENDERED
    assert "rules" not in config.read_text(encoding="utf-8"), "the one row that is no config.toml key"
    assert 'model = "gpt-5"' in config.read_text(encoding="utf-8")
    [record] = _settings_records(grim)
    [output] = [output for output in record["outputs"] if "entry" not in output]
    assert output["target"] == {"anchor": "codex-root", "relative": "rules/grim-team.rules"}
    assert output["content_hash"] == f"sha256:{hashlib.sha256(RENDERED.encode()).hexdigest()}"
    assert "adopted" not in output
    assert _state(grim, "team") == "installed"

    # Nothing moved, so the second run has nothing to do or report.
    assert _changes(grim.run("--global", "install", format="json")) == []
    assert rendered.read_text(encoding="utf-8") == RENDERED

    # A file of any bytes is a drifted file, whatever a parser would make of it.
    for drifted in (b'prefix_rule(pattern = ["rm"], decision = "allow")\n', b"\xff\xfe not text"):
        rendered.write_bytes(drifted)
        refused = grim.run("--global", "install", format="json", check=False)
        assert refused.returncode == 65, refused.stderr
        error = json.loads(refused.stdout)["error"]
        assert (error["reason"], error["forceable"]) == ("modified", True), error
        assert [(row["pointer"], row["state"]) for row in error["pieces"]] == [("/rules", "modified")]
        assert rendered.read_bytes() == drifted, "a refusal leaves the file"
        assert _state(grim, "team") == "modified"

    forced = grim.run("--global", "install", "--force", format="json")
    assert _changes(forced) == [("team", "/rules", None, "overwritten")]
    assert rendered.read_text(encoding="utf-8") == RENDERED

    # A deleted file comes back, as a deleted key does.
    rendered.unlink()
    assert _state(grim, "team") == "missing"
    assert _changes(grim.run("--global", "install", format="json")) == [("team", "/rules", None, "added")]
    assert rendered.read_text(encoding="utf-8") == RENDERED

    # Uninstall judges the file too: it comes out only as recorded, or forced.
    rendered.write_text("# edited\n", encoding="utf-8")
    refused = grim.run("--global", "uninstall", "settings", "team", format="json", check=False)
    assert refused.returncode == 65, refused.stderr
    assert rendered.read_text(encoding="utf-8") == "# edited\n"
    assert 'model = "gpt-5"' in config.read_text(encoding="utf-8"), "one refusal moves nothing"
    assert _settings_records(grim) != []

    grim.run("--global", "uninstall", "settings", "team", "--force")

    assert not rendered.exists()
    assert "model" not in config.read_text(encoding="utf-8")
    assert [record for record in _settings_records(grim) if record["outputs"]] == []
    assert _settings_records(grim) == [], "no output keeps the record"
    assert default.read_bytes() == codex_owned


def test_codex_rules_file_adopts_what_it_would_write(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """A file already holding the bytes is adopted, and one holding others is not."""
    team = _package(unique_repo, "team", {"codex": {"rules": RULES}})
    roots = _global(grim, grim_home, tmp_path / "s", team, ["codex"])
    rendered = roots["codex"] / "rules" / "grim-team.rules"
    rendered.parent.mkdir()

    rendered.write_text("# mine\n", encoding="utf-8")
    refused = grim.run("--global", "install", format="json", check=False)
    assert refused.returncode == 65, refused.stderr
    assert json.loads(refused.stdout)["error"]["reason"] == "untracked-destination"
    assert rendered.read_text(encoding="utf-8") == "# mine\n"

    rendered.write_text(RENDERED, encoding="utf-8", newline="\n")
    assert _changes(grim.run("--global", "install", format="json")) == []
    [output] = _settings_records(grim)[0]["outputs"]
    assert output["adopted"] is True

    # An adopted file was never grim's, so the uninstall leaves it and says so.
    report = grim.json("--global", "uninstall", "settings", "team")
    assert rendered.read_text(encoding="utf-8") == RENDERED
    assert _abandoned(report["abandoned_entries"]) == [(rendered.resolve(), "/rules", None)]
    assert _settings_records(grim) == []


def test_codex_rules_empty_list_places_no_file(grim, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    """A package declaring no rules owns no file, though the list validates.

    The same package with rules in the same root places one, so the absent file
    is the empty list's doing and not a run that never reached the piece.
    """
    empty = _package(unique_repo, "team", {"codex": {"model": "gpt-5", "rules": []}})
    roots = _global(grim, grim_home, tmp_path / "s", empty, ["codex"])
    rendered = roots["codex"] / "rules" / "grim-team.rules"

    result = grim.run("--global", "install", format="json")

    assert _changes(result) == [("team", "/model", None, "added")]
    assert not rendered.exists()
    assert [output["entry"] for output in _settings_records(grim)[0]["outputs"]] == ["/model"]

    # Positive control: a rule in the list, in this root, is a file.
    other = _package(unique_repo, "other", {"codex": {"rules": RULES}})
    grim.run("--global", "add", "--name", "other", other.fq)
    assert (roots["codex"] / "rules" / "grim-other.rules").read_text(encoding="utf-8") == RENDERED
