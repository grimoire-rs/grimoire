# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim why <kind> <name>`: every chain from a declaration to one artifact.

The locks are hand-written, so none of this needs a registry: `why` reads the
config and the lock alone and never contacts a registry.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from src.helpers import write_config

HOST = "ghcr.io/acme"
DECLARATION_HASH = "sha256:" + "d" * 64
HELP = (
    "Show why an artifact is installed: every chain from a declaration in "
    "grimoire.toml to it, root first. Reads the lock; never contacts a registry."
)


def _digest(byte: str) -> str:
    return "sha256:" + byte * 64


def _ref(name: str, tag: str = "1") -> str:
    return f"{HOST}/skills/{name}:{tag}"


def _entry(name: str, byte: str, *, requires=()) -> str:
    out = f'[[skill]]\nname = "{name}"\npinned = "{HOST}/skills/{name}@{_digest(byte)}"\n'
    for dep, ident in requires:
        out += f'\n[[skill.requires]]\nkind = "skill"\nname = "{dep}"\nid = "{ident}"\n'
    return out + "\n"


def _bundle(name: str, byte: str, members) -> str:
    out = (
        f'[[bundle]]\nname = "{name}"\nrepo = "{HOST}/bundles/{name}"\ntag = "1"\n'
        f'pinned = "{HOST}/bundles/{name}@{_digest(byte)}"\n'
    )
    for member, ident in members:
        out += f'\n[[bundle.member]]\nkind = "skill"\nname = "{member}"\nid = "{ident}"\n'
    return out + "\n"


def _write_lock(project_dir: Path, *sections: str) -> None:
    head = (
        "[metadata]\nlock_version = 1\ndeclaration_hash_version = 1\n"
        f'declaration_hash = "{DECLARATION_HASH}"\n'
        'generated_by = "grim 0.1.0"\ngenerated_at = "2026-04-19T00:00:00Z"\n\n'
    )
    (project_dir / "grimoire.lock").write_text(head + "".join(sections))


def _why(runner, kind: str, name: str) -> dict:
    return runner.json("why", kind, name)


def _diamond(project_dir: Path) -> None:
    """`a` and `b` both require `shared`; `stack` brings in `m`."""
    write_config(
        project_dir,
        skills={"a": _ref("a"), "b": _ref("b")},
        bundles={"stack": f"{HOST}/bundles/stack:1"},
    )
    shared = ("shared", _ref("shared"))
    _write_lock(
        project_dir,
        _entry("a", "a", requires=[shared]),
        _entry("b", "b", requires=[shared]),
        _entry("shared", "c"),
        _entry("m", "e"),
        _bundle("stack", "f", [("m", _ref("m"))]),
    )


def test_paths_root_first_sorted(grim_at, project_dir: Path) -> None:
    _diamond(project_dir)
    runner = grim_at(project_dir)

    doc = _why(runner, "skill", "shared")

    names = [[(n["kind"], n["name"]) for n in path] for path in doc["paths"]]
    assert names == [
        [("skill", "a"), ("skill", "shared")],
        [("skill", "b"), ("skill", "shared")],
    ]
    assert doc["direct"] is False
    assert doc["path_count"] == 2
    assert doc["truncated"] is False
    assert doc["pinned"] == f"{HOST}/skills/shared@{_digest('c')}"
    assert doc["paths"][0][0]["pinned"] == f"{HOST}/skills/a@{_digest('a')}"

    # A bundle is the root of the chain that reaches its member.
    via_bundle = _why(runner, "skill", "m")
    assert [(n["kind"], n["name"]) for n in via_bundle["paths"][0]] == [
        ("bundle", "stack"),
        ("skill", "m"),
    ]


def test_direct_target_includes_itself(grim_at, project_dir: Path) -> None:
    _diamond(project_dir)

    doc = _why(grim_at(project_dir), "skill", "a")

    assert doc["direct"] is True
    assert [[n["name"] for n in p] for p in doc["paths"]] == [["a"]]
    assert doc["path_count"] == 1


def test_pinned_null_for_path_and_local_bundle(grim_at, project_dir: Path) -> None:
    skill = project_dir / "skills" / "loc"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text("---\nname: loc\ndescription: Demo.\n---\nbody\n")
    (project_dir / "grimoire.toml").write_text('[skills]\nloc = "./skills/loc"\n')
    runner = grim_at(project_dir)
    runner.env["GRIM_OFFLINE"] = "1"
    runner.run("lock")

    doc = _why(runner, "skill", "loc")
    assert doc["pinned"] is None
    assert doc["direct"] is True
    assert all(n["pinned"] is None for p in doc["paths"] for n in p)

    # A local (path-declared) bundle: `grim lock` cannot build one offline, so
    # the lock is written by hand. The graph must still be computable.
    (project_dir / "stack.toml").write_text("# members live in the lock\n")
    (project_dir / "grimoire.toml").write_text(
        '[skills]\nloc = "./skills/loc"\n\n[bundles]\nstack = "./stack.toml"\n'
    )
    _write_lock(
        project_dir,
        _entry("m", "e"),
        '[[bundle]]\nname = "stack"\npath = "./stack.toml"\n'
        f'hash = "{_digest("f")}"\n\n'
        f'[[bundle.member]]\nkind = "skill"\nname = "m"\nid = "{_ref("m")}"\n',
    )

    bundle = _why(runner, "bundle", "stack")
    assert bundle["pinned"] is None
    assert bundle["direct"] is True

    member = _why(runner, "skill", "m")
    assert member["direct"] is False
    assert [[n["pinned"] for n in p] for p in member["paths"]] == [
        [None, f"{HOST}/skills/m@{_digest('e')}"]
    ]


def test_overrides(grim_at, project_dir: Path) -> None:
    write_config(
        project_dir,
        skills={"x": _ref("x", "2"), "y": _ref("y")},
    )
    # `y` asked for `x` at tag 1, the declaration pins tag 2: the declaration
    # wins and the request is reported.
    _write_lock(
        project_dir,
        _entry("x", "a"),
        _entry("y", "b", requires=[("x", _ref("x", "1"))]),
    )
    runner = grim_at(project_dir)

    doc = _why(runner, "skill", "x")

    assert doc["overrides"] == [
        {"kind": "skill", "name": "y", "id": _ref("x", "1")}
    ]
    plain = runner.plain("why", "skill", "x")
    assert f"overrides: skill/y asked for {_ref('x', '1')}" in plain.stdout
    # An override is not redundant: no removal hint.
    assert "can be removed" not in plain.stderr

    # Same request as the declaration: no override, a removal hint instead.
    _write_lock(
        project_dir,
        _entry("x", "a"),
        _entry("y", "b", requires=[("x", _ref("x", "2"))]),
    )
    same = _why(runner, "skill", "x")
    assert same["overrides"] == []
    assert "can be removed" in runner.plain("why", "skill", "x").stderr


def _fan_in(project_dir: Path, parents: int) -> None:
    names = [f"p{i:03d}" for i in range(parents)]
    write_config(project_dir, skills={n: _ref(n) for n in names})
    target = ("t", _ref("t"))
    _write_lock(
        project_dir,
        *[_entry(n, "a", requires=[target]) for n in names],
        _entry("t", "b"),
    )


def test_truncated_and_path_count(grim_at, project_dir: Path) -> None:
    _fan_in(project_dir, 257)

    doc = _why(grim_at(project_dir), "skill", "t")

    assert len(doc["paths"]) == 256
    assert doc["path_count"] == 257
    assert doc["truncated"] is True


def test_plain_truncates_at_ten(grim_at, project_dir: Path) -> None:
    _fan_in(project_dir, 257)

    plain = grim_at(project_dir).plain("why", "skill", "t")

    chains = [ln for ln in plain.stdout.splitlines() if "-> skill/t" in ln]
    assert len(chains) == 10
    assert (
        "… and 247 more; grim why --format json lists up to 256" in plain.stdout
    )


def _expect(
    runner, code: int, reason: str | None, *args: str, message: str | None = None
) -> None:
    result = runner.run("--format", "json", "why", *args, check=False)
    assert result.returncode == code, (result.stdout, result.stderr)
    if reason is not None:
        assert json.loads(result.stdout)["error"]["reason"] == reason
    if message is not None:
        assert message in json.loads(result.stdout)["error"]["message"]


def test_exit_codes(grim_at, project_dir: Path, tmp_path: Path) -> None:
    # No project at all.
    empty = tmp_path / "empty"
    empty.mkdir()
    _expect(grim_at(empty), 79, None, "skill", "x")

    # A project without a lock.
    write_config(project_dir, skills={"a": _ref("a")})
    runner = grim_at(project_dir)
    _expect(runner, 79, "no-lock", "skill", "a")

    # Target outside the effective set.
    _write_lock(project_dir, _entry("a", "a"))
    _expect(runner, 79, None, "skill", "ghost")
    _expect(runner, 0, None, "skill", "a")

    # Unknown kind is a usage error.
    _expect(runner, 64, None, "widget", "a")

    # A closure the lock cannot follow offline.
    _write_lock(
        project_dir, _entry("a", "a", requires=[("gone", _ref("gone"))])
    )
    _expect(
        runner,
        65,
        "stale-lock",
        "skill",
        "a",
        message="dependency graph cannot be computed offline; run `grim lock`",
    )

    # A cycle in the lock.
    write_config(project_dir, skills={"a": _ref("a"), "b": _ref("b")})
    _write_lock(
        project_dir,
        _entry("a", "a", requires=[("b", _ref("b"))]),
        _entry("b", "b", requires=[("a", _ref("a"))]),
    )
    _expect(
        runner,
        65,
        "stale-lock",
        "skill",
        "a",
        message="dependency graph has a cycle: skill/a -> skill/b -> skill/a; run `grim lock`",
    )


def test_registry_flag_inert(grim_at, project_dir: Path) -> None:
    _diamond(project_dir)
    runner = grim_at(project_dir)

    plain = runner.run("why", "skill", "shared")
    flagged = runner.run("--registry", "does-not-exist.invalid", "why", "skill", "shared")

    assert flagged.returncode == 0
    assert flagged.stdout == plain.stdout


@pytest.mark.parametrize("flag", ["--help"])
def test_help_text(grim: "GrimRunner", flag: str) -> None:  # noqa: F821
    out = grim.run("why", flag).stdout
    assert HELP in " ".join(out.split())
