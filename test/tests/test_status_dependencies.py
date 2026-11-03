# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim status` dependency fields: `direct`, `required_by`, `source`, the note.

The locks are hand-written, so none of this needs a registry: `status`
without `--check` reads the config and the lock alone.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import write_config

HOST = "ghcr.io/acme"
DECLARATION_HASH = "sha256:" + "d" * 64


def _digest(byte: str) -> str:
    return "sha256:" + byte * 64


def _entry(table: str, name: str, repo: str, byte: str, *, requires=(), bundle=None) -> str:
    out = f'[[{table}]]\nname = "{name}"\npinned = "{HOST}/{repo}@{_digest(byte)}"\n'
    if bundle:
        out += f'bundle = "{HOST}/{bundle[0]}"\nbundle_tag = "{bundle[1]}"\n'
    for kind, dep, ident in requires:
        out += f'\n[[{table}.requires]]\nkind = "{kind}"\nname = "{dep}"\nid = "{ident}"\n'
    return out + "\n"


def _bundle(name: str, repo: str, tag: str, byte: str, members) -> str:
    out = (
        f'[[bundle]]\nname = "{name}"\nrepo = "{HOST}/{repo}"\ntag = "{tag}"\n'
        f'pinned = "{HOST}/{repo}@{_digest(byte)}"\n'
    )
    for kind, member, ident in members:
        out += f'\n[[bundle.member]]\nkind = "{kind}"\nname = "{member}"\nid = "{ident}"\n'
    return out + "\n"


def _write_lock(project_dir: Path, *sections: str) -> None:
    head = (
        "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n"
        f'declaration_hash = "{DECLARATION_HASH}"\n'
        'generated_by = "grim 0.1.0"\ngenerated_at = "2026-04-19T00:00:00Z"\n\n'
    )
    (project_dir / "grimoire.lock").write_text(head + "".join(sections))


def _rows(runner) -> dict[str, dict]:
    return {f"{i['kind']}/{i['name']}": i for i in runner.json("status")["items"]}


def _skill_ref(name: str) -> str:
    return f"{HOST}/skills/{name}:1"


def test_required_by_fields(grim_at, project_dir: Path) -> None:
    write_config(
        project_dir,
        skills={"a": _skill_ref("a"), "b": _skill_ref("b")},
        bundles={"stack": f"{HOST}/bundles/stack:1"},
    )
    shared = ("skill", "shared", _skill_ref("shared"))
    _write_lock(
        project_dir,
        _entry("skill", "a", "skills/a", "a", requires=[shared]),
        _entry("skill", "b", "skills/b", "b", requires=[shared]),
        _entry("skill", "shared", "skills/shared", "c"),
        _entry(
            "skill", "m", "skills/m", "e", bundle=("bundles/stack", "1")
        ),
        _bundle("stack", "bundles/stack", "1", "f", [("skill", "m", _skill_ref("m"))]),
    )

    rows = _rows(grim_at(project_dir))

    assert rows["skill/a"]["direct"] is True
    assert rows["skill/a"]["required_by"] == []
    assert rows["bundle/stack"]["direct"] is True
    assert rows["bundle/stack"]["required_by"] == []
    assert rows["skill/shared"]["direct"] is False
    assert rows["skill/shared"]["required_by"] == [
        {"kind": "skill", "name": "a"},
        {"kind": "skill", "name": "b"},
    ]
    # Bundles are parents like any other, named with kind "bundle".
    assert rows["skill/m"]["direct"] is False
    assert rows["skill/m"]["required_by"] == [{"kind": "bundle", "name": "stack"}]


def test_required_by_null_when_incomputable(grim_at, project_dir: Path) -> None:
    write_config(project_dir, skills={"a": _skill_ref("a")})
    # `a` names an edge whose target has no entry: the closure cannot be
    # followed offline, so the graph is incomputable.
    _write_lock(
        project_dir,
        _entry(
            "skill",
            "a",
            "skills/a",
            "a",
            requires=[("skill", "gone", _skill_ref("gone"))],
        ),
        _entry("skill", "orphan", "skills/orphan", "b"),
    )
    runner = grim_at(project_dir)

    rows = _rows(runner)
    assert rows["skill/a"]["direct"] is True
    assert rows["skill/a"]["required_by"] is None
    assert rows["skill/orphan"]["direct"] is False
    assert rows["skill/orphan"]["required_by"] is None

    # No lock at all is incomputable too: every field is still present.
    (project_dir / "grimoire.lock").unlink()
    only = _rows(runner)["skill/a"]
    assert only["required_by"] is None
    assert only["direct"] is True


def test_source_literals(grim_at, project_dir: Path, tmp_path: Path) -> None:
    write_config(
        project_dir,
        skills={"a": _skill_ref("a"), "b": _skill_ref("b")},
        bundles={"stack": f"{HOST}/bundles/stack:1"},
    )
    shared = ("skill", "shared", _skill_ref("shared"))
    _write_lock(
        project_dir,
        _entry("skill", "a", "skills/a", "a", requires=[shared]),
        _entry("skill", "b", "skills/b", "b", requires=[shared]),
        _entry("skill", "shared", "skills/shared", "c"),
        _entry("skill", "m", "skills/m", "e", bundle=("bundles/stack", "1")),
        _bundle("stack", "bundles/stack", "1", "f", [("skill", "m", _skill_ref("m"))]),
    )
    rows = _rows(grim_at(project_dir))
    assert rows["skill/a"]["source"] == "direct"
    assert rows["bundle/stack"]["source"] == "direct"
    assert rows["skill/m"]["source"] == f"bundle: {HOST}/bundles/stack"
    assert rows["skill/shared"]["source"] == "dependency: skill/a, skill/b"

    # An incomputable graph cannot name the parents: bare `dependency`.
    _write_lock(
        project_dir,
        _entry("skill", "a", "skills/a", "a", requires=[shared]),
        _entry("skill", "orphan", "skills/orphan", "b"),
    )
    assert _rows(grim_at(project_dir))["skill/orphan"]["source"] == "dependency"

    # A path declaration keeps its literal.
    local = tmp_path / "local-project"
    (local / ".claude").mkdir(parents=True)
    skill = local / "skills" / "loc"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text("---\nname: loc\ndescription: Demo.\n---\nbody\n")
    (local / "grimoire.toml").write_text('[skills]\nloc = "./skills/loc"\n')
    runner = grim_at(local)
    runner.env["GRIM_OFFLINE"] = "1"
    runner.run("lock")
    assert _rows(runner)["skill/loc"]["source"] == "path: ./skills/loc"


def test_redundant_direct_note(grim_at, project_dir: Path) -> None:
    write_config(
        project_dir, skills={"x": _skill_ref("x"), "y": _skill_ref("y")}
    )
    # `y` requires `x` at the very reference the declaration already pins.
    _write_lock(
        project_dir,
        _entry("skill", "x", "skills/x", "a"),
        _entry(
            "skill", "y", "skills/y", "b", requires=[("skill", "x", _skill_ref("x"))]
        ),
    )
    runner = grim_at(project_dir)

    plain = runner.plain("status")
    note = (
        "note: skill/x is declared directly and also required by skill/y "
        "at the same digest; the declaration can be removed"
    )
    assert note in plain.stderr
    assert note not in plain.stdout

    # JSON carries no note, and nothing on stderr.
    as_json = runner.run("--format", "json", "status")
    assert "note:" not in as_json.stdout
    assert "note:" not in as_json.stderr

    # A project without dependencies prints no note.
    write_config(project_dir, skills={"x": _skill_ref("x")})
    _write_lock(project_dir, _entry("skill", "x", "skills/x", "a"))
    assert "note:" not in runner.plain("status").stderr
