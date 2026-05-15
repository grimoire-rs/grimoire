# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim release` acceptance tests — push + cascade tags.

These build a *real* local skill directory / rule file in a tmp path and
`grim release` it (not `make_artifact`), exercising the full
validate → pack → push → cascade-tag path against the live registry.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import write_config


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _local_skill(project_dir: Path, name: str = "code-review") -> Path:
    skill = project_dir / name
    _write(
        skill / "SKILL.md",
        f"---\nname: {name}\ndescription: Review code.\n"
        f"metadata:\n  keywords: review,quality\n---\n# {name}\n",
    )
    _write(skill / "scripts/run.sh", "echo hi\n")
    return skill


def test_release_pushes_with_cascade_tags(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3")
    assert out["pushed"] is True
    assert out["tags"] == ["1.2.3", "1.2", "1", "latest"]
    digest = out["manifest_digest"]
    assert digest.startswith("sha256:")

    # Every cascade tag must resolve to the same manifest digest. Use
    # lock + install per the spine: declare each tag and lock it.
    write_config(
        project_dir,
        skills={
            "cr-exact": f"{repo}:1.2.3",
            "cr-minor": f"{repo}:1.2",
            "cr-major": f"{repo}:1",
            "cr-latest": f"{repo}:latest",
        },
    )
    runner.run("lock", check=False)
    rows = runner.json("status")
    # `grim lock` pins every declared tag; they must all share one digest.
    locked = runner.json("lock")
    pins = {r["name"]: r["pinned"] for r in locked}
    assert len(set(pins.values())) == 1, (
        f"all cascade tags must pin the same digest, got {pins}"
    )
    assert rows  # status renders the declared set


def test_release_dry_run_does_not_push(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:2.0.0", "--dry-run")
    assert out["pushed"] is False
    assert out["tags"] == ["2.0.0", "2.0", "2", "latest"]

    # Nothing was pushed: locking the tag must fail to resolve (79).
    write_config(project_dir, skills={"cr": f"{repo}:2.0.0"})
    result = runner.run("lock", check=False)
    assert result.returncode == 79, (
        f"dry-run must not push (lock should 404→79), got "
        f"{result.returncode}; {result.stderr}"
    )


def test_release_is_idempotent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    first = runner.json("release", str(skill), f"{repo}:1.0.0")
    second = runner.json("release", str(skill), f"{repo}:1.0.0")
    assert first["manifest_digest"] == second["manifest_digest"], (
        "re-releasing identical content must yield the same digest"
    )
    assert second["pushed"] is True


def test_release_refuses_overwrite_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    v1 = _local_skill(project_dir, "code-review")
    runner.json("release", str(v1), f"{repo}:1.0.0")

    # Change the content, re-release the SAME version → must refuse.
    (v1 / "scripts/run.sh").write_text("echo CHANGED\n")
    result = runner.run("release", str(v1), f"{repo}:1.0.0", check=False)
    assert result.returncode == 65, (
        f"overwriting an existing version must exit 65, got "
        f"{result.returncode}; {result.stderr}"
    )

    # With --force the move is allowed.
    forced = runner.json("release", str(v1), f"{repo}:1.0.0", "--force")
    assert forced["pushed"] is True


def test_release_rule_file(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    rule = project_dir / "rust-style.md"
    _write(rule, "---\npaths: ['**/*.rs']\n---\n# Rust Style\nbody\n")
    repo = f"{registry}/{unique_repo}/rust-style"
    runner = grim_at(project_dir)

    out = runner.json("release", str(rule), f"{repo}:3.4.5")
    assert out["pushed"] is True
    assert out["tags"] == ["3.4.5", "3.4", "3", "latest"]

    # Install the released rule and assert the canonical file lands.
    write_config(project_dir, rules={"rust-style": f"{repo}:3.4.5"})
    runner.run("lock", check=False)
    rows = runner.json("install")
    assert {r["status"] for r in rows} == {"installed"}
    assert (project_dir / ".claude/rules/rust-style.md").is_file()


def test_release_prerelease_is_exact_tag_only(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3-rc.1", "--dry-run")
    assert out["tags"] == ["1.2.3-rc.1"], (
        "a prerelease must NOT cascade and must NOT move latest"
    )
