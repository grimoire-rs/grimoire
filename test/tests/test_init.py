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


def test_init_snapshots_env_default_registry(grim_at, project_dir: Path) -> None:
    """Without ``--registry``, init snapshots ``GRIM_DEFAULT_REGISTRY``."""
    runner = grim_at(project_dir)
    runner.env["GRIM_DEFAULT_REGISTRY"] = "snap.example"
    result = runner.plain("init", check=False)
    assert result.returncode == 0, result.stderr
    body = (project_dir / "grimoire.toml").read_text()
    assert 'default_registry = "snap.example"' in body


def test_init_explicit_registry_beats_env(grim_at, project_dir: Path) -> None:
    """``--registry`` wins over ``GRIM_DEFAULT_REGISTRY`` at init time."""
    runner = grim_at(project_dir)
    runner.env["GRIM_DEFAULT_REGISTRY"] = "env.example"
    runner.run("init", "--registry", "flag.example", check=False)
    body = (project_dir / "grimoire.toml").read_text()
    assert 'default_registry = "flag.example"' in body
    assert "env.example" not in body


def test_init_without_any_registry_omits_options(grim_at, project_dir: Path) -> None:
    """No --registry, no env: the built-in fallback registry is never
    snapshotted — ``[options]`` stays absent so the default keeps floating
    with the binary."""
    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)
    runner.plain("init", check=False)
    body = (project_dir / "grimoire.toml").read_text()
    assert "[options]" not in body
    assert "default_registry" not in body


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
