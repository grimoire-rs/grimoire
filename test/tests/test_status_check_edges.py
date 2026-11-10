# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim status --check` reads a dependency row's reference from edges the
user's own store lists, never from the lock alone (CWE-345), and answers a key
several references require only where they name one digest."""
from __future__ import annotations

from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, make_dependent, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact


def _skill(unique_repo: str, name: str, body: str) -> PublishedArtifact:
    text = f"---\nname: {name}\ndescription: d\n---\n# {body}\n"
    return make_artifact(f"{unique_repo}/{name}", "skill", {f"{name}/SKILL.md": text}, tag="1")


def _edited_edge(lock: str, unique_repo: str, old: PublishedArtifact) -> str:
    return lock.replace('id = "./dep:1"', f'id = "./dep:1@{old.digest}"')


def _planted_parent(lock: str, unique_repo: str, old: PublishedArtifact) -> str:
    return lock + f'''
[[skill]]
name = "planted"
pinned = "{REGISTRY_HOST}/{unique_repo}/planted@sha256:{"f" * 64}"

[[skill.requires]]
kind = "skill"
name = "dep"
id = "{REGISTRY_HOST}/{unique_repo}/dep:1@{old.digest}"
'''


@pytest.mark.parametrize("forge", [_edited_edge, _planted_parent], ids=["edited-edge", "planted-parent"])
def test_status_check_answers_no_dependency_row_on_an_edge_the_store_does_not_list(
    grim_at, project_dir: Path, registry: str, unique_repo: str, forge
) -> None:
    """The control reads `true` for the moved tag; a digest edge the lock alone
    carries would read `false` however far the tag moved, so the row answers
    nothing instead."""
    old = _skill(unique_repo, "dep", "v1")
    parent = make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        {"parent/SKILL.md": "---\nname: parent\ndescription: d\n---\n# v1\n"},
        [("skill", "dep", "./dep:1")],
    )
    write_config(project_dir, skills={"parent": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _skill(unique_repo, "dep", "v2")

    def dep_answer() -> bool | None:
        result = runner.json("--registry", f"{REGISTRY_HOST}/{unique_repo}", "status", "--check")
        assert result["checked"] is True
        return next(r["update_available"] for r in result["items"] if r["name"] == "dep")

    assert dep_answer() is True

    lock = project_dir / "grimoire.lock"
    lock.write_text(forge(lock.read_text(encoding="utf-8"), unique_repo, old), encoding="utf-8", newline="\n")

    assert dep_answer() is None


@pytest.mark.parametrize("shape", ["b1-first", "b2-first", "requires"])
def test_status_check_answers_null_where_a_keys_references_name_different_digests(
    grim_at, project_dir: Path, registry: str, unique_repo: str, shape: str
) -> None:
    """`x = x:2` held off a parent's `x:1`. After `remove skill x`, `grim lock`
    keeps x:2's digest and no tag moves, while `grim update` meets two digests and
    exits 78. The answer is null in every parent order, and the note says why."""
    x1 = _skill_at(unique_repo, "x", "1", "one")
    x2 = _skill_at(unique_repo, "x", "2", "two")
    if shape == "requires":
        parents = {
            name: make_dependent(
                f"{unique_repo}/{name}",
                "1",
                "skill",
                {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"},
                [("skill", "x", wanted.fq)],
            ).fq
            for name, wanted in (("p", x1), ("q", x2))
        }
        write_config(project_dir, skills={"x": x2.fq, **parents})
    else:
        b1 = make_bundle(f"{unique_repo}/b1", [("skill", "x", x1.fq)], tag="1")
        b2 = make_bundle(f"{unique_repo}/b2", [("skill", "x", x2.fq)], tag="1")
        # The lock lists bundles by binding, so `a2` puts x:2's bundle first.
        bundles = {"b1": b1.fq, "b2": b2.fq} if shape == "b1-first" else {"a2": b2.fq, "b1": b1.fq}
        write_config(project_dir, skills={"x": x2.fq}, bundles=bundles)
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    runner.run("remove", "skill", "x")
    runner.run("lock")
    scope = ("--registry", f"{REGISTRY_HOST}/{unique_repo}")

    result = runner.json(*scope, "status", "--check")
    plain = runner.plain(*scope, "status", "--check")

    assert result["checked"] is True
    assert [r["update_available"] for r in result["items"] if r["name"] == "x"] == [None]
    assert f"{x1.fq} and {x2.fq} name different digests" in plain.stderr, plain.stderr
    assert "`grim update` exits 78 until grimoire.toml declares one" in plain.stderr, plain.stderr
    assert "tag moved" not in plain.stderr, plain.stderr
    assert runner.run("update", "--", "x", check=False).returncode == 78


def _skill_at(unique_repo: str, name: str, tag: str, body: str) -> PublishedArtifact:
    text = f"---\nname: {name}\ndescription: d\n---\n# {body}\n"
    return make_artifact(f"{unique_repo}/{name}", "skill", {f"{name}/SKILL.md": text}, tag=tag)
