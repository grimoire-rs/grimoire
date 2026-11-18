# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim config registry add` / `set` input checks that the shared write path must keep.

Pins the usage and value errors raised before the config is touched, and the
one-default invariant on add, so the validation can be shared with other
front ends without moving an exit code.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import write_config
from src.runner import GrimRunner


def _runner(grim_at: object, project_dir: Path) -> GrimRunner:
    write_config(project_dir)
    return grim_at(project_dir)  # type: ignore[call-arg,return-value]


def test_registry_add_with_neither_locator_exits_64(grim_at: object, project_dir: Path) -> None:
    runner = _runner(grim_at, project_dir)
    r = runner.run("config", "registry", "add", "acme", check=False)
    assert r.returncode == 64, f"rc={r.returncode} stderr={r.stderr!r}"
    assert "acme" not in (project_dir / "grimoire.toml").read_text(encoding="utf-8")


def test_registry_add_with_both_locators_exits_64(grim_at: object, project_dir: Path) -> None:
    runner = _runner(grim_at, project_dir)
    r = runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme", "--index", "https://index.grimoire.rs",
        check=False,
    )
    assert r.returncode == 64, f"rc={r.returncode} stderr={r.stderr!r}"


def test_registry_add_control_character_in_locator_exits_65(grim_at: object, project_dir: Path) -> None:
    runner = _runner(grim_at, project_dir)
    r = runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/ac\x1bme", check=False)
    assert r.returncode == 65, f"rc={r.returncode} stderr={r.stderr!r}"
    assert "acme" not in (project_dir / "grimoire.toml").read_text(encoding="utf-8")


def test_registry_add_unclassifiable_index_locator_exits_65(grim_at: object, project_dir: Path) -> None:
    runner = _runner(grim_at, project_dir)
    r = runner.run("config", "registry", "add", "hub", "--index", "not a locator", check=False)
    assert r.returncode == 65, f"rc={r.returncode} stderr={r.stderr!r}"
    assert "invalid index locator" in r.stderr


def test_registry_set_unclassifiable_index_locator_exits_65(grim_at: object, project_dir: Path) -> None:
    runner = _runner(grim_at, project_dir)
    runner.run("config", "registry", "add", "hub", "--oci", "ghcr.io/acme")
    r = runner.run("config", "registry", "set", "hub", "--index", "not a locator", check=False)
    assert r.returncode == 65, f"rc={r.returncode} stderr={r.stderr!r}"
    assert "invalid index locator" in r.stderr


def test_registry_add_default_clears_the_previous_default(grim_at: object, project_dir: Path) -> None:
    runner = _runner(grim_at, project_dir)
    runner.run("config", "registry", "add", "first", "--oci", "ghcr.io/first", "--default")
    runner.run("config", "registry", "add", "second", "--oci", "ghcr.io/second", "--default")
    items = runner.json("config", "registry", "list")["items"]
    defaults = [r["alias"] for r in items if r.get("default") is True]
    assert defaults == ["second"], f"defaults={defaults}"
