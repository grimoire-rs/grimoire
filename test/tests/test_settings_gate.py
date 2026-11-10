# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Who may apply settings, and what a run that may not leaves alone.

The run table of ``adr_settings_artifact_kind.md``, Decision 7: the flag, the
workspace's answer and ``GRIM_HOME`` each stop a run before it writes, and a
stopped run exits 0. Fixtures come from ``make_settings``. Every "nothing was
written" sits beside a run that does write, in the same test.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import select
import shutil
import subprocess
import time
from pathlib import Path
from subprocess import CompletedProcess

import pytest

from src.helpers import make_artifact, make_settings, settings_sandbox, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, fetch_manifest
from src.runner import GrimRunner

TRUST = "--trust-workspace"
LOCAL = ".claude/settings.local.json"
SKILL_DOC = "---\nname: good\ndescription: d\n---\n# Good\n"

# The one piece of the question its wording never moves.
PROMPT = "[y/N]"

TERMINAL_DEADLINE_SECONDS = 60

# What a run with the flag off prints for each declared package, by scope. The
# project flag does not cover `--global`, so each names its own.
PROJECT_GATED = (
    "settings package '{}' not applied: settings are gated; "
    "enable them with `grim config set options.experimental.settings true`"
)
GLOBAL_GATED = (
    "settings package '{}' not applied: settings are gated; "
    "enable them with `grim --global config set options.experimental.settings true`"
)


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


def _changes(result: CompletedProcess[str]) -> list[tuple[str, str, str | None, str]]:
    rows = json.loads(result.stdout)["settings_changes"]
    return sorted((row["package"], row["key"], row["element"], row["action"]) for row in rows)


def _repin(workspace: Path, old: PublishedArtifact, new: PublishedArtifact) -> None:
    lock = workspace / "grimoire.lock"
    text = lock.read_text(encoding="utf-8")
    assert old.digest in text
    lock.write_text(text.replace(old.digest, new.digest), encoding="utf-8", newline="\n")


def _file(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _store_files(runner: GrimRunner) -> list[Path]:
    """Every project settings record on this machine."""
    return sorted((Path(runner.grim_home) / "settings").glob("*.json"))


def _consent_files(runner: GrimRunner) -> list[Path]:
    """Every consent record on this machine."""
    return sorted((Path(runner.grim_home) / "hooks" / "consent").glob("*.json"))


def _layer_cached(runner: GrimRunner, package: PublishedArtifact) -> bool:
    """Whether this machine fetched ``package``'s layer. Its manifest is the edge check's."""
    [layer] = fetch_manifest(package.repo, package.tag)["layers"]
    digest = layer["digest"].removeprefix("sha256:")
    return any(digest in path.name for path in (Path(runner.grim_home) / "blobs").rglob("*"))


def _seed_record(runner: GrimRunner, workspace: Path, keys: list[str]) -> None:
    """Write the record an earlier consent left for ``workspace``.

    Written by hand, never by the grim under test: the shape is the shipped
    on-disk contract.
    """
    key = hashlib.sha256(str(workspace).encode()).hexdigest()
    path = Path(runner.grim_home) / "hooks" / "consent" / f"{key}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    record = {"v": 1, "workspace": str(workspace), "hooks": keys, "consented_at": "2026-08-28T10:00:00Z"}
    path.write_text(json.dumps(record), encoding="utf-8", newline="\n")


def _key(binding: str, package: PublishedArtifact) -> str:
    return f"settings:{binding}@{REGISTRY_HOST}/{package.repo}"


def _on_a_terminal(runner: GrimRunner, *args: str, answer: str | None = None) -> tuple[int, str]:
    """Run grim with stdin and stderr on a pseudo-terminal.

    Returns the exit code and everything printed there. ``answer`` is typed
    once the question appears. With no answer, a question nobody expected hangs
    the child until the deadline, which fails the test.
    """
    pty = pytest.importorskip("pty")
    master, slave = pty.openpty()
    child = subprocess.Popen(
        [str(runner.binary), *args],
        cwd=runner.cwd,
        env=runner.env,
        stdin=slave,
        stdout=subprocess.DEVNULL,
        stderr=slave,
    )
    os.close(slave)
    transcript = ""
    answered = answer is None
    deadline = time.monotonic() + TERMINAL_DEADLINE_SECONDS
    try:
        while True:
            remaining = deadline - time.monotonic()
            assert remaining > 0, f"grim never finished on its terminal:\n{transcript}"
            if not select.select([master], [], [], remaining)[0]:
                continue
            try:
                chunk = os.read(master, 4096)
            except OSError:  # Linux reports the closed far side as EIO.
                break
            if not chunk:
                break
            transcript += chunk.decode(errors="replace")
            if not answered and PROMPT in transcript:
                os.write(master, f"{answer}\n".encode())
                answered = True
    finally:
        os.close(master)
        if child.poll() is None:
            child.kill()
    return child.wait(timeout=TERMINAL_DEADLINE_SECONDS), transcript


def test_fresh_clone_writes_nothing(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A repository can turn the flag on for itself. That is not the workspace's answer."""
    skill = make_artifact(f"{unique_repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(project_dir, skills={"good": skill.fq}, settings={"team": team.fq}, experimental_settings=True)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")
    shutil.rmtree(Path(runner.grim_home) / "blobs")

    result = runner.run("install")

    assert result.returncode == 0, result.stderr
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()
    assert not (project_dir / LOCAL).exists()
    assert _store_files(runner) == []
    assert _consent_files(runner) == []
    assert not _layer_cached(runner, team), "a package past no gate is not fetched"

    # Positive control: the same clone, answered for this run, writes.
    runner.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
    assert len(_store_files(runner)) == 1
    assert _layer_cached(runner, team)


def test_trust_flag_applies(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """The flag answers for one run: no question, and nothing later runs inherit."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    target = project_dir / LOCAL

    code, shown = _on_a_terminal(runner, "install", TRUST)

    assert code == 0, shown
    assert PROMPT not in shown
    assert _file(target) == {"model": "sonnet"}
    assert _consent_files(runner) == []

    # Positive control: the plain run asks on that terminal, and accepting records.
    _repin(project_dir, team, retuned)
    code, shown = _on_a_terminal(runner, "install", answer="y")
    assert code == 0, shown
    assert PROMPT in shown
    assert _file(target) == {"model": "opus"}
    assert len(_consent_files(runner)) == 1


@pytest.mark.parametrize(
    ("answer", "flags"), [("n", ()), (None, ("--no-trust-workspace",))], ids=["at-the-prompt", "by-flag"]
)
def test_declined_changes_no_file(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, answer: str | None, flags: tuple[str, ...]
) -> None:
    """Told no, nothing moves, including an orphan the question never named."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    gone = _package(unique_repo, "gone", {"claude": {"language": "en"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "gone": gone})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    write_config(project_dir, settings={"team": team.fq}, experimental_settings=True)
    runner.run("lock")
    _repin(project_dir, team, retuned)
    file_bytes = target.read_bytes()
    [store] = _store_files(runner)
    store_bytes = store.read_bytes()

    code, shown = _on_a_terminal(runner, "install", *flags, answer=answer)

    assert code == 0, shown
    assert (PROMPT in shown) == (answer is not None)
    assert target.read_bytes() == file_bytes
    assert store.read_bytes() == store_bytes
    assert _consent_files(runner) == []

    # Positive control: accepted, the same state converges and the orphan goes.
    runner.run("install", TRUST)
    assert _file(target) == {"model": "opus"}


@pytest.mark.parametrize("flags", [(), (TRUST,)], ids=["unattended", "trusted"])
def test_orphan_with_a_changed_value_refuses_past_the_gate(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, flags: tuple[str, ...]
) -> None:
    """Removal needs no consent, so it is the piece rule that stops it, not the gate."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    write_config(project_dir, experimental_settings=True)
    runner.run("lock")
    target.write_text(json.dumps({"model": "opus"}), encoding="utf-8", newline="\n")

    result = runner.run("install", *flags, check=False)

    assert result.returncode == 65, result.stderr
    assert _file(target) == {"model": "opus"}

    # Positive control: with the value back the same run removes it.
    target.write_text(json.dumps({"model": "sonnet"}), encoding="utf-8", newline="\n")
    runner.run("install", *flags)
    assert _file(target) == {}


def test_unattended_run_applies_what_the_workspace_consented_to(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """With no terminal to ask on the consented converge, the rest wait, and orphans go."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    gone = _package(unique_repo, "gone", {"claude": {"language": "en"}})
    waiting = _package(unique_repo, "waiting", {"claude": {"effortLevel": "high"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "gone": gone})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    write_config(project_dir, settings={"team": team.fq, "waiting": waiting.fq}, experimental_settings=True)
    runner.run("lock")
    _repin(project_dir, team, retuned)
    _seed_record(runner, project_dir, [_key("team", team)])

    result = runner.run("install", format="json")

    assert result.returncode == 0, result.stderr
    assert _changes(result) == [("gone", "/language", None, "removed"), ("team", "/model", None, "changed")]
    assert _file(target) == {"model": "opus"}

    # Positive control: answered for the run, the waiting package lands.
    runner.run("install", TRUST)
    assert _file(target) == {"model": "opus", "effortLevel": "high"}


def test_gated_package_keeps_its_piece_through_another_s_drop(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """A gated package's record survives the run, so its piece is still held."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    write_config(project_dir, settings={"other": other.fq}, experimental_settings=True)
    runner.run("lock")

    result = runner.run("install", format="json")

    assert result.returncode == 0, result.stderr
    assert _changes(result) == []
    assert _file(target) == {"model": "sonnet"}

    # Positive control: once the holder leaves too, nothing holds the piece.
    write_config(project_dir, experimental_settings=True)
    runner.run("lock")
    runner.run("install")
    assert _file(target) == {}


@pytest.mark.parametrize("relative", [False, True], ids=["nested", "relative"])
def test_grim_home_in_workspace_writes_nothing(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str, relative: bool
) -> None:
    """Stopped before the question and before the conflict check, and it exits 0."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"model": "opus"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    nested = project_dir / ".grim-home"
    nested.mkdir()
    home = runner.env["GRIM_HOME"]
    runner.env["GRIM_HOME"] = ".grim-home" if relative else str(nested)

    code, shown = _on_a_terminal(runner, "install")

    assert code == 0, shown
    assert PROMPT not in shown
    assert "GRIM_HOME" in shown
    trusted = runner.run("install", TRUST)
    assert trusted.returncode == 0, trusted.stderr
    assert "GRIM_HOME" in trusted.stderr
    assert not (project_dir / LOCAL).exists()
    assert not (nested / "settings").exists()
    assert not (nested / "hooks" / "consent").exists()

    # Positive control: under a machine-local home the question and the conflict are real.
    runner.env["GRIM_HOME"] = home
    code, shown = _on_a_terminal(runner, "install", answer="n")
    assert code == 0, shown
    assert PROMPT in shown
    assert runner.run("install", TRUST, check=False).returncode == 78

    # Positive control: and without the conflict the same workspace writes.
    write_config(project_dir, settings={"team": team.fq}, experimental_settings=True)
    runner.run("lock")
    runner.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}


def test_a_record_under_a_refused_home_authorises_nothing(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Removal needs no consent, so a repository-delivered record must not reach it."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    write_config(project_dir, experimental_settings=True)
    runner.run("lock")
    home = runner.env["GRIM_HOME"]
    nested = project_dir / ".grim-home"
    shutil.copytree(Path(home) / "settings", nested / "settings")
    runner.env["GRIM_HOME"] = str(nested)

    result = runner.run("install", TRUST)

    assert result.returncode == 0, result.stderr
    assert _file(target) == {"model": "sonnet"}

    # Positive control: the same record under the real home does remove it.
    runner.env["GRIM_HOME"] = home
    runner.run("install", TRUST)
    assert _file(target) == {}


def test_a_relative_grim_home_at_global_scope_moves_no_value(
    grim, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Global scope has no workspace to nest in, but a relative home still follows the cwd."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(grim_home, settings={"team": team.fq}, experimental_settings=True)
    sandbox, roots = settings_sandbox(tmp_path / "s", grim.home)
    grim.env.update(sandbox)
    grim.run("--global", "lock")
    grim.run("--global", "install", "--client", "claude")
    target = roots["claude"] / "settings.json"
    assert _file(target) == {"model": "sonnet"}
    write_config(grim_home, experimental_settings=True)
    grim.run("--global", "lock")

    grim.cwd = grim_home.parent
    grim.env["GRIM_HOME"] = grim_home.name
    result = grim.run("--global", "install", "--client", "claude")

    assert result.returncode == 0, result.stderr
    assert "GRIM_HOME" in result.stderr
    assert _file(target) == {"model": "sonnet"}

    # Positive control: the record survived that run, so the absolute home removes the value.
    grim.env["GRIM_HOME"] = str(grim_home)
    grim.run("--global", "install", "--client", "claude")
    assert _file(target) == {}


def test_records_it_cannot_read_stop_the_conflict_check_too(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Exit 0 with the cause on stderr, the row a refused store has in the exit table."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    clash = _package(unique_repo, "other", {"claude": {"model": "opus"}}, tag="2")
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "other": other})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    held = target.read_bytes()
    _repin(project_dir, other, clash)

    # Positive control: with its records readable the same lock is a conflict.
    assert runner.run("install", TRUST, check=False).returncode == 78

    [store] = _store_files(runner)
    store.unlink()
    store.mkdir()
    result = runner.run("install", TRUST)

    assert result.returncode == 0, result.stderr
    assert "settings records" in result.stderr
    assert target.read_bytes() == held


def test_flag_off_leaves_values(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """Clearing the flag removes nothing, and no later run touches what stayed."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    retuned = _package(unique_repo, "team", {"claude": {"model": "opus"}}, tag="2")
    gone = _package(unique_repo, "gone", {"claude": {"language": "en"}})
    added = _package(unique_repo, "added", {"claude": {"effortLevel": "high"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team, "gone": gone})
    runner.run("install", TRUST)
    target = project_dir / LOCAL
    [store] = _store_files(runner)
    file_bytes, store_bytes = target.read_bytes(), store.read_bytes()

    # The flag goes, one package with it, and the other moves.
    write_config(project_dir, settings={"team": retuned.fq})
    runner.run("lock")

    for flags in ((), (TRUST,)):
        result = runner.run("install", *flags, format="json")
        assert result.returncode == 0, result.stderr
        assert _changes(result) == []
        assert target.read_bytes() == file_bytes
        assert store.read_bytes() == store_bytes

    # The flag gates writing, not declaring.
    runner.run("add", "--kind", "settings", added.fq)
    assert added.repo in (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert not _layer_cached(runner, added), "with the flag off nothing is fetched"
    assert target.read_bytes() == file_bytes
    assert store.read_bytes() == store_bytes
    assert _consent_files(runner) == []

    # Positive control: back on, the same state converges.
    runner.run("config", "set", "options.experimental.settings", "true")
    runner.run("install", TRUST)
    assert _file(target) == {"model": "opus", "effortLevel": "high"}


def test_a_global_add_under_the_project_flag_names_the_global_flag(
    grim_at, project_dir: Path, grim_home: Path, tmp_path: Path, unique_repo: str
) -> None:
    """The project flag does not cover `--global`, so the run says which flag is off."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(project_dir, experimental_settings=True)
    write_config(grim_home, options={"clients": json.dumps(["claude"])})
    runner = grim_at(project_dir)
    env, roots = settings_sandbox(tmp_path / "s", runner.home)
    runner.env.update(env)
    target = roots["claude"] / "settings.json"

    result = runner.run("add", "--global", "--kind", "settings", team.fq)

    assert result.returncode == 0, result.stderr
    assert team.repo in (grim_home / "grimoire.lock").read_text(encoding="utf-8")
    assert not _layer_cached(runner, team), "with the global flag off nothing is fetched"
    assert not target.exists()
    assert result.stderr.count(GLOBAL_GATED.format("team")) == 1, result.stderr

    # Positive control: the command the line names turns the kind on, and the value lands.
    runner.run("config", "set", "--global", "options.experimental.settings", "true")
    applied = runner.run("--global", "install")
    assert _file(target) == {"model": "sonnet"}
    assert "not applied" not in applied.stderr, applied.stderr


def test_a_flag_off_run_names_each_package_once(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """Exit 0 and nothing written, as before, and stderr says why and which flag to set."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    other = _package(unique_repo, "other", {"claude": {"language": "en"}})
    write_config(project_dir, settings={"team": team.fq, "other": other.fq})
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")

    for verb in ("install", "update"):
        result = runner.run(verb)
        assert result.returncode == 0, result.stderr
        for name in ("team", "other"):
            assert result.stderr.count(PROJECT_GATED.format(name)) == 1, result.stderr
        assert "set --global" not in result.stderr, result.stderr
    assert not (project_dir / LOCAL).exists()
    assert _store_files(runner) == []
    assert _consent_files(runner) == []
    assert not _layer_cached(runner, team), "with the flag off nothing is fetched"

    # Positive control: with the flag on the same run writes, and says nothing of the flag.
    runner.run("config", "set", "options.experimental.settings", "true")
    applied = runner.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"model": "sonnet", "language": "en"}
    assert "not applied" not in applied.stderr, applied.stderr


def test_no_line_without_a_settings_package(grim_at, project_dir: Path, tmp_path: Path, unique_repo: str) -> None:
    """A project without the kind prints what it printed before the kind existed."""
    skill = make_artifact(f"{unique_repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(project_dir, skills={"good": skill.fq})
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])
    runner.run("lock")

    result = runner.run("install")

    assert result.returncode == 0, result.stderr
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()
    assert "settings package" not in result.stderr, result.stderr

    # Positive control: the same run with a package declared names it.
    write_config(project_dir, skills={"good": skill.fq}, settings={"team": team.fq})
    runner.run("lock")
    declared = runner.run("install")
    assert declared.stderr.count(PROJECT_GATED.format("team")) == 1, declared.stderr


def test_commands_that_apply_nothing_say_nothing_of_the_flag(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """`lock`, `status` and `uninstall` hold no settings policy, so the flag is not theirs to name."""
    skill = make_artifact(f"{unique_repo}/good", "skill", {"good/SKILL.md": SKILL_DOC}, tag="1")
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    write_config(project_dir, skills={"good": skill.fq}, settings={"team": team.fq})
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "s", runner.home)[0])

    for args in (("lock",), ("status",)):
        result = runner.run(*args)
        assert "not applied" not in result.stderr, (args, result.stderr)
    assert "gated" in runner.run("status").stdout

    # Positive control: the run that holds the policy names the package.
    installed = runner.run("install")
    assert installed.stderr.count(PROJECT_GATED.format("team")) == 1, installed.stderr

    removed = runner.run("uninstall", "skill", "good")
    assert removed.returncode == 0, removed.stderr
    assert not (project_dir / ".claude/skills/good/SKILL.md").exists()
    assert "not applied" not in removed.stderr, removed.stderr


def test_path_source_in_a_hand_edited_lock_never_applies(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """No digest pins it, so even an explicit trust has nothing it may apply."""
    team = _package(unique_repo, "team", {"claude": {"model": "sonnet"}})
    runner = _workspace(grim_at, project_dir, tmp_path / "s", {"team": team})
    lock = project_dir / "grimoire.lock"
    pinned = lock.read_text(encoding="utf-8")
    edited, swapped = re.subn(
        r'^pinned = "[^"]*team[^"]*"$',
        f'path = "./team"\nhash = "sha256:{"a" * 64}"',
        pinned,
        flags=re.MULTILINE,
    )
    assert swapped == 1, pinned
    lock.write_text(edited, encoding="utf-8", newline="\n")

    result = runner.run("install", TRUST, format="json")

    assert result.returncode == 0, result.stderr
    assert _changes(result) == []
    assert not (project_dir / LOCAL).exists()
    assert _store_files(runner) == []

    # Positive control: the same package under its pin applies.
    lock.write_text(pinned, encoding="utf-8", newline="\n")
    runner.run("install", TRUST)
    assert _file(project_dir / LOCAL) == {"model": "sonnet"}
