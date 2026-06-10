# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim add` / `grim remove` acceptance tests — edit config + lock."""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config


def test_add_declares_and_locks_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    # Start from an empty config.
    write_config(project_dir)
    runner = grim_at(project_dir)

    # New CLI: reference is the only required arg. Kind is inferred from the
    # manifest's `com.grimoire.kind` annotation; name defaults to the
    # reference's last path segment (`code-review`).
    out = runner.json("add", sk.fq)
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "added"
    assert "@sha256:" in out["pinned"]

    # The config now declares it and the lock pins it.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "code-review" in cfg
    assert (project_dir / "grimoire.lock").is_file()
    status = runner.json("status")
    cr = next(r for r in status if r["name"] == "code-review")
    assert cr["state"] in ("missing", "outdated", "installed")


def test_add_then_remove_round_trip(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    runner.json("add", ru.fq)
    assert "rust-style" in (project_dir / "grimoire.toml").read_text()

    out = runner.json("remove", "rule", "rust-style")
    assert out["status"] == "removed"

    cfg = (project_dir / "grimoire.toml").read_text()
    assert "rust-style" not in cfg

    # The lock no longer carries the entry and its declaration hash is
    # back in sync with the (now empty) config — install is a clean no-op.
    lock = (project_dir / "grimoire.lock").read_text()
    assert "rust-style" not in lock


def test_remove_absent_entry_is_reported_not_error(
    grim_at, project_dir: Path, registry: str
) -> None:
    write_config(project_dir)
    runner = grim_at(project_dir)
    out = runner.json("remove", "skill", "never-declared")
    assert out["status"] == "absent"


def test_add_two_entries_then_lock_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    runner.json("add", sk.fq)
    runner.json("add", ru.fq)

    # The lock carries both; install materializes both cleanly.
    rows = runner.json("install")
    assert {r["status"] for r in rows} == {"installed"}
    assert (project_dir / ".claude/skills/code-review/SKILL.md").is_file()
    assert (project_dir / ".claude/rules/rust-style.md").is_file()
