# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim config`` over the project-only ``[plugin]`` table.

``plugin.name``, ``plugin.description``, ``plugin.version`` and
``plugin.logo`` are get/set/unset/list-able like any other key; values are
checked with the same rules ``grimoire.toml`` load applies (65 at set, 78
at load). ``[plugin]`` is outside the declaration hash, so editing it never
makes the lock stale, and every other config rewrite keeps it.
"""
from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

PLUGIN_KEYS = ["plugin.name", "plugin.description", "plugin.version", "plugin.logo"]


def _plugin_table(project_dir: Path) -> dict | None:
    return tomllib.loads((project_dir / "grimoire.toml").read_text()).get("plugin")


@pytest.mark.parametrize(
    ("key", "value", "stored"),
    [
        ("plugin.name", "team-tools", "team-tools"),
        ("plugin.description", "Team tools for everyone", "Team tools for everyone"),
        ("plugin.version", "v1.2.0", "1.2.0"),
        ("plugin.logo", "assets/team.svg", "assets/team.svg"),
    ],
)
def test_set_get_unset_round_trip(grim_at, project_dir: Path, key: str, value: str, stored: str) -> None:
    write_config(project_dir)
    runner = grim_at(project_dir)

    report = runner.json("config", "set", key, value)
    assert report["value"] == stored, report
    assert runner.plain("config", "get", key).stdout.strip() == stored
    assert _plugin_table(project_dir) == {key.removeprefix("plugin."): stored}

    runner.run("config", "unset", key)
    assert runner.plain("config", "get", key, check=False).returncode == 1
    assert _plugin_table(project_dir) is None, "an emptied [plugin] table is removed"


@pytest.mark.parametrize(
    ("key", "value", "reason"),
    [
        ("plugin.name", "Team", "invalid plugin name"),
        ("plugin.description", "x" * 501, "at most 500"),
        ("plugin.version", "1.0.0+build", "invalid version"),
        ("plugin.logo", "", "must not be empty"),
    ],
)
def test_invalid_value_exits_65_and_leaves_the_file_unchanged(
    grim_at, project_dir: Path, key: str, value: str, reason: str
) -> None:
    write_config(project_dir)
    before = (project_dir / "grimoire.toml").read_text()
    runner = grim_at(project_dir)

    result = runner.run("config", "set", key, value, check=False)

    assert result.returncode == 65, result.stderr
    assert reason in result.stderr, result.stderr
    assert (project_dir / "grimoire.toml").read_text() == before


def test_dry_run_writes_nothing(grim_at, project_dir: Path) -> None:
    write_config(project_dir)
    before = (project_dir / "grimoire.toml").read_text()
    runner = grim_at(project_dir)

    report = runner.json("config", "set", "plugin.version", "v2.0.0", "--dry-run")

    assert report["dry_run"] is True
    assert report["value"] == "2.0.0"
    assert (project_dir / "grimoire.toml").read_text() == before
    assert runner.run("config", "set", "plugin.name", "Bad", "--dry-run", check=False).returncode == 65


def test_list_all_appends_the_four_plugin_rows_with_metadata(grim_at, project_dir: Path) -> None:
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.run("config", "set", "plugin.name", "team")

    items = runner.json("config", "list", "--all")["items"]

    assert [i["key"] for i in items[-4:]] == PLUGIN_KEYS, "plugin rows come last"
    rows = {i["key"]: i for i in items}
    for key in PLUGIN_KEYS:
        assert rows[key]["type"] == "string"
        assert rows[key]["title"] and rows[key]["description"]
    assert rows["plugin.name"]["value"] == "team"
    assert rows["plugin.version"]["set"] is False
    plain = runner.json("config", "list")["items"]
    assert [i["key"] for i in plain if i["key"].startswith("plugin.")] == ["plugin.name"]


@pytest.mark.parametrize("verb", [["get"], ["set", "--dry-run"], ["unset"]])
def test_global_scope_is_refused_with_64(grim_at, grim_home: Path, project_dir: Path, verb: list[str]) -> None:
    (grim_home / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    runner = grim_at(project_dir)
    args = ["config", "--global", verb[0], "plugin.name"]
    if verb[0] == "set":
        args += ["team", *verb[1:]]

    result = runner.run(*args, check=False)

    assert result.returncode == 64, result.stderr
    assert "project-only" in result.stderr


def test_global_list_carries_no_plugin_rows(grim_at, grim_home: Path, project_dir: Path) -> None:
    (grim_home / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    items = grim_at(project_dir).json("config", "--global", "list", "--all")["items"]
    assert not [i for i in items if i["key"].startswith("plugin.")]


def test_rename_rule_is_not_a_config_key(grim_at, project_dir: Path) -> None:
    write_config(project_dir)
    result = grim_at(project_dir).run("config", "set", "plugin.rename.strip_prefix", "acme-", check=False)
    assert result.returncode == 64, result.stderr


def test_plugin_table_survives_other_writes_and_never_stales_the_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n"},
        tag="stable",
    )
    cfg = write_config(project_dir, skills={"code-review": sk.fq})
    cfg.write_text(
        cfg.read_text()
        + '\n[plugin]\nname = "team"\nversion = "v1.0.0"\n\n[plugin.rename]\nstrip_prefix = "acme-"\n'
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    lock_before = (project_dir / "grimoire.lock").read_text()

    runner.run("config", "set", "plugin.description", "Team tools")
    runner.run("config", "set", "options.show_deprecated", "true")

    assert _plugin_table(project_dir) == {
        "name": "team",
        "description": "Team tools",
        "version": "v1.0.0",
        "rename": {"strip_prefix": "acme-"},
    }, "unrelated writes keep [plugin] as authored"
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}, "the lock is not stale"
    assert (project_dir / "grimoire.lock").read_text() == lock_before
