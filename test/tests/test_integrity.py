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


def test_modified_add_is_refused_then_forced(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim add` installs-on-add through the same integrity gate as
    `grim install`: re-adding the same reference over a locally modified
    file refuses (65) until `--force` — the VS Code-extension retry shape."""
    repo = f"{unique_repo}/rust-style"
    make_artifact(
        repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# canonical\n"},
        tag="v1",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)
    ref = f"{registry}/{repo}:v1"

    added = runner.run("add", ref, check=False)
    assert added.returncode == 0, added.stderr
    installed = project_dir / ".claude/rules/rust-style.md"
    installed.write_text("hand edited\n")

    # Re-adding the same reference is an idempotent re-declare that reaches
    # the shared install pipeline — the integrity gate refuses it.
    refused = runner.run("add", ref, check=False)
    assert refused.returncode == 65, (
        f"modified artifact must refuse re-add with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused add must not overwrite the user's edit"
    )

    forced = runner.run("add", "--force", ref, check=False)
    assert forced.returncode == 0, forced.stderr
    assert installed.read_text().endswith("# canonical\n")


def test_status_reports_modified(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("tampered\n")

    rows = runner.json("status")["items"]
    row = next(r for r in rows if r["name"] == "rust-style")
    assert row["state"] == "modified"
    # status is read-only data: it must always exit 0.


def test_update_also_refuses_modified_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`update` runs the same integrity gate as `install`.

    Regression: `update` used to pass a hard-coded `force = true` into the
    installer, so a locally modified artifact was overwritten silently —
    exit 0, no warning, no report field — while `install` refused the same
    bytes with 65. Destroying a hand edit is the one thing `--force` exists
    to gate, and update's own prune/reap passes already honour it.
    """
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    refused = runner.run("update", check=False)
    assert refused.returncode == 65, (
        f"modified artifact must refuse update with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused update must not overwrite the user's edit"
    )

    forced = runner.run("update", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert installed.read_text().endswith("# canonical\n")


def test_update_refusal_names_the_artifact_on_stderr(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The refused artifact is named, not just counted by the exit code.

    `update` inspects only `Err` outcomes; a `Refused` one is `Ok(..)`, so
    without explicit handling the refusal falls through as exit 0 with the
    artifact silently unmaterialized. The refusal travels the same route as
    `install`'s: an `IntegrityMismatch` carrying the artifact reference. The
    report itself is discarded on the failing path (see `install::finish`),
    so stderr is the whole signal.
    """
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    result = runner.run("update", check=False)
    assert result.returncode == 65, result.stderr
    assert "rust-style" in result.stderr, (
        f"the refusal must name the artifact; got {result.stderr}"
    )
