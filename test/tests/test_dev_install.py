# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Dev-install: `grim install <path>` renders a local source one-off.

The config and lock stay byte-untouched; the record is dev-marked so
status lists it, update refreshes it, prune spares it, uninstall removes
it. Fully offline.
"""
from __future__ import annotations

from pathlib import Path

import pytest


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _project(project_dir: Path) -> Path:
    (project_dir / ".claude").mkdir()
    _write(project_dir / "grimoire.toml", "[skills]\n")
    d = project_dir / "dev-skill"
    _write(
        d / "SKILL.md",
        "---\nname: dev-skill\ndescription: Dev.\n---\n# Dev v1\n",
    )
    return d


def _offline(runner):
    runner.env["GRIM_OFFLINE"] = "1"
    return runner


def test_dev_install_leaves_config_and_lock_untouched(
    grim_at, project_dir: Path
) -> None:
    _project(project_dir)
    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    cfg_before = (project_dir / "grimoire.toml").read_bytes()
    lock_before = (project_dir / "grimoire.lock").read_bytes()

    runner.run("install", "./dev-skill", "--client", "claude")

    assert (project_dir / "grimoire.toml").read_bytes() == cfg_before
    assert (project_dir / "grimoire.lock").read_bytes() == lock_before
    rendered = project_dir / ".claude" / "skills" / "dev-skill" / "SKILL.md"
    assert rendered.is_file()

    state = (project_dir / ".grimoire" / "state.json").read_text()
    assert '"dev"' in state and "true" in state


def test_dev_install_visible_in_status(grim_at, project_dir: Path) -> None:
    _project(project_dir)
    runner = _offline(grim_at(project_dir))
    runner.run("install", "./dev-skill", "--client", "claude")

    items = runner.json("status")["items"]
    dev = [e for e in items if e["name"] == "dev-skill"]
    assert dev, f"dev-install must appear in status: {items}"
    assert dev[0]["source"] == "path: ./dev-skill (dev)"
    assert dev[0]["state"] == "installed"


def test_update_refreshes_drifted_dev_install(
    grim_at, project_dir: Path
) -> None:
    d = _project(project_dir)
    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "./dev-skill", "--client", "claude")

    _write(
        d / "SKILL.md",
        "---\nname: dev-skill\ndescription: Dev.\n---\n# Dev v2\n",
    )
    assert (
        runner.json("status")["items"][-1]["state"] == "outdated"
    ), "source drift must flag the dev record outdated"

    runner.run("update", "--client", "claude")
    rendered = project_dir / ".claude" / "skills" / "dev-skill" / "SKILL.md"
    assert "# Dev v2" in rendered.read_text()
    # The record survives the update (never pruned) and stays dev-marked.
    items = runner.json("status")["items"]
    dev = [e for e in items if e["name"] == "dev-skill"]
    assert dev and dev[0]["source"].endswith("(dev)")
    assert dev[0]["state"] == "installed"


def test_uninstall_removes_dev_install(grim_at, project_dir: Path) -> None:
    _project(project_dir)
    runner = _offline(grim_at(project_dir))
    runner.run("install", "./dev-skill", "--client", "claude")
    runner.run("uninstall", "skill", "dev-skill")
    assert not (project_dir / ".claude" / "skills" / "dev-skill").exists()
    items = runner.json("status")["items"]
    assert not [e for e in items if e["name"] == "dev-skill"]


def test_global_dev_install_renders_to_native_home(
    grim_at, grim_home: Path, tmp_path: Path
) -> None:
    # Global dev-install writes into every vendor's native user-level dir
    # (isolated $HOME, nothing detected → all clients), records dev state
    # in global.json, and the lifecycle (status, uninstall) works there.
    src = tmp_path / "dev-skill"
    _write(
        src / "SKILL.md",
        "---\nname: dev-skill\ndescription: Dev.\n---\n# Global dev\n",
    )
    runner = _offline(grim_at(tmp_path))
    runner.run("--global", "install", str(src))

    outputs = (
        runner.home / ".claude" / "skills" / "dev-skill" / "SKILL.md",
        runner.home / ".config" / "opencode" / "skills" / "dev-skill" / "SKILL.md",
        runner.home / ".copilot" / "skills" / "dev-skill" / "SKILL.md",
    )
    for out in outputs:
        assert out.is_file(), f"missing vendor output: {out}"
        assert "# Global dev" in out.read_text()

    state = (grim_home / "state" / "global.json").read_text()
    assert '"dev"' in state

    items = runner.json("--global", "status")["items"]
    dev = [e for e in items if e["name"] == "dev-skill"]
    assert dev and dev[0]["source"].endswith("(dev)")

    runner.run("--global", "uninstall", "skill", "dev-skill")
    for out in outputs:
        assert not out.exists(), f"vendor output must be removed: {out}"


@pytest.mark.parametrize(
    ("client", "env_var", "default_root"),
    [
        ("claude", "CLAUDE_CONFIG_DIR", ".claude"),
        ("copilot", "COPILOT_HOME", ".copilot"),
        ("opencode", "OPENCODE_CONFIG_DIR", ".config/opencode"),
    ],
)
def test_global_dev_install_honors_vendor_env_override(
    grim_at, tmp_path: Path, client: str, env_var: str, default_root: str
) -> None:
    # Each vendor's env override replaces its native global root; the
    # default $HOME location must stay untouched when the override is set.
    src = tmp_path / "dev-skill"
    _write(
        src / "SKILL.md",
        "---\nname: dev-skill\ndescription: Dev.\n---\n# Override\n",
    )
    override = tmp_path / f"{client}-config"
    runner = _offline(grim_at(tmp_path))
    runner.env[env_var] = str(override)
    runner.run("--global", "install", str(src), "--client", client)

    assert (override / "skills" / "dev-skill" / "SKILL.md").is_file()
    assert not (
        runner.home / default_root / "skills" / "dev-skill"
    ).exists(), f"default {default_root} must stay untouched with {env_var} set"


def test_bare_word_positional_is_64(grim_at, project_dir: Path) -> None:
    _project(project_dir)
    runner = _offline(grim_at(project_dir))
    result = runner.run("install", "bogus", check=False)
    assert result.returncode == 64, result.stderr
    assert "grim add" in result.stderr
