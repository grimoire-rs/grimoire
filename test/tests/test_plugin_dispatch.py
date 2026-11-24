# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""External ``grim-<name>`` plugin dispatch (design 3.1, 3.2)."""
from __future__ import annotations

import os
import shutil
import stat
import sys
from pathlib import Path

import pytest

from src.runner import GrimRunner

WINDOWS = sys.platform == "win32"

# grim 0.x output for an unknown subcommand, captured from the pre-dispatch binary.
NOT_FOUND_STDERR = (
    "error: unrecognized subcommand 'nosuchcmd'\n"
    "\n"
    "  tip: a similar subcommand exists: 'schema'\n"
    "\n"
    # clap names the program after the binary file, which is `grim.exe` on Windows.
    f"Usage: {'grim.exe' if WINDOWS else 'grim'} [OPTIONS] [COMMAND]\n"
    "\n"
    "For more information, try '--help'.\n"
)

SCRIPT = """#!/bin/sh
echo "argv:$*"
echo "GRIM_BIN=${GRIM_BIN-unset}"
echo "GRIM_OFFLINE=${GRIM_OFFLINE-unset}"
echo "GRIM_LOCKED=${GRIM_LOCKED-unset}"
exit "${PROBE_EXIT:-0}"
"""


def _put_path(grim: GrimRunner, *dirs: str) -> None:
    grim.env["PATH"] = os.pathsep.join([*dirs, grim.env["PATH"]])


@pytest.fixture()
def plugin_dir(tmp_path: Path) -> Path:
    d = tmp_path / "plugins"
    d.mkdir()
    return d


def _script(directory: Path, name: str, mode: int = 0o755) -> Path:
    p = directory / name
    p.write_text(SCRIPT, encoding="utf-8", newline="\n")
    p.chmod(mode)
    return p


@pytest.mark.skipif(WINDOWS, reason="shell-script plugin; Windows uses the grim.exe copy below")
class TestUnixPlugin:
    def test_runs_with_argv_and_env(self, grim: GrimRunner, plugin_dir: Path) -> None:
        _script(plugin_dir, "grim-probe")
        _put_path(grim, str(plugin_dir))
        r = grim.run("probe", "a", "--b", "--", "c", check=False)
        assert r.returncode == 0, r.stderr
        assert "argv:a --b -- c" in r.stdout
        bin_line = next(l for l in r.stdout.splitlines() if l.startswith("GRIM_BIN="))
        assert Path(bin_line.split("=", 1)[1]) == Path(grim.binary).resolve()
        assert "GRIM_OFFLINE=unset" in r.stdout
        assert "GRIM_LOCKED=unset" in r.stdout

    def test_offline_and_locked_before_the_name_are_forwarded(
        self, grim: GrimRunner, plugin_dir: Path
    ) -> None:
        _script(plugin_dir, "grim-probe")
        _put_path(grim, str(plugin_dir))
        r = grim.run("--offline", "--locked", "probe", "x", check=False)
        assert r.returncode == 0, r.stderr
        assert "argv:x" in r.stdout
        assert "GRIM_OFFLINE=1" in r.stdout
        assert "GRIM_LOCKED=1" in r.stdout

    @pytest.mark.parametrize("code", [0, 3, 77])
    def test_exit_code_passes_through(self, grim: GrimRunner, plugin_dir: Path, code: int) -> None:
        _script(plugin_dir, "grim-probe")
        _put_path(grim, str(plugin_dir))
        grim.env["PROBE_EXIT"] = str(code)
        assert grim.run("probe", check=False).returncode == code

    def test_other_global_flag_before_the_name_is_refused(
        self, grim: GrimRunner, plugin_dir: Path
    ) -> None:
        _script(plugin_dir, "grim-probe")
        _put_path(grim, str(plugin_dir))
        r = grim.run("--format", "json", "probe", check=False)
        assert r.returncode == 64
        assert r.stdout == ""
        assert r.stderr == (
            "grim: '--format' before a plugin name is not supported; "
            "write it after the name: grim probe --format\n"
        )

    def test_non_executable_file_is_skipped(self, grim: GrimRunner, plugin_dir: Path) -> None:
        _script(plugin_dir, "grim-nosuchcmd", mode=0o644)
        _put_path(grim, str(plugin_dir))
        r = grim.run("nosuchcmd", check=False)
        assert r.returncode == 64
        assert r.stderr == NOT_FOUND_STDERR

    def test_relative_path_entry_is_ignored(
        self, grim: GrimRunner, plugin_dir: Path, tmp_path: Path
    ) -> None:
        _script(plugin_dir, "grim-nosuchcmd")
        _put_path(grim, plugin_dir.name)
        grim.cwd = plugin_dir.parent
        r = grim.run("nosuchcmd", check=False)
        assert r.returncode == 64
        assert r.stderr == NOT_FOUND_STDERR

    @pytest.mark.parametrize("entry", ["", "."])
    def test_empty_and_dot_path_entries_are_not_dispatched(
        self, grim: GrimRunner, plugin_dir: Path, entry: str
    ) -> None:
        _script(plugin_dir, "grim-nosuchcmd")
        _put_path(grim, entry)
        grim.cwd = plugin_dir
        r = grim.run("nosuchcmd", check=False)
        assert r.returncode == 64
        assert "GRIM_BIN=" not in r.stdout
        assert r.stderr == NOT_FOUND_STDERR

    def test_builtin_wins_over_a_plugin(self, grim: GrimRunner, plugin_dir: Path) -> None:
        _script(plugin_dir, "grim-install")
        _put_path(grim, str(plugin_dir))
        r = grim.run("install", "--help", check=False)
        assert r.returncode == 0
        assert "GRIM_BIN=" not in r.stdout
        assert "Usage:" in r.stdout

    @pytest.mark.parametrize("argv", [("config", "config"), ("hook", "hook"), ("consent", "consent")])
    def test_builtin_name_as_the_invalid_token_is_not_dispatched(
        self, grim: GrimRunner, plugin_dir: Path, argv: tuple[str, ...]
    ) -> None:
        _script(plugin_dir, f"grim-{argv[0]}")
        _put_path(grim, str(plugin_dir))
        r = grim.run(*argv, check=False)
        assert r.returncode == 64
        assert "GRIM_BIN=" not in r.stdout

    def test_nested_unknown_subcommand_is_not_dispatched(
        self, grim: GrimRunner, plugin_dir: Path
    ) -> None:
        _script(plugin_dir, "grim-nosuch")
        _put_path(grim, str(plugin_dir))
        r = grim.run("config", "nosuch", check=False)
        assert r.returncode == 64
        assert "GRIM_BIN=" not in r.stdout

    def test_unexecutable_interpreter_exits_69_or_77(
        self, grim: GrimRunner, plugin_dir: Path
    ) -> None:
        p = plugin_dir / "grim-probe"
        p.write_text("#!/nonexistent/interp\n", encoding="utf-8", newline="\n")
        p.chmod(stat.S_IRWXU)
        _put_path(grim, str(plugin_dir))
        r = grim.run("probe", check=False)
        assert r.returncode in (69, 77)
        assert f"grim: cannot run {p}" in r.stderr


def test_not_found_with_a_global_flag_is_clap_unchanged(grim: GrimRunner) -> None:
    r = grim.run("--format", "json", "nosuchcmd", check=False)
    assert r.returncode == 64
    assert r.stdout == ""
    assert r.stderr == NOT_FOUND_STDERR


def test_not_found_stderr_is_clap_unchanged(grim: GrimRunner) -> None:
    r = grim.run("nosuchcmd", check=False)
    assert r.returncode == 64
    assert r.stdout == ""
    assert r.stderr == NOT_FOUND_STDERR


@pytest.mark.skipif(not WINDOWS, reason="Windows dispatch proof")
def test_windows_runs_a_grim_exe_copy_and_ignores_bat(
    grim: GrimRunner, plugin_dir: Path
) -> None:
    shutil.copy(grim.binary, plugin_dir / "grim-probe.exe")
    (plugin_dir / "grim-batty.bat").write_text("@echo ran\r\n", encoding="utf-8", newline="")
    _put_path(grim, str(plugin_dir))
    r = grim.run("probe", "--version", check=False)
    assert r.returncode == 0 and "grim" in r.stdout
    assert grim.run("probe", "--no-such-flag", check=False).returncode == 64
    b = grim.run("batty", check=False)
    assert b.returncode == 64 and "ran" not in b.stdout
