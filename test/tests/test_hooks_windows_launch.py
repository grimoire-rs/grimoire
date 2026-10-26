# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Hooks arm on Windows and the registered command really launches (S-010, C-023).

Each client is admitted to `HOOKS_WINDOWS_VERIFIED` only with a green run of
this file. A client's row in `CLIENTS` is its admission evidence: grim installs
the hook, the test reads back the registration the client would read, runs it
the way that client launches a hook command, and requires the payload to have
run. The positive control is the marker the payload writes, because every
refusal on this path exits 0 by design (I3).

Launch paths, each the closest a test can get without the client's own login:

- Claude hands the command string to Git Bash, or to PowerShell when the row
  carries `"shell": "powershell"`. A bare path is run through both.

Copilot and Codex are not admitted: their CLIs are not installed on the test
host, so only emulated launches ever passed, and an unobserved admission can
deny every tool call. Their launch tests return with a live run of each CLI;
`test_hooks_fixes_windows.py` pins the decline until then.
"""
from __future__ import annotations

import json
import os
import shutil
import struct
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import pytest

from src.helpers import make_artifact
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(os.name != "nt", reason="Windows launch paths")

SESSION_START = json.dumps({"hook_event_name": "SessionStart", "cwd": "C:/repo", "session_id": "s-1"})


@dataclass(frozen=True)
class Client:
    #: Directory under the isolated home whose presence selects the client.
    root: str
    #: The file grim writes the registration into, relative to the home.
    surface: str


CLIENTS = {
    "claude": Client(".claude", ".claude/settings.json"),
}

HOMES = {
    "plain": "grim-home",
    "space": "grim home",
    "apostrophe": "o'brien grim",
    "typographic": "o\u2019brien",
}


def _hook_toml() -> str:
    python = Path(sys.executable).as_posix()
    return f"""\
schema = 1
name = "shell-guard"
description = "Observes session starts."

[[hooks]]
id = "guard"
event = "SessionStart"
tier = "observer"
argv = ["{python}", "${{GRIM_HOOK_DIR}}/guard.py"]
timeout = 20
"""


def _guard_py(marker: Path) -> str:
    return (
        "import sys\n"
        f"open({str(marker)!r}, 'w', encoding='utf-8').write(sys.stdin.read())\n"
        "sys.stdout.write('{}')\n"
    )


@pytest.fixture()
def arming(grim: GrimRunner, tmp_path: Path):
    """Arm one hook at global scope under a chosen `$GRIM_HOME` name."""

    def arm(client: str, home_name: str = "plain", *, publish_as: str | None = None) -> Path:
        home = grim.grim_home.parent / HOMES[home_name]
        home.mkdir(exist_ok=True)
        grim.grim_home = home
        grim.env["GRIM_HOME"] = str(home)
        # tmp_path sits under the real %LOCALAPPDATA%, which grim requires for a
        # local home; the runner's env otherwise carries only USERPROFILE.
        grim.env["LOCALAPPDATA"] = os.environ["LOCALAPPDATA"]
        marker = tmp_path / "ran.txt"
        hook = make_artifact(
            f"grim-test/{publish_as or client}-{home_name}-{os.getpid()}",
            "hook",
            {"shell-guard/hook.toml": _hook_toml(), "shell-guard/guard.py": _guard_py(marker)},
            tag="1",
        )
        (grim.home / CLIENTS[client].root).mkdir(parents=True, exist_ok=True)
        (home / "grimoire.toml").write_text(
            f'[hooks]\nshell-guard = "{hook.fq}"\n\n[options.experimental]\nhooks = true\n'
        )
        grim.run("lock", "--global")
        grim.run("install", "--global", "--trust-hooks")
        return marker

    return arm


def _commands(surface: Path) -> list[dict]:
    """Every registration object in `surface` that points at the launcher."""
    found: list[dict] = []

    def walk(node) -> None:
        if isinstance(node, dict):
            if any("grim-hook" in str(node.get(key, "")) for key in ("command", "exec")):
                found.append(node)
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    walk(json.loads(surface.read_text(encoding="utf-8")))
    return found


def _git_bash() -> str | None:
    git = shutil.which("git")
    bash = Path(git).parent.parent / "bin" / "bash.exe" if git else None
    return str(bash) if bash and bash.is_file() else None


def _launches(client: str, row: dict) -> dict[str, list[str] | str]:
    """The ways `client` can start this registration, by name.

    A string is a raw Windows command line, handed to `CreateProcess` verbatim:
    a list would be re-quoted by `subprocess.list2cmdline`, which is not what a
    client does with a command string.
    """
    if client == "claude":
        command = row["command"]
        launches: dict[str, list[str] | str] = {
            "powershell": f'powershell.exe -NoProfile -NonInteractive -Command "{command}"'
            if '"' not in command
            else ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command]
        }
        if row.get("shell") != "powershell" and (bash := _git_bash()):
            launches["git-bash"] = [bash, "-c", command]
        return launches
    raise AssertionError(f"no launch path for {client}")


def _fire(launch: list[str] | str, stdin: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        launch, input=stdin, capture_output=True, text=True, encoding="utf-8", timeout=60, check=False
    )


@pytest.mark.parametrize("home_name", HOMES)
@pytest.mark.parametrize("client", CLIENTS)
def test_the_registered_command_launches_the_payload(arming, grim: GrimRunner, client: str, home_name: str) -> None:
    marker = arming(client, home_name)

    rows = _commands(grim.home / CLIENTS[client].surface)
    assert rows, f"{client}: nothing registered at {CLIENTS[client].surface}"
    for row in rows:
        for how, launch in _launches(client, row).items():
            marker.unlink(missing_ok=True)
            result = _fire(launch, SESSION_START)
            assert marker.is_file(), f"{client} via {how}: the payload never ran\n{result}\n{row}"
            envelope = json.loads(marker.read_text(encoding="utf-8"))
            assert (envelope["event"], envelope["client"]) == ("SessionStart", client), f"{client} via {how}"
            assert envelope["raw"] == json.loads(SESSION_START), f"{client} via {how}: stdin did not arrive intact"
            assert result.returncode == 0, f"{client} via {how}\n{result}"


def test_the_hooks_root_holds_only_the_launcher_and_it_loads_from_system32(arming, grim: GrimRunner) -> None:
    """C-023: no other `.exe` for a bare program name to resolve to, and the
    PE header carries `DependentLoadFlags = LOAD_LIBRARY_SEARCH_SYSTEM32`."""
    arming("claude")

    hooks = grim.grim_home / "hooks"
    assert sorted(p.name for p in hooks.glob("*.exe")) == ["grim-hook.exe"]

    # A second global hook bound as `cmd.exe` is refused, so the root still holds no other `.exe`.
    hostile = make_artifact(
        f"grim-test/cmd-exe-{os.getpid()}",
        "hook",
        {"shell-guard/hook.toml": _hook_toml(), "shell-guard/guard.py": _guard_py(hooks.parent / "never.txt")},
        tag="1",
    )
    config = grim.grim_home / "grimoire.toml"
    config.write_text(config.read_text().replace("[options", f'"cmd.exe" = "{hostile.fq}"\n\n[options', 1))
    grim.run("lock", "--global")
    grim.run("install", "--global", "--trust-hooks")
    assert sorted(p.name for p in hooks.glob("*.exe")) == ["grim-hook.exe"]

    data = (hooks / "grim-hook.exe").read_bytes()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    optional = pe + 24
    assert struct.unpack_from("<H", data, optional)[0] == 0x20B, "expected a PE32+ image"
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    table = optional + struct.unpack_from("<H", data, pe + 20)[0]
    load_config_rva = struct.unpack_from("<I", data, optional + 112 + 10 * 8)[0]
    assert load_config_rva, "no load config directory, so no DependentLoadFlags"
    for index in range(sections):
        _, size, virtual, _, raw = struct.unpack_from("<8sIIII", data, table + index * 40)
        if virtual <= load_config_rva < virtual + size:
            flags = struct.unpack_from("<H", data, load_config_rva - virtual + raw + 0x4E)[0]
            assert flags == 0x800, hex(flags)
            return
    raise AssertionError("the load config directory is in no section")


def test_a_home_outside_the_profile_is_refused_as_shared(arming, grim: GrimRunner, tmp_path: Path) -> None:
    """C-023: `grim-home-shared`, and nothing armable written."""
    home = tmp_path / "elsewhere"
    home.mkdir()
    grim.grim_home = home
    grim.env["GRIM_HOME"] = str(home)
    # Neither USERPROFILE (the isolated home) nor LOCALAPPDATA (unset) holds it.
    grim.env.pop("LOCALAPPDATA", None)
    (grim.home / ".claude").mkdir(parents=True, exist_ok=True)
    hook = make_artifact(
        f"grim-test/shared-{os.getpid()}",
        "hook",
        {"shell-guard/hook.toml": _hook_toml(), "shell-guard/guard.py": _guard_py(tmp_path / "ran.txt")},
        tag="1",
    )
    (home / "grimoire.toml").write_text(f'[hooks]\nshell-guard = "{hook.fq}"\n\n[options.experimental]\nhooks = true\n')
    grim.run("lock", "--global")
    grim.run("install", "--global", "--trust-hooks")

    (item,) = grim.json("hook", "list", "--global")["items"]
    assert {a["cause"] for a in item["arming"]} == {"grim-home-shared"}, item
    assert not (grim.home / ".claude" / "settings.json").exists()
    assert not (home / "hooks" / "grim-hook.exe").exists()


def test_a_unc_home_is_refused_as_shared(arming, grim: GrimRunner, tmp_path: Path) -> None:
    """The admin share of this machine's own drive is the nearest UNC path a test can reach."""
    home = tmp_path / "unc-home"
    home.mkdir()
    drive, rest = os.path.splitdrive(home)
    unc = Path(f"\\\\localhost\\{drive[0]}$" + rest)
    if not unc.is_dir():
        pytest.skip(f"administrative share {unc} is not reachable")
    grim.grim_home = unc
    grim.env["GRIM_HOME"] = str(unc)
    grim.env["LOCALAPPDATA"] = os.environ["LOCALAPPDATA"]
    (grim.home / ".claude").mkdir(parents=True, exist_ok=True)
    hook = make_artifact(
        f"grim-test/unc-{os.getpid()}",
        "hook",
        {"shell-guard/hook.toml": _hook_toml(), "shell-guard/guard.py": _guard_py(tmp_path / "ran.txt")},
        tag="1",
    )
    (unc / "grimoire.toml").write_text(f'[hooks]\nshell-guard = "{hook.fq}"\n\n[options.experimental]\nhooks = true\n')
    grim.run("lock", "--global")
    grim.run("install", "--global", "--trust-hooks")

    (item,) = grim.json("hook", "list", "--global")["items"]
    assert {a["cause"] for a in item["arming"]} == {"grim-home-shared"}, item
    assert not (home / "hooks" / "grim-hook.exe").exists()
