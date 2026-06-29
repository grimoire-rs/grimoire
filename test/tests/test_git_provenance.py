# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`--git` provenance acceptance tests (issue #17).

`grim build`/`release`/`publish --git` embed the publishing commit as the
standard OCI annotations `org.opencontainers.image.{revision,created,source}`.
The flag is opt-in; a non-git path is a hard data error (65).
"""
from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest

from src.registry import fetch_manifest

# git is required to exercise the provenance path; skip cleanly if absent.
pytestmark = pytest.mark.skipif(shutil.which("git") is None, reason="git not on PATH")


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _local_skill(project_dir: Path, name: str = "code-review") -> Path:
    skill = project_dir / name
    _write(
        skill / "SKILL.md",
        f"---\nname: {name}\ndescription: Review code.\n---\n# {name}\n",
    )
    return skill


def _git(repo: Path, *args: str) -> str:
    """Run a git command in `repo`, returning trimmed stdout."""
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def _init_repo(repo: Path, remote: str | None = None) -> None:
    _git(repo, "init", "-q")
    # Repo-local identity so the commit succeeds in an isolated environment.
    _git(repo, "config", "user.email", "test@example.invalid")
    _git(repo, "config", "user.name", "Test")
    if remote is not None:
        _git(repo, "remote", "add", "origin", remote)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", "initial")


def test_build_git_fails_outside_repo(grim_at, project_dir: Path) -> None:
    """`--git` on a non-git path is a hard data error (65), never a silent
    skip — the user explicitly asked for provenance."""
    skill = _local_skill(project_dir)
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill), "--git", check=False)
    assert result.returncode == 65, (
        f"--git outside a repo must exit 65, got {result.returncode}; {result.stderr}"
    )


def test_build_git_succeeds_in_repo(grim_at, project_dir: Path) -> None:
    """`grim build --git` inside a repo succeeds and counts the extra
    provenance annotations."""
    skill = _local_skill(project_dir)
    _init_repo(project_dir)
    runner = grim_at(project_dir)

    plain = runner.json("build", str(skill))
    with_git = runner.json("build", str(skill), "--git")
    assert with_git["status"] == "built"
    # revision + created (no remote configured ⇒ no extra source) = +2.
    assert with_git["annotation_count"] >= plain["annotation_count"] + 2, (
        f"--git must add provenance annotations: {plain} vs {with_git}"
    )


def test_release_git_embeds_revision_and_created(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim release --git` stamps the commit SHA, the commit date, and the
    normalized `origin` remote onto the pushed manifest."""
    skill = _local_skill(project_dir)
    _init_repo(project_dir, remote="git@github.com:acme/code-review.git")
    head = _git(project_dir, "rev-parse", "HEAD")

    repo = f"{registry}/{unique_repo}/code-review"
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3", "--git")
    assert out["pushed"] is True

    annotations = fetch_manifest(repo_path, "1.2.3").get("annotations") or {}
    assert annotations.get("org.opencontainers.image.revision") == head, (
        f"revision must be the HEAD sha (clean tree, no -dirty), got {annotations}"
    )
    assert annotations.get("org.opencontainers.image.created"), (
        f"--git must stamp a commit date, got {annotations}"
    )
    # The scp-like remote is normalized to an https:// source URL.
    assert annotations.get("org.opencontainers.image.source") == (
        "https://github.com/acme/code-review"
    ), annotations


def test_release_without_git_omits_provenance(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A plain release (no `--git`) carries neither provenance annotation —
    the default stays byte-deterministic."""
    skill = _local_skill(project_dir)
    _init_repo(project_dir, remote="git@github.com:acme/code-review.git")

    repo = f"{registry}/{unique_repo}/plain"
    repo_path = f"{unique_repo}/plain"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.0.0")
    assert out["pushed"] is True

    annotations = fetch_manifest(repo_path, "1.0.0").get("annotations") or {}
    assert "org.opencontainers.image.revision" not in annotations, annotations
    assert "org.opencontainers.image.created" not in annotations, annotations


def test_release_git_dirty_marks_revision(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An uncommitted tracked change marks the revision `-dirty`."""
    skill = _local_skill(project_dir)
    _init_repo(project_dir)
    head = _git(project_dir, "rev-parse", "HEAD")
    # Mutate a tracked file without committing.
    (skill / "SKILL.md").write_text(
        "---\nname: code-review\ndescription: Review code, edited.\n---\n# code-review\n"
    )

    repo = f"{registry}/{unique_repo}/dirty"
    repo_path = f"{unique_repo}/dirty"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.0.0", "--git")
    assert out["pushed"] is True

    annotations = fetch_manifest(repo_path, "1.0.0").get("annotations") or {}
    assert annotations.get("org.opencontainers.image.revision") == f"{head}-dirty", (
        f"a dirty tracked tree must suffix the revision with -dirty, got {annotations}"
    )


# ── B: Bundle --git fail-fast (PASSES now — regression guard) ───────────────


def test_build_bundle_git_fails_outside_repo(grim_at, project_dir: Path) -> None:
    """`grim build --git` on a bundle `.toml` outside a git repo must exit 65.

    The bundle branch in ``build`` once returned before checking ``--git``, so
    the flag was silently ignored and the command exited 0. This is a regression
    guard: re-introducing that early return would make this test fail.
    """
    bundle = project_dir / "my-bundle.toml"
    bundle.write_text("[skills]\ncr = \"ghcr.io/acme/code-review:1.0.0\"\n")

    runner = grim_at(project_dir)
    result = runner.run("build", str(bundle), "--git", check=False)
    assert result.returncode == 65, (
        f"--git on a bundle outside a git repo must exit 65 (DataError), "
        f"got {result.returncode}; stderr: {result.stderr}"
    )


def test_build_bundle_git_succeeds_in_repo(grim_at, project_dir: Path) -> None:
    """`grim build --git` on a bundle `.toml` INSIDE a git repo exits 0.

    The success branch of the bundle fail-fast guard: a git working tree
    satisfies the ``--git`` contract, and a bundle build emits no provenance
    annotations, so the command completes normally.
    """
    bundle = project_dir / "my-bundle.toml"
    bundle.write_text("[skills]\ncr = \"ghcr.io/acme/code-review:1.0.0\"\n")
    _init_repo(project_dir)

    runner = grim_at(project_dir)
    result = runner.run("build", str(bundle), "--git", check=False)
    assert result.returncode == 0, (
        f"--git on a bundle inside a git repo must exit 0, "
        f"got {result.returncode}; stderr: {result.stderr}"
    )


# ── C: `created` determinism guard (PASSES now — regression guard) ──────────


def test_release_git_created_is_per_commit_date_not_wall_clock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``org.opencontainers.image.created`` is the per-commit committer date
    (``%cI``), NOT wall-clock time, so the same commit released under two
    different version tags produces byte-identical timestamps.

    This is a regression guard: swapping to wall-clock would cause both the
    annotation values to differ from each other AND to differ from the git
    committer date, making this test fail.
    """
    skill = _local_skill(project_dir)
    _init_repo(project_dir)

    # The reference timestamp: git's own %cI for HEAD.
    expected_date = _git(project_dir, "show", "-s", "--format=%cI", "HEAD")

    repo_base = f"{registry}/{unique_repo}/created-guard"
    repo_path = f"{unique_repo}/created-guard"
    runner = grim_at(project_dir)

    # Release the same commit twice under different version tags.
    out1 = runner.json("release", str(skill), f"{repo_base}:1.0.0", "--git")
    out2 = runner.json("release", str(skill), f"{repo_base}:2.0.0", "--git")
    assert out1["pushed"] is True, f"first release must push: {out1}"
    assert out2["pushed"] is True, f"second release must push: {out2}"

    ann1 = fetch_manifest(repo_path, "1.0.0").get("annotations") or {}
    ann2 = fetch_manifest(repo_path, "2.0.0").get("annotations") or {}

    created1 = ann1.get("org.opencontainers.image.created")
    created2 = ann2.get("org.opencontainers.image.created")

    assert created1 is not None, (
        f"release 1.0.0 must carry a created annotation; got annotations={ann1}"
    )
    assert created2 is not None, (
        f"release 2.0.0 must carry a created annotation; got annotations={ann2}"
    )
    # Both releases of the SAME commit must produce identical timestamps.
    assert created1 == created2, (
        f"same commit released twice must yield byte-identical created annotation: "
        f"{created1!r} (1.0.0) != {created2!r} (2.0.0)"
    )
    # The value must equal the git committer date, not a wall-clock timestamp.
    assert created1 == expected_date, (
        f"created annotation must equal git committer date (%%cI), "
        f"got {created1!r}, expected {expected_date!r}"
    )


# ── D: Untracked-only file does NOT mark dirty (PASSES now — regression guard)


def test_release_git_untracked_file_does_not_mark_dirty(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An untracked (un-staged, uncommitted) file must NOT append ``-dirty``
    to the revision annotation.

    Grimoire's dirty-detection uses ``git status --porcelain --untracked-files=no``
    which explicitly ignores untracked files: only *tracked* changes dirty the
    tree.  This is a regression guard so that switching to a mode that counts
    untracked files causes this test to fail.
    """
    skill = _local_skill(project_dir)
    _init_repo(project_dir)
    head = _git(project_dir, "rev-parse", "HEAD")

    # Write an untracked file — not staged, not committed.
    (project_dir / "scratch.txt").write_text("not tracked, must not dirty the tree")

    repo = f"{registry}/{unique_repo}/untracked-guard"
    repo_path = f"{unique_repo}/untracked-guard"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.0.0", "--git")
    assert out["pushed"] is True

    annotations = fetch_manifest(repo_path, "1.0.0").get("annotations") or {}
    revision = annotations.get("org.opencontainers.image.revision")

    assert revision == head, (
        f"an untracked-only change must not dirty the revision; "
        f"expected {head!r}, got {revision!r}; annotations={annotations}"
    )
    assert revision is not None and "-dirty" not in revision, (
        f"revision must not carry a -dirty suffix when the only change is untracked: "
        f"{revision!r}"
    )
