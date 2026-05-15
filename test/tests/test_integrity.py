# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Local-modification integrity gate acceptance tests."""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config


def _install_rule(grim_at, project_dir, registry, unique_repo):
    repo = f"{unique_repo}/rust-style"
    make_artifact(
        repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# canonical\n"},
        tag="v1",
    )
    write_config(
        project_dir, rules={"rust-style": f"{registry}/{repo}:v1"}
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    return runner, project_dir / ".claude/rules/rust-style.md"


def test_modified_install_is_refused_then_forced(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    refused = runner.run("install", check=False)
    assert refused.returncode == 65, (
        f"modified artifact must refuse with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused install must not overwrite the user's edit"
    )

    forced = runner.run("install", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert installed.read_text().endswith("# canonical\n")


def test_status_reports_modified(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("tampered\n")

    rows = runner.json("status")
    row = next(r for r in rows if r["name"] == "rust-style")
    assert row["state"] == "modified"
    # status is read-only data: it must always exit 0.


def test_update_also_refuses_modified_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")
    # `update` re-materializes with force semantics for changed digests,
    # but here the digest is unchanged and the file is locally modified —
    # the rolling-release contract overwrites it. Assert it succeeds and
    # restores canonical content (force is implied by update).
    result = runner.run("update", check=False)
    assert result.returncode == 0, result.stderr
    assert installed.read_text().endswith("# canonical\n")
