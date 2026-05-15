# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim install` acceptance tests."""
from __future__ import annotations

from pathlib import Path

from src.assertions import assert_dir_exists, assert_path_exists
from src.helpers import make_artifact, write_config


def _setup(project_dir, unique_repo):
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {
            "code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n",
            "code-review/scripts/run.sh": "echo hi\n",
        },
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    write_config(
        project_dir,
        skills={"code-review": sk.fq},
        rules={"rust-style": ru.fq},
    )
    return sk, ru


def test_lock_then_install_materializes_files(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _setup(project_dir, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    rows = runner.json("install")
    assert {r["status"] for r in rows} == {"installed"}

    assert_dir_exists(project_dir / ".claude/skills/code-review")
    assert_path_exists(
        project_dir / ".claude/skills/code-review/SKILL.md"
    )
    assert_path_exists(
        project_dir / ".claude/skills/code-review/scripts/run.sh"
    )
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")


def test_install_without_lock_exits_79(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _setup(project_dir, unique_repo)
    runner = grim_at(project_dir)
    result = runner.run("install", check=False)
    assert result.returncode == 79, (
        f"install without a lock must exit 79, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_stale_lock_blocks_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk, ru = _setup(project_dir, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    # Change the declaration without re-locking.
    extra = make_artifact(
        f"{unique_repo}/docs",
        "rule",
        {"docs.md": "---\npaths: ['**/*.md']\n---\n# docs\n"},
        tag="v1",
    )
    write_config(
        project_dir,
        skills={"code-review": sk.fq},
        rules={"rust-style": ru.fq, "docs": extra.fq},
    )
    result = runner.run("install", check=False)
    assert result.returncode == 65, (
        f"stale lock must exit 65, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_offline_cold_cache_blocks_install_exit_81(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _setup(project_dir, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)  # online: pins resolved

    # Fresh GRIM_HOME ⇒ blob cache is cold; offline must refuse.
    runner.env["GRIM_HOME"] = str(project_dir / "cold-home")
    result = runner.run("--offline", "install", check=False)
    assert result.returncode == 81, (
        f"offline cold-cache install must exit 81, got "
        f"{result.returncode}; {result.stderr}"
    )


def test_offline_warm_blob_cache_succeeds(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _setup(project_dir, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)  # warms the blob cache

    # Same GRIM_HOME ⇒ blobs cached ⇒ offline reinstall is a no-op success.
    result = runner.run("--offline", "install", check=False)
    assert result.returncode == 0, (
        f"offline warm-cache install must succeed, got "
        f"{result.returncode}; {result.stderr}"
    )
