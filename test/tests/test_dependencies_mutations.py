# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim remove` / `grim uninstall` over a lock that carries dependency edges.

Both commands act on the difference between the effective set before and after
the edit: what only the removed artifact reached goes, what a remaining parent
still reaches stays. Fixtures publish dependents with `make_dependent` and
lock/install them with the real commands; only the incomputable case edits the
lock by hand.
"""
from __future__ import annotations

import re
from pathlib import Path

from src.helpers import make_artifact, make_bundle, make_dependent, write_config

KEPT_NOTE = re.compile(
    r"kept (\d+) artifacts whose dependency graph could not be computed offline; run grim lock"
)


def _files(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}


def _leaf(unique_repo: str, name: str):
    return make_artifact(f"{unique_repo}/{name}", "skill", _files(name), tag="1")


def _dependent(unique_repo: str, name: str, *requires: str):
    return make_dependent(
        f"{unique_repo}/{name}",
        "1",
        "skill",
        _files(name),
        [("skill", dep, f"./{dep}:1") for dep in requires],
    )


def _lock_names(project_dir: Path) -> set[str]:
    text = (project_dir / "grimoire.lock").read_text()
    return set(re.findall(r'(?m)^\[\[skill\]\]\nname = "([^"]+)"', text))


def _installed(project_dir: Path) -> set[str]:
    root = project_dir / ".claude/skills"
    return {p.name for p in root.iterdir()} if root.is_dir() else set()


def _declared(project_dir: Path) -> set[str]:
    text = (project_dir / "grimoire.toml").read_text()
    skills = text.split("[skills]")[1].split("[")[0]
    return set(re.findall(r"(?m)^([\w-]+) =", skills))


def _drop_lock_entry(project_dir: Path, name: str) -> None:
    """Delete one `[[skill]]` entry and its `[[skill.requires]]` tables."""
    lock = project_dir / "grimoire.lock"
    chunks = re.split(r"(?m)^(?=\[\[)", lock.read_text())
    kept: list[str] = []
    skipping = False
    for chunk in chunks:
        if chunk.startswith("[[skill]]"):
            skipping = f'name = "{name}"' in chunk
        elif not chunk.startswith("[[skill.requires]]"):
            skipping = False
        if not skipping:
            kept.append(chunk)
    lock.write_text("".join(kept))


def _tree(root: Path) -> dict[str, bytes]:
    return {
        str(p): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file() and not p.is_symlink()
    }


def _entry_block(project_dir: Path, name: str) -> str:
    text = (project_dir / "grimoire.lock").read_text()
    return next(
        chunk.strip()
        for chunk in re.split(r"(?m)^(?=\[\[)", text)
        if chunk.startswith("[[skill]]") and f'name = "{name}"' in chunk
    )


def test_remove_drops_orphans(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # a -> b -> c, a -> shared, keep -> shared, keep -> d
    _leaf(unique_repo, "c")
    _leaf(unique_repo, "d")
    _leaf(unique_repo, "shared")
    _dependent(unique_repo, "b", "c")
    a = _dependent(unique_repo, "a", "b", "shared")
    keep = _dependent(unique_repo, "keep", "shared", "d")
    write_config(project_dir, skills={"a": a.fq, "keep": keep.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    everything = {"a", "b", "c", "d", "keep", "shared"}
    assert _lock_names(project_dir) == everything, "positive control: closure locked"
    assert _installed(project_dir) == everything, "positive control: closure installed"

    out = runner.json("remove", "skill", "a")

    assert out["status"] == "removed"
    assert _declared(project_dir) == {"keep"}
    # Only what `a` alone reached left the lock; `shared` and `d` stay.
    assert _lock_names(project_dir) == {"keep", "shared", "d"}
    # `remove` never deletes materialized files.
    assert _installed(project_dir) == everything
    # The lock reads fresh: nothing to re-lock, nothing resurrected on install.
    runner.run("install")
    assert _lock_names(project_dir) == {"keep", "shared", "d"}


def test_remove_offline_incomputable_keeps_files(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # a -> b (b's entry is lost below); keep -> c.
    _leaf(unique_repo, "b")
    _leaf(unique_repo, "c")
    a = _dependent(unique_repo, "a", "b")
    keep = _dependent(unique_repo, "keep", "c")
    write_config(project_dir, skills={"a": a.fq, "keep": keep.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    assert _installed(project_dir) == {"a", "b", "c", "keep"}
    # `a`'s edge now points at an entry the lock no longer holds: the closure
    # cannot be followed offline.
    _drop_lock_entry(project_dir, "b")
    untouched = {name: _entry_block(project_dir, name) for name in ("a", "c")}
    files_before = _tree(project_dir / ".claude")

    result = runner.run("--format", "json", "remove", "skill", "keep")

    assert result.returncode == 0, result.stderr
    assert '"status": "removed"' in result.stdout
    # Plain stderr, one line, counting the artifact it could not judge (`c`).
    match = KEPT_NOTE.search(result.stderr)
    assert match and match.group(1) == "1", result.stderr
    assert _declared(project_dir) == {"a"}
    assert _lock_names(project_dir) == {"a", "c"}, "the target only left the lock"
    for name, block in untouched.items():
        assert _entry_block(project_dir, name) == block, f"{name} entry rewritten"
    assert _tree(project_dir / ".claude") == files_before, "no file touched"
    # The lock stays honestly stale: the next install asks for `grim lock`.
    stale = runner.run("--format", "json", "install", check=False)
    assert stale.returncode != 0 and "grim lock" in stale.stdout + stale.stderr


def test_remove_dependency_free_prints_no_note(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = _leaf(unique_repo, "plain")
    write_config(project_dir, skills={"plain": sk.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    result = runner.run("remove", "skill", "plain")

    assert "could not be computed" not in result.stderr
    assert _lock_names(project_dir) == set()


def test_uninstall_deletes_orphans(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # a -> b -> c, a -> shared, keep -> shared
    _leaf(unique_repo, "c")
    _leaf(unique_repo, "shared")
    _dependent(unique_repo, "b", "c")
    a = _dependent(unique_repo, "a", "b", "shared")
    keep = _dependent(unique_repo, "keep", "shared")
    write_config(project_dir, skills={"a": a.fq, "keep": keep.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    assert _installed(project_dir) == {"a", "b", "c", "keep", "shared"}

    out = runner.json("uninstall", "skill", "a")

    assert out["status"] == "uninstalled"
    assert _declared(project_dir) == {"keep"}
    assert _lock_names(project_dir) == {"keep", "shared"}
    # The files of everything only `a` reached are gone; `shared` stays.
    assert _installed(project_dir) == {"keep", "shared"}
    rows = {r["name"]: r for r in runner.json("status")["items"]}
    assert set(rows) == {"keep", "shared"}
    assert rows["shared"]["state"] == "installed"


def test_uninstall_dependency_only_kept(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _leaf(unique_repo, "dep")
    # `both` is declared directly as well as required by `parent`.
    both = _leaf(unique_repo, "both")
    member = _leaf(unique_repo, "member")
    parent = _dependent(unique_repo, "parent", "dep", "both")
    bundle = make_bundle(f"{unique_repo}/pack", [("skill", "member", member.fq)], tag="1")
    write_config(
        project_dir,
        skills={"parent": parent.fq, "both": both.fq},
        bundles={"pack": bundle.fq},
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    everything = {"parent", "dep", "both", "member"}
    assert _installed(project_dir) == everything, "positive control"
    lock_before = (project_dir / "grimoire.lock").read_text()

    # Dependency-only: a remaining parent reaches it.
    kept = runner.json("uninstall", "skill", "dep")
    assert kept["status"] == "kept-by-dependency"
    assert _installed(project_dir) == everything
    assert (project_dir / "grimoire.lock").read_text() == lock_before
    assert _declared(project_dir) == {"parent", "both"}

    # Held by a bundle: the existing literal is unchanged.
    held = runner.json("uninstall", "skill", "member")
    assert held["status"] == "kept-by-bundle"
    assert _installed(project_dir) == everything

    # Directly declared and still reached: degrades to remove, files kept.
    degraded = runner.json("uninstall", "skill", "both")
    assert degraded["status"] == "uninstalled"
    assert _declared(project_dir) == {"parent"}
    assert _installed(project_dir) == everything
    assert "both" in _lock_names(project_dir), "the parent still requires it"
