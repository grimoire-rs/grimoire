# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim publish` orders a batch by the `requires` its entries declare.

An entry that requires another entry of the same manifest is published after
it; with no such edge the fixed kind order is kept. `pin = true` is accepted on
every kind and freezes the entry's `requires`. A cycle refuses the whole batch
(65) before anything is pushed.
"""
from __future__ import annotations

import json
from pathlib import Path
from urllib.error import HTTPError

import pytest

from src.registry import fetch_blob, fetch_manifest, tag_digest

VERSION = "0.1.0"


def _write(p: Path, body: str) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body, encoding="utf-8", newline="\n")
    return p


def _skill(root: Path, name: str, requires: dict[str, str] | None = None) -> None:
    meta = ""
    if requires:
        meta = "metadata:\n" + "".join(
            f'  grim.requires.{key}: "{ref}"\n' for key, ref in requires.items()
        )
    _write(
        root / "skills" / name / "SKILL.md",
        f"---\nname: {name}\ndescription: Demo {name}.\n{meta}---\n# {name}\n",
    )


def _rule(root: Path, name: str) -> None:
    _write(root / f"rules/{name}.md", "---\npaths: ['**/*.md']\n---\n# rule\n")


def _mcp(root: Path, name: str) -> None:
    _write(
        root / f"mcp/{name}.toml",
        'description = "d"\n[server]\ntransport = "stdio"\ncommand = "grim"\n',
    )


def _manifest(root: Path, registry: str, unique_repo: str, entries: str) -> Path:
    # One repository per entry name, `<unique>/p/<name>`, so a relative `./b:1`
    # between two entries names the sibling.
    return _write(
        root / "publish.toml",
        f'registry = "{registry}"\nrepository_prefix = "{unique_repo}/p"\n\n{entries}',
    )


def _names(rows: list[dict]) -> list[str]:
    return [r["ref"].rsplit("/", 1)[1].split(":")[0] for r in rows]


def _entry(table: str, name: str, extra: str = "") -> str:
    return f'[{table}.{name}]\nversion = "{VERSION}"\n{extra}\n'


def _assert_absent(repo_path: str) -> None:
    with pytest.raises(HTTPError) as exc:
        tag_digest(repo_path, VERSION)
    assert exc.value.code == 404


def test_requires_order(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Positive control: with no edge, alphabetical order within the skill kind.
    _skill(project_dir, "a")
    _skill(project_dir, "b")
    plain = _manifest(
        project_dir, registry, unique_repo + "/control", _entry("skills", "a") + _entry("skills", "b")
    )
    runner = grim_at(project_dir)
    assert _names(runner.json("publish", "--manifest", str(plain))["items"]) == ["a", "b"]

    # a requires b: b is released first.
    _skill(project_dir, "a", {"skills.b": "./b:0.1.0"})
    edge = _manifest(
        project_dir, registry, unique_repo, _entry("skills", "a") + _entry("skills", "b")
    )
    assert _names(runner.json("publish", "--manifest", str(edge))["items"]) == ["b", "a"]


def test_requires_order_keeps_kind_order_as_tie_break(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Fixed order: skills, rules, agents, mcp, hooks, bundles. Skill `a` waits
    # for mcp `m`; everything else keeps its place around it.
    _skill(project_dir, "a", {"mcp.m": "./m:0.1.0"})
    _skill(project_dir, "c")
    _rule(project_dir, "r")
    _mcp(project_dir, "m")
    manifest = _manifest(
        project_dir,
        registry,
        unique_repo,
        _entry("skills", "a") + _entry("skills", "c") + _entry("rules", "r") + _entry("mcp", "m"),
    )
    rows = grim_at(project_dir).json("publish", "--manifest", str(manifest))["items"]
    assert _names(rows) == ["c", "r", "m", "a"]


def test_pin_accepted_on_skill(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _skill(project_dir, "a", {"skills.b": "./b:0.1.0"})
    _skill(project_dir, "b")
    manifest = _manifest(
        project_dir,
        registry,
        unique_repo,
        _entry("skills", "a", "pin = true") + _entry("skills", "b"),
    )
    rows = grim_at(project_dir).json("publish", "--manifest", str(manifest))["items"]
    by_name = dict(zip(_names(rows), rows))

    # b went first, so a's edge froze to the digest b was just pushed under.
    assert _names(rows) == ["b", "a"]
    requires = fetch_manifest(f"{unique_repo}/p/a", VERSION)["layers"][1]
    members = json.loads(fetch_blob(f"{unique_repo}/p/a", requires["digest"]))["members"]
    assert members[0]["id"].endswith(f"@{by_name['b']['digest']}")


def test_pin_noop_without_requires(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _skill(project_dir, "a")
    runner = grim_at(project_dir)
    plain = _manifest(project_dir, registry, unique_repo, _entry("skills", "a"))
    first = runner.json("publish", "--manifest", str(plain))["items"][0]

    pinned = _manifest(
        project_dir, registry, unique_repo, _entry("skills", "a", "pin = true")
    )
    again = runner.json("publish", "--manifest", str(pinned), "--force")["items"][0]
    assert again["status"] == "pushed"
    assert again["digest"] == first["digest"]
    assert len(fetch_manifest(f"{unique_repo}/p/a", VERSION)["layers"]) == 1


def test_batch_cycle_refused(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _skill(project_dir, "a", {"skills.b": "./b:0.1.0"})
    _skill(project_dir, "b", {"skills.a": "./a:0.1.0"})
    _skill(project_dir, "free")
    manifest = _manifest(
        project_dir,
        registry,
        unique_repo,
        _entry("skills", "a") + _entry("skills", "b") + _entry("skills", "free"),
    )
    result = grim_at(project_dir).run("publish", "--manifest", str(manifest), check=False)
    assert result.returncode == 65, result.stderr
    assert "cycle" in result.stderr
    # Nothing was pushed, not even the entry outside the cycle.
    for name in ("a", "b", "free"):
        _assert_absent(f"{unique_repo}/p/{name}")
