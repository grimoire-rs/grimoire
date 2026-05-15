# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim init` acceptance tests."""
from __future__ import annotations

from pathlib import Path

from src.runner import GrimRunner


def test_project_init_creates_config(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    result = runner.plain("init", check=False)
    assert result.returncode == 0, result.stderr
    cfg = project_dir / "grimoire.toml"
    assert cfg.is_file()
    body = cfg.read_text()
    assert "[skills]" in body
    assert "[rules]" in body


def test_init_with_registry_seeds_options(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    runner.run("init", "--registry", "ghcr.io/acme", check=False)
    body = (project_dir / "grimoire.toml").read_text()
    assert "[options]" in body
    assert 'default_registry = "ghcr.io/acme"' in body


def test_init_refuses_existing_config_exit_64(
    grim_at, project_dir: Path
) -> None:
    runner = grim_at(project_dir)
    runner.run("init", check=False)
    second = runner.run("init", check=False)
    assert second.returncode == 64, (
        f"re-init must be EX_USAGE 64, got {second.returncode}; "
        f"{second.stderr}"
    )


def test_init_json_shape(grim_at, project_dir: Path) -> None:
    runner = grim_at(project_dir)
    result = runner.run("--format", "json", "init", check=False)
    assert result.returncode == 0
    import json

    obj = json.loads(result.stdout)
    assert obj["scope"] == "project"
    assert obj["status"] == "created"
    assert obj["path"].endswith("grimoire.toml")


def test_global_init_uses_grim_home(
    grim_binary: Path, grim_home: Path
) -> None:
    runner = GrimRunner(grim_binary, grim_home)
    result = runner.run("--format", "json", "init", "--global", check=False)
    assert result.returncode == 0
    import json

    obj = json.loads(result.stdout)
    assert obj["scope"] == "global"
    assert (grim_home / "grimoire.toml").is_file()
