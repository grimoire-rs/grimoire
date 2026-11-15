# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`--locked` refuses a command that writes `grimoire.lock` when the lock no
longer matches `grimoire.toml`, or is missing, and writes nothing.

`GRIM_LOCKED` is its environment form. In sync, a run with the flag is the run
without it. `install` already refuses a stale lock and `status` stays a report,
so the flag changes neither. Every artifact here is a path source, so no test
needs a registry.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import pytest

from src.helpers import printed_argv
from src.runner import GrimRunner

# Each command that writes the lock, as typed after the global flags.
COMMANDS = {
    "add": ("add", "./skills/c"),
    "remove": ("remove", "skill", "b"),
    "uninstall": ("uninstall", "skill", "b"),
    "lock": ("lock",),
    "update": ("update",),
    "update-named": ("update", "a"),
}
REMEDY = r"run `([^`]+)` without --locked or GRIM_LOCKED, then this command again"
MISSING_REMEDY = r"create it with `([^`]+)` without --locked or GRIM_LOCKED"


def _skill(root: Path, name: str) -> None:
    path = root / "skills" / name
    path.mkdir(parents=True, exist_ok=True)
    (path / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: d\n---\n# {name}\n", encoding="utf-8", newline="\n"
    )


def _declare(root: Path, *names: str) -> None:
    lines = "".join(f'{n} = "./skills/{n}"\n' for n in names)
    (root / "grimoire.toml").write_text(f"[skills]\n{lines}", encoding="utf-8", newline="\n")


def _workspace(grim_at, root: Path) -> GrimRunner:
    """`a` and `b` declared, locked and installed; `c` and `d` exist but are not declared."""
    (root / ".claude").mkdir(parents=True, exist_ok=True)
    for name in "abcd":
        _skill(root, name)
    _declare(root, "a", "b")
    runner = grim_at(root)
    runner.run("lock")
    runner.run("install")
    return runner


def _make_stale(root: Path) -> None:
    """Declare `d` by hand, after the last lock."""
    _declare(root, "a", "b", "d")


def _snapshot(root: Path) -> dict[str, bytes]:
    files = ["grimoire.toml", "grimoire.lock", ".grimoire/state.json"]
    return {f: (root / f).read_bytes() for f in files if (root / f).is_file()}


def _lock_body(root: Path) -> str:
    """The lock without its `generated_at` line, which two runs never share."""
    text = (root / "grimoire.lock").read_text(encoding="utf-8")
    return "".join(line for line in text.splitlines(keepends=True) if not line.startswith("generated_at"))


def _error(result: subprocess.CompletedProcess[str], exit_code: int) -> dict:
    assert result.returncode == exit_code, (result.stdout, result.stderr)
    return json.loads(result.stdout)["error"]


@pytest.mark.parametrize("command", COMMANDS.values(), ids=COMMANDS.keys())
def test_a_stale_lock_refuses_65_and_writes_nothing(
    grim_at, project_dir: Path, command: tuple[str, ...]
) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)
    before = _snapshot(project_dir)

    error = _error(runner.run("--locked", *command, format="json", check=False), 65)

    assert error["reason"] == "stale-lock", error
    assert re.search(REMEDY, error["message"]), error
    assert _snapshot(project_dir) == before


@pytest.mark.parametrize("command", COMMANDS.values(), ids=COMMANDS.keys())
def test_the_printed_remedy_brings_the_lock_in_line(
    grim_at, project_dir: Path, command: tuple[str, ...]
) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)
    error = _error(runner.run("--locked", *command, format="json", check=False), 65)
    remedy = re.search(REMEDY, error["message"])
    assert remedy, error

    followed = runner.run(*printed_argv(remedy.group(1))[1:], check=False)
    assert followed.returncode == 0, followed.stderr

    again = runner.run("--locked", *command, check=False)
    assert again.returncode == 0, again.stderr


@pytest.mark.parametrize("command", COMMANDS.values(), ids=COMMANDS.keys())
def test_a_missing_lock_refuses_79_no_lock(
    grim_at, project_dir: Path, command: tuple[str, ...]
) -> None:
    runner = _workspace(grim_at, project_dir)
    (project_dir / "grimoire.lock").unlink()
    before = _snapshot(project_dir)

    error = _error(runner.run("--locked", *command, format="json", check=False), 79)

    assert error["reason"] == "no-lock", error
    assert re.search(MISSING_REMEDY, error["message"]), error
    assert "run `grim lock` first" in error["message"], error
    assert _snapshot(project_dir) == before


@pytest.mark.parametrize("command", COMMANDS.values(), ids=COMMANDS.keys())
def test_in_sync_the_flag_changes_nothing(
    grim_at, tmp_path: Path, command: tuple[str, ...]
) -> None:
    plain = _workspace(grim_at, tmp_path / "plain")
    locked = _workspace(grim_at, tmp_path / "locked")

    without = plain.run(*command, check=False)
    with_flag = locked.run("--locked", *command, check=False)

    assert with_flag.returncode == without.returncode == 0, (without.stderr, with_flag.stderr)
    assert _lock_body(tmp_path / "locked") == _lock_body(tmp_path / "plain")


@pytest.mark.parametrize("command", COMMANDS.values(), ids=COMMANDS.keys())
def test_grim_locked_set_acts_as_the_flag(
    grim_at, project_dir: Path, command: tuple[str, ...]
) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)
    before = _snapshot(project_dir)
    runner.env["GRIM_LOCKED"] = "1"

    error = _error(runner.run(*command, format="json", check=False), 65)

    assert error["reason"] == "stale-lock", error
    assert _snapshot(project_dir) == before


def test_with_grim_locked_set_the_printed_remedy_runs_once_it_is_unset(
    grim_at, project_dir: Path
) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)
    runner.env["GRIM_LOCKED"] = "1"
    command = COMMANDS["add"]

    remedy = re.search(REMEDY, _error(runner.run(*command, format="json", check=False), 65)["message"])
    assert remedy, "the refusal names its remedy"
    remedy_argv = printed_argv(remedy.group(1))[1:]
    still_set = runner.run(*remedy_argv, check=False)
    assert still_set.returncode == 65, "the remedy itself refuses while GRIM_LOCKED is set"

    del runner.env["GRIM_LOCKED"]
    followed = runner.run(*remedy_argv, check=False)
    assert followed.returncode == 0, followed.stderr
    runner.env["GRIM_LOCKED"] = "1"
    again = runner.run(*command, check=False)
    assert again.returncode == 0, again.stderr


def test_with_grim_locked_set_a_missing_lock_names_the_lock_to_run_without_it(
    grim_at, project_dir: Path
) -> None:
    runner = _workspace(grim_at, project_dir)
    (project_dir / "grimoire.lock").unlink()
    runner.env["GRIM_LOCKED"] = "1"
    command = COMMANDS["add"]

    error = _error(runner.run(*command, format="json", check=False), 79)
    assert error["reason"] == "no-lock", error
    remedy = re.search(MISSING_REMEDY, error["message"])
    assert remedy, error

    del runner.env["GRIM_LOCKED"]
    followed = runner.run(*printed_argv(remedy.group(1))[1:], check=False)
    assert followed.returncode == 0, followed.stderr
    assert (project_dir / "grimoire.lock").is_file()
    runner.env["GRIM_LOCKED"] = "1"
    again = runner.run(*command, check=False)
    assert again.returncode == 0, again.stderr


def test_grim_locked_off_relocks_as_without_it(grim_at, project_dir: Path) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)
    runner.env["GRIM_LOCKED"] = "0"

    result = runner.run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert 'name = "d"' in (project_dir / "grimoire.lock").read_text(encoding="utf-8")


@pytest.mark.parametrize("verb", ["remove", "uninstall"])
def test_no_relock_does_not_skip_the_check(grim_at, project_dir: Path, verb: str) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)
    before = _snapshot(project_dir)

    result = runner.run("--locked", verb, "--no-relock", "skill", "b", format="json", check=False)

    assert _error(result, 65)["reason"] == "stale-lock"
    assert _snapshot(project_dir) == before


def test_a_global_refusal_names_the_global_lock(grim: GrimRunner, grim_home: Path) -> None:
    for name in "ab":
        _skill(grim_home, name)
    _declare(grim_home, "a")
    grim.run("--global", "lock")
    _declare(grim_home, "a", "b")

    error = _error(grim.run("--global", "--locked", "lock", format="json", check=False), 65)

    remedy = re.search(REMEDY, error["message"])
    assert remedy and remedy.group(1) == "grim --global lock", error


def test_install_refuses_a_stale_lock_as_it_does_without_the_flag(
    grim_at, project_dir: Path
) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)

    without = runner.run("install", format="json", check=False)
    with_flag = runner.run("--locked", "install", format="json", check=False)

    assert with_flag.returncode == without.returncode == 65, without.stderr
    assert json.loads(with_flag.stdout) == json.loads(without.stdout)


def test_status_stays_a_report_on_a_stale_lock(grim_at, project_dir: Path) -> None:
    runner = _workspace(grim_at, project_dir)
    _make_stale(project_dir)

    result = runner.run("--locked", "status", check=False)

    assert result.returncode == 0, result.stderr
