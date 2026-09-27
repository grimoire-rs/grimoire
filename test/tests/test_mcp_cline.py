# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Cline MCP registration — global scope only, under Cline's own lock.

Cline's CLI and VS Code extension share one settings file,
`<cline dir>/data/settings/cline_mcp_settings.json`, and rewrite it whole
under a directory lock `<file>.lock` (cline/cline@252082b9). grim joins that
lock, writes the flat entry form with `${env:VAR}` references, and has no
project-scope surface to write.
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import pytest

from src.helpers import write_config

ENV_DESCRIPTOR = """\
description = "Server with an env reference."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
env = { GRIM_TOKEN = "${GITHUB_TOKEN}" }
"""


def _release(runner, src: Path, registry: str, unique_repo: str) -> str:
    descriptor = src / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(ENV_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def test_global_cline_registers_under_its_lock_and_self_heals(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """An install recorded before Cline MCP support (simulated by a
    Claude-only install) lists Cline under `outputs_pending`; the next install
    reclaims a crashed holder's stale lock, adds the flat entry without
    touching the user's servers, and releases the lock. A repeat install is a
    byte-identical no-op and uninstall removes only grim's entry."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    for marker in (".claude", ".cline"):
        (runner.home / marker).mkdir(parents=True, exist_ok=True)
    ref = _release(runner, tmp_path / "src", registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "claude")

    settings = runner.home / ".cline" / "data" / "settings" / "cline_mcp_settings.json"
    settings.parent.mkdir(parents=True)
    settings.write_text('{\n  "mcpServers": {\n    "mine": {"command": "keep-me"}\n  },\n  "extra": 1\n}\n')
    pending = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert any(o["client"] == "cline" for o in pending["outputs_pending"]), pending

    # A lock a crashed Cline left behind, older than Cline's 10 s threshold.
    lock_dir = settings.with_name(settings.name + ".lock")
    lock_dir.mkdir()
    (lock_dir / "owner.1.1.crashed").write_text("1.1.crashed")
    old = time.time() - 60
    os.utime(lock_dir, (old, old))

    result = runner.run("install", "--global", check=False)
    assert result.returncode == 0, result.stderr
    doc = json.loads(settings.read_text())
    assert doc["mcpServers"]["grim-mcp"] == {
        "type": "stdio",
        "command": "grim",
        "args": ["mcp"],
        "env": {"GRIM_TOKEN": "${env:GITHUB_TOKEN}"},
    }
    assert doc["mcpServers"]["mine"] == {"command": "keep-me"}, "user server preserved"
    assert doc["extra"] == 1, "foreign key preserved"
    assert not lock_dir.exists(), "stale lock reclaimed and ours released"
    assert sorted(p.name for p in settings.parent.iterdir()) == ["cline_mcp_settings.json"], (
        "no staging, stale or temp leftovers"
    )

    before = settings.read_bytes()
    runner.json("install", "--global")
    assert settings.read_bytes() == before, "repeat install must be byte-identical"
    row = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row
    assert row["outputs_pending"] == [], row

    runner.json("uninstall", "--global", "mcp", "grim-mcp")
    doc = json.loads(settings.read_text())
    assert "grim-mcp" not in doc["mcpServers"]
    assert doc["mcpServers"]["mine"] == {"command": "keep-me"}
    assert not lock_dir.exists()


def test_global_cline_honors_the_settings_path_override(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """`CLINE_MCP_SETTINGS_PATH` names the file outright, as it does for
    Cline itself."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    (runner.home / ".cline").mkdir()
    custom = runner.home / ".cline" / "custom" / "mcp.json"
    runner.env["CLINE_MCP_SETTINGS_PATH"] = str(custom)
    ref = _release(runner, tmp_path / "src", registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "cline")
    assert json.loads(custom.read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"
    assert not (runner.home / ".cline" / "data").exists(), "the default file is not written"


def test_project_scope_skips_cline_mcp(grim_at, project_dir: Path, registry: str, unique_repo: str) -> None:
    """Cline has no project MCP file: a project install warns and writes
    nothing for Cline while the other clients register normally."""
    runner = grim_at(project_dir)
    ref = _release(runner, project_dir / "src", registry, unique_repo)
    (project_dir / ".clinerules").mkdir()
    (project_dir / ".claude").mkdir(exist_ok=True)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    result = runner.run("install", check=False)
    assert result.returncode == 0, result.stderr
    assert "skipped for cline" in result.stderr and "project scope" in result.stderr, result.stderr
    assert json.loads((project_dir / ".mcp.json").read_text())["mcpServers"]["grim-mcp"]
    assert not (runner.home / ".cline").exists(), "nothing written for Cline"


def test_global_cline_keeps_its_own_keys_in_grims_entry(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Cline writes approvals and OAuth tokens into every server entry,
    grim's included. Those keys are not a user edit: status stays
    `installed`, a pin-change update rewrites grim's fields but keeps them,
    and uninstall still removes the whole entry."""
    from src.registry import retag
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    (runner.home / ".cline").mkdir()
    descriptor = tmp_path / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(ENV_DESCRIPTOR)
    repo_path = f"{unique_repo}/mcp/grim-mcp"
    repo = f"{registry}/{repo_path}"
    runner.json("release", str(descriptor), f"{repo}:1.0.0", "--kind", "mcp")
    runner.json("release", str(descriptor), f"{repo}:stable", "--kind", "mcp")
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{repo}:stable"\n')
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "cline")

    settings = runner.home / ".cline" / "data" / "settings" / "cline_mcp_settings.json"
    doc = json.loads(settings.read_text())
    entry = doc["mcpServers"]["grim-mcp"]
    entry["autoApprove"] = ["grim_search"]
    entry["oauth"] = {"tokens": {"access_token": "t"}}
    settings.write_text(json.dumps(doc, indent=2) + "\n")  # Cline rewrites the file whole

    def row() -> dict:
        return next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")

    assert row()["state"] == "installed", "Cline's own keys are not a modification"
    before = settings.read_bytes()
    runner.json("install", "--global", "--client", "cline")
    assert settings.read_bytes() == before, "an unchanged install leaves Cline's rewrite alone"

    descriptor.write_text(ENV_DESCRIPTOR.replace('command = "grim"', 'command = "grim2"'))
    second = runner.json("release", str(descriptor), f"{repo}:2.0.0", "--kind", "mcp")
    retag(repo_path, "stable", second["manifest_digest"])
    rows = runner.json("update", "--global", "--client", "cline")["items"]
    assert rows[0]["action"] == "updated", rows
    entry = json.loads(settings.read_text())["mcpServers"]["grim-mcp"]
    assert entry["command"] == "grim2", "grim's fields move to the new pin"
    assert entry["autoApprove"] == ["grim_search"], "approvals survive the rewrite"
    assert entry["oauth"] == {"tokens": {"access_token": "t"}}, "tokens survive the rewrite"
    assert row()["state"] == "installed"

    runner.json("uninstall", "--global", "mcp", "grim-mcp")
    remaining = json.loads(settings.read_text()).get("mcpServers", {})
    assert "grim-mcp" not in remaining, "uninstall removes the whole entry"


def _outside_path_is_skipped(runner, grim_home: Path, ref: str, settings: Path, outside: Path) -> None:
    runner.env["CLINE_MCP_SETTINGS_PATH"] = str(settings)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    result = runner.run("install", "--global", "--client", "claude,cline", check=False)
    assert result.returncode == 0, result.stderr
    assert "skipped for cline" in result.stderr and "not anchorable" in result.stderr, result.stderr
    assert json.loads((runner.home / ".claude.json").read_text())["mcpServers"]["grim-mcp"]
    assert not outside.exists(), f"nothing written outside ~/.cline: {sorted(outside.parent.iterdir())}"
    assert not outside.with_name(outside.name + ".lock").exists(), "no lock dir outside ~/.cline"


def test_global_cline_skips_a_settings_path_outside_its_root(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    (runner.home / ".cline").mkdir()
    ref = _release(runner, tmp_path / "src", registry, unique_repo)
    outside = tmp_path / "elsewhere" / "cline_mcp_settings.json"
    outside.parent.mkdir()
    _outside_path_is_skipped(runner, grim_home, ref, outside, outside)


def test_global_cline_skips_a_settings_path_that_climbs_out_with_dotdot(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    (runner.home / ".cline").mkdir()
    ref = _release(runner, tmp_path / "src", registry, unique_repo)
    (runner.home / "escaped").mkdir()
    climbing = runner.home / ".cline" / ".." / "escaped" / "cline_mcp_settings.json"
    _outside_path_is_skipped(runner, grim_home, ref, climbing, runner.home / "escaped" / "cline_mcp_settings.json")


@pytest.mark.skipif(sys.platform == "win32", reason="stow-style symlinks are a Unix layout")
def test_global_cline_locks_beside_a_stow_linked_settings_file(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """A dotfiles-linked settings file gets the lock Cline takes: beside the
    link at `~/.cline/data/settings/`, never beside the link's target. A
    crashed holder's stale lock planted there is reclaimed on install and on
    uninstall, and no lock dir ever appears in the dotfiles tree."""
    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    dotfiles = runner.home / "dotfiles"
    dotfiles.mkdir()
    real = dotfiles / "cline_mcp_settings.json"
    real.write_text('{\n  "mcpServers": {}\n}\n')
    settings = runner.home / ".cline" / "data" / "settings" / "cline_mcp_settings.json"
    settings.parent.mkdir(parents=True)
    settings.symlink_to(real)
    lock_dir = settings.with_name(settings.name + ".lock")

    def plant_stale_lock() -> None:
        lock_dir.mkdir()
        (lock_dir / "owner.1.1.crashed").write_text("1.1.crashed")
        old = time.time() - 60
        os.utime(lock_dir, (old, old))

    ref = _release(runner, tmp_path / "src", registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    plant_stale_lock()
    runner.json("install", "--global", "--client", "cline")
    assert settings.is_symlink(), "the link survives the write"
    assert "grim-mcp" in json.loads(real.read_text())["mcpServers"]
    assert not lock_dir.exists(), "install took (and released) the lock beside the link"

    plant_stale_lock()
    runner.json("uninstall", "--global", "mcp", "grim-mcp")
    assert "grim-mcp" not in json.loads(real.read_text()).get("mcpServers", {})
    assert not lock_dir.exists(), "uninstall took the lock beside the link, not beside its target"
    assert sorted(p.name for p in dotfiles.iterdir()) == ["cline_mcp_settings.json"], "no lock in the dotfiles tree"


def test_global_cline_refuses_a_member_cline_changed_while_grim_waited(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """The untracked gate judges the file before grim holds Cline's lock. If
    Cline writes the same member while grim waits, that verdict is stale:
    grim must leave Cline's member alone and exit 75 (retry-safe, like a
    held lock), never overwrite it."""
    import shutil
    import subprocess

    from src.runner import GrimRunner

    runner = GrimRunner(grim_binary, grim_home)
    (runner.home / ".cline").mkdir(parents=True, exist_ok=True)
    ref = _release(runner, tmp_path / "src", registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    settings = runner.home / ".cline" / "data" / "settings" / "cline_mcp_settings.json"
    settings.parent.mkdir(parents=True)
    settings.write_text('{\n  "mcpServers": {}\n}\n')

    # A live Cline holder. A future mtime reads as fresh, so grim never
    # reclaims it as stale while the test drives it.
    lock_dir = settings.with_name(settings.name + ".lock")
    lock_dir.mkdir()
    (lock_dir / "owner.1.1.live").write_text("1.1.live")
    future = time.time() + 3600
    os.utime(lock_dir, (future, future))

    proc = subprocess.Popen(
        [str(runner.binary), "install", "--global", "--client", "cline"],
        env=runner.env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    # grim is waiting once it stages a lock attempt beside the held lock.
    staging = settings.name + ".lock.tmp."
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline and not any(
        p.name.startswith(staging) for p in settings.parent.iterdir()
    ):
        pass
    cline_member = {"type": "stdio", "command": "cline-wrote-this"}
    settings.write_text(json.dumps({"mcpServers": {"grim-mcp": cline_member}}, indent=2))
    shutil.rmtree(lock_dir)
    _, stderr = proc.communicate(timeout=60)

    assert proc.returncode == 75, stderr
    assert "cline_mcp_settings.json" in stderr, stderr
    assert json.loads(settings.read_text())["mcpServers"]["grim-mcp"] == cline_member
