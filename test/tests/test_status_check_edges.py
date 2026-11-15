# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim status --check` reads a dependency row's reference from edges the
user's own store lists, never from the lock alone (CWE-345), and answers a key
several references require only where they name one digest."""
from __future__ import annotations

from pathlib import Path

import pytest

from src.helpers import (
    make_artifact,
    make_bundle,
    make_dependent,
    publish_release,
    write_config,
)
from src.registry import REGISTRY_HOST, PublishedArtifact, retag


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
    exits 78. The answer is null in every parent order, and the note says why.

    Covers the in-place edit under `--no-relock`; plain `remove` is
    covered by `test_remove_relock.py`."""
    # Literal tags on `requires` edges: numeric ones claim two majors of `x`, which
    # `grim lock` refuses once the direct line is gone.
    low, high = ("v1", "v2") if shape == "requires" else ("1", "2")
    x1 = _skill_at(unique_repo, "x", low, "one")
    x2 = _skill_at(unique_repo, "x", high, "two")
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
    runner.run("remove", "--no-relock", "skill", "x")
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


@pytest.mark.parametrize(
    ("high", "release", "answer", "exit_code"),
    [("1.2", "1.2.0", True, 0), ("2", "2.0.0", None, 78)],
    ids=["same-major", "two-majors"],
)
def test_status_check_answers_a_range_claimed_row_by_its_lines_target_tag(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    high: str,
    release: str,
    answer: bool | None,
    exit_code: int,
) -> None:
    """Parents claim `x:1` and `x:<high>` of annotated releases, and `x = x:<high>`
    holds the pin until `remove skill x`. One major is one line, read through its
    float `1`: once 1.3.0 ships the row reads `true` with no split note, and
    `grim update` takes 1.3.0. Two majors satisfy no line, so the row reads null
    with the split note, and `grim lock` and `grim update` exit 78.

    Covers the in-place edit under `--no-relock`; plain `remove` is
    covered by `test_remove_relock.py`."""
    repo = f"{unique_repo}/x"
    files = {"x/SKILL.md": "---\nname: x\ndescription: d\n---\n# x\n"}
    for version in ("1.0.0", release):
        publish_release(repo, version, files)
    low_claim, high_claim = f"{REGISTRY_HOST}/{repo}:1", f"{REGISTRY_HOST}/{repo}:{high}"
    parents = {
        name: make_dependent(
            f"{unique_repo}/{name}",
            "1",
            "skill",
            {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"},
            [("skill", "x", claim)],
        ).fq
        for name, claim in (("p", low_claim), ("q", high_claim))
    }
    write_config(project_dir, skills={"x": high_claim, **parents})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("remove", "--no-relock", "skill", "x")
    assert runner.run("lock", check=False).returncode == exit_code
    shipped = publish_release(repo, "1.3.0", files)
    scope = ("--registry", f"{REGISTRY_HOST}/{unique_repo}")

    result = runner.json(*scope, "status", "--check")
    plain = runner.plain(*scope, "status", "--check")

    assert result["checked"] is True
    assert [r["update_available"] for r in result["items"] if r["name"] == "x"] == [answer]
    if answer is None:
        split = f"{low_claim} and {high_claim} name different digests; `grim update` exits 78 until grimoire.toml declares one"
        assert split in plain.stderr, plain.stderr
        assert "tag moved" not in plain.stderr, plain.stderr
    else:
        assert "name different digests" not in plain.stderr, plain.stderr
        moved = "update available: skill 'x': its tag moved, and more than one reference requires it"
        assert moved in plain.stderr, plain.stderr
    update = runner.run("update", check=False)
    assert update.returncode == exit_code, update.stderr
    lock = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert (shipped.digest in lock) is (exit_code == 0), lock


def _skill_at(unique_repo: str, name: str, tag: str, body: str) -> PublishedArtifact:
    text = f"---\nname: {name}\ndescription: d\n---\n# {body}\n"
    return make_artifact(f"{unique_repo}/{name}", "skill", {f"{name}/SKILL.md": text}, tag=tag)


def _x_answer(runner, unique_repo: str) -> tuple[bool | None, str]:
    """`x`'s `update_available` and the plain run's stderr."""
    scope = ("--registry", f"{REGISTRY_HOST}/{unique_repo}")
    result = runner.json(*scope, "status", "--check")
    assert result["checked"] is True
    answer = next(r["update_available"] for r in result["items"] if r["name"] == "x")
    return answer, runner.plain(*scope, "status", "--check").stderr


def test_status_check_answers_a_bare_declaration_by_the_claim_it_yields_to(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`x` is declared with no tag and `a` requires `x:2`, so the pin follows the
    claim to 2.6.0 while `latest` names 3.1.0. `grim update -- x` reads the
    claim's float `2`, so the row reads `false` with no note until 2.7.0 ships;
    then it reads `true`, and the command the note names takes 2.7.0."""
    repo = f"{unique_repo}/x"
    files = {"x/SKILL.md": "---\nname: x\ndescription: d\n---\n# x\n"}
    held = publish_release(repo, "2.6.0", files)
    publish_release(repo, "3.1.0", files)
    parent = make_dependent(
        f"{unique_repo}/a",
        "1",
        "skill",
        {"a/SKILL.md": "---\nname: a\ndescription: d\n---\n# a\n"},
        [("skill", "x", f"{REGISTRY_HOST}/{repo}:2")],
    )
    write_config(project_dir, skills={"x": f"{REGISTRY_HOST}/{repo}", "a": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"

    answer, stderr = _x_answer(runner, unique_repo)
    assert answer is False, stderr
    assert "update available: skill 'x'" not in stderr, stderr
    runner.run("update", "--", "x")
    assert held.digest in lock.read_text(encoding="utf-8")

    shipped = publish_release(repo, "2.7.0", files)
    answer, stderr = _x_answer(runner, unique_repo)
    assert answer is True, stderr
    assert "update available: skill 'x': `grim update -- x` moves its pin" in stderr, stderr
    runner.run("update", "--", "x")
    assert shipped.digest in lock.read_text(encoding="utf-8")


def test_status_check_answers_a_bare_declaration_by_its_claim_beside_a_literal(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """As above, with `b` requiring the literal `x:stable` too. The resolver
    yields the bare `x` to `a`'s claim alone, so the row reads the float `2`,
    never `latest`: `false` with no note until 2.7.0 ships, then `true`, and
    the update takes 2.7.0."""
    repo = f"{unique_repo}/x"
    files = {"x/SKILL.md": "---\nname: x\ndescription: d\n---\n# x\n"}
    held = publish_release(repo, "2.6.0", files)
    publish_release(repo, "3.1.0", files)
    retag(repo, "stable", held.digest)
    parents = {
        name: make_dependent(
            f"{unique_repo}/{name}",
            "1",
            "skill",
            {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"},
            [("skill", "x", f"{REGISTRY_HOST}/{repo}:{tag}")],
        ).fq
        for name, tag in (("a", "2"), ("b", "stable"))
    }
    write_config(project_dir, skills={"x": f"{REGISTRY_HOST}/{repo}", **parents})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"

    answer, stderr = _x_answer(runner, unique_repo)
    assert answer is False, stderr
    assert "update available: skill 'x'" not in stderr, stderr
    runner.run("update", "--", "x")
    assert held.digest in lock.read_text(encoding="utf-8")

    shipped = publish_release(repo, "2.7.0", files)
    answer, stderr = _x_answer(runner, unique_repo)
    assert answer is True, stderr
    assert "update available: skill 'x': `grim update -- x` moves its pin" in stderr, stderr
    runner.run("update", "--", "x")
    assert shipped.digest in lock.read_text(encoding="utf-8")


def test_status_check_answers_null_where_the_lines_float_names_another_major(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`p` requires `x:1`, pinned on 1.0.0, and then the float `1` is moved onto
    the 2.0.0 manifest. `grim update -- x` passes that release over, as it lies
    outside the claim, and reads the tag listing, which `status --check` does
    not, so the row reads null with no note, before the update and after it."""
    repo = f"{unique_repo}/x"
    files = {"x/SKILL.md": "---\nname: x\ndescription: d\n---\n# x\n"}
    held = publish_release(repo, "1.0.0", files)
    other = publish_release(repo, "2.0.0", files)
    parent = make_dependent(
        f"{unique_repo}/p",
        "1",
        "skill",
        {"p/SKILL.md": "---\nname: p\ndescription: d\n---\n# p\n"},
        [("skill", "x", f"{REGISTRY_HOST}/{repo}:1")],
    )
    write_config(project_dir, skills={"p": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    retag(repo, "1", other.digest)
    lock = project_dir / "grimoire.lock"

    for _ in range(2):
        answer, stderr = _x_answer(runner, unique_repo)
        assert answer is None, stderr
        assert "update available" not in stderr, stderr
        runner.run("update", "--", "x")
        assert held.digest in lock.read_text(encoding="utf-8")
