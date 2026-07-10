# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim release` acceptance tests — non-semver and partial-semver tags.

Non-version tags (e.g. `canary`, `edge`, `1.2`) publish exactly one tag
with no cascade.  Full semver (`X.Y.Z`) cascades as already covered by
test_release.py.  An empty tag (no `:tag` in the reference) is rejected
with exit 65.
"""
from __future__ import annotations

from pathlib import Path


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


def test_release_canary_tag_no_cascade(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A non-version tag like `canary` publishes exactly one tag, no cascade."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:canary")
    assert out["pushed"] is True
    assert out["tags"] == ["canary"], (
        f"non-version tag must publish exactly one literal tag, got {out['tags']}"
    )
    assert out["manifest_digest"].startswith("sha256:")


def test_release_canary_dry_run_single_tag(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--dry-run with a non-version tag also reports exactly one tag."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:canary", "--dry-run")
    assert out["pushed"] is False
    assert out["tags"] == ["canary"], (
        f"dry-run non-version tag must report exactly one tag, got {out['tags']}"
    )


def test_release_edge_tag_no_cascade(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Another non-version tag (`edge`) publishes exactly one tag."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:edge")
    assert out["pushed"] is True
    assert out["tags"] == ["edge"], (
        f"non-version tag 'edge' must publish exactly one literal tag, got {out['tags']}"
    )


def test_release_partial_semver_tag_no_cascade(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A partial semver tag like `1.2` (no patch) publishes exactly one tag.

    `1.2` is not a valid full semver (`X.Y.Z`) so no cascade fires.
    """
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2")
    assert out["pushed"] is True
    assert out["tags"] == ["1.2"], (
        f"partial semver '1.2' must publish exactly one tag (no cascade), "
        f"got {out['tags']}"
    )


def test_release_missing_tag_exits_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A reference with no tag is rejected with exit code 65 (DataError)."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    result = runner.run("release", str(skill), repo, check=False)
    assert result.returncode == 65, (
        f"a tagless release reference must exit 65, got "
        f"{result.returncode}; stderr: {result.stderr}"
    )


def test_release_no_cascade_semver_single_tag(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--no-cascade publishes only the exact semver tag, suppressing the
    `X.Y`/`X`/`latest` floats."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3", "--no-cascade")
    assert out["pushed"] is True
    assert out["tags"] == ["1.2.3"], (
        f"--no-cascade must publish exactly the exact tag, got {out['tags']}"
    )


def test_release_cascade_semver_still_cascades(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--cascade on a full semver moves the whole rolling set (as the default
    would), asserting the flag is honoured."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3", "--cascade")
    assert out["pushed"] is True
    assert out["tags"] == ["1.2.3", "1.2", "1", "latest"], (
        f"--cascade on full semver must move the rolling set, got {out['tags']}"
    )


def test_release_cascade_on_non_semver_exits_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--cascade requires a semver tag; a channel tag with --cascade is a
    DataError (65) — the typo guard."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    result = runner.run("release", str(skill), f"{repo}:canary", "--cascade", check=False)
    assert result.returncode == 65, (
        f"--cascade on a non-semver tag must exit 65, got "
        f"{result.returncode}; stderr: {result.stderr}"
    )
