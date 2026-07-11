# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim context` acceptance tests — read-only resolution introspection."""
from __future__ import annotations

from pathlib import Path

from src.helpers import write_config


def _project(project_dir: Path, registry: str) -> None:
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "test"\noci = "{registry}"\ndefault = true\n\n'
        "[skills]\n\n[rules]\n"
    )
    (project_dir / ".claude").mkdir()


def test_context_reports_scope_paths_clients_registries(
    grim_at, project_dir: Path, registry: str
) -> None:
    _project(project_dir, registry)
    runner = grim_at(project_dir)

    doc = runner.json("context")
    assert doc["scope"] == "project"
    assert doc["workspace"] == str(project_dir)
    assert doc["config_path"] == str(project_dir / "grimoire.toml")
    assert doc["config_exists"] is True
    assert doc["lock_path"] == str(project_dir / "grimoire.lock")
    assert doc["lock_exists"] is False
    assert doc["state_path"] == str(project_dir / ".grimoire" / "state.json")
    assert doc["grim_home"], "grim_home always resolves"
    assert doc["version"], "version always present"
    assert doc["offline"] is False
    assert "offline_source" in doc and doc["offline_source"] is None
    # .claude/ marker present ⇒ claude detected as an effective client.
    assert "claude" in doc["clients"], doc["clients"]
    regs = doc["registries"]
    assert any(
        r["alias"] == "test" and r["url"] == registry and r["default"] and r["kind"] == "registry"
        for r in regs
    ), regs
    # `authenticated` is always present as a bool; the isolated HOME has no
    # docker config, so nothing is authenticated.
    for r in regs:
        assert isinstance(r["authenticated"], bool)
        assert r["authenticated"] is False, r
    assert doc["default_registry"] == registry


def test_context_registry_authenticated_from_docker_config(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    """A stored credential for the registry's host flips `authenticated`.

    Offline and hermetic: `grim context` never dials the registry, and the
    credential presence is derived from a temp `$DOCKER_CONFIG` file — no
    real keychain or network involved.
    """
    _project(project_dir, "private.example.com/team")
    docker_dir = tmp_path / "docker"
    docker_dir.mkdir()
    # An `auths` entry for the host (namespace path stripped by the probe).
    (docker_dir / "config.json").write_text(
        '{"auths": {"private.example.com": {"auth": "dXNlcjpwYXNz"}}}'
    )

    runner = grim_at(project_dir)
    runner.env["DOCKER_CONFIG"] = str(docker_dir)
    doc = runner.json("context")

    reg = next(r for r in doc["registries"] if r["url"] == "private.example.com/team")
    assert reg["authenticated"] is True, doc["registries"]


def test_context_global_scope(grim_at, project_dir: Path, grim_home: Path) -> None:
    runner = grim_at(project_dir)
    doc = runner.json("context", "--global")
    assert doc["scope"] == "global"
    assert doc["workspace"] == str(grim_home)
    assert doc["config_path"] == str(grim_home / "grimoire.toml")


def test_context_offline_flag_flips_source(grim_at, project_dir: Path, registry: str) -> None:
    _project(project_dir, registry)
    runner = grim_at(project_dir)
    doc = runner.json("context", "--offline")
    assert doc["offline"] is True
    assert doc["offline_source"] == "flag"


def test_context_outside_project_exits_79(grim_at, tmp_path: Path) -> None:
    outside = tmp_path / "empty"
    outside.mkdir()
    runner = grim_at(outside)
    result = runner.plain("context", check=False)
    assert result.returncode == 79, result.stderr


def test_context_plain_is_key_value_table(grim_at, project_dir: Path, registry: str) -> None:
    _project(project_dir, registry)
    runner = grim_at(project_dir)
    result = runner.plain("context")
    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert lines[0].startswith("Key"), lines[0]
    assert any("scope" in ln and "project" in ln for ln in lines), result.stdout
