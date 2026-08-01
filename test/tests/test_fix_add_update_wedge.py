# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Regression tests for the `add`/`update` mutation-ordering defects.

Independent bugs, one theme — a command committed a durable edit before the
step that could still fail, leaving the project in a state no retry of the
same command could clear.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config


def _seed_working_project(grim_at, project_dir: Path, registry: str, unique_repo: str):
    """A project with one declared, locked and installed skill (`keep`).

    Returns ``(runner, unresolvable_ref, good_ref)`` where both refs bind the
    same name (`code-review`) — that pairing is what the declare-conflict
    guard refuses once a failed declaration has been left on disk.
    """
    make_artifact(
        f"{unique_repo}/keep",
        "skill",
        {"keep/SKILL.md": "---\nname: keep\ndescription: d\n---\n# keep\n"},
        tag="stable",
    )
    make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.json("add", f"{registry}/{unique_repo}/keep:stable")

    return (
        runner,
        f"{registry}/{unique_repo}/code-review:9.9.9",
        f"{registry}/{unique_repo}/code-review:stable",
    )


def test_add_unresolvable_ref_leaves_config_and_project_intact(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A failed relock must roll the declaration back out of `grimoire.toml`.

    Committing it left the project wedged: the declaration hash no longer
    matched the lock, so `grim install` refused with LockStale (65) for
    *every* artifact — including the unrelated, already-working one.
    """
    runner, bad_ref, _ = _seed_working_project(
        grim_at, project_dir, registry, unique_repo
    )
    config = project_dir / "grimoire.toml"
    before = config.read_bytes()

    result = runner.run("add", "--kind", "skill", bad_ref, check=False)
    assert result.returncode != 0, "an unresolvable reference must fail the add"

    assert config.read_bytes() == before, (
        "a failed relock must leave grimoire.toml byte-identical; it committed "
        f"the declaration instead:\n{config.read_text()}"
    )

    # The pre-existing artifact still installs — no LockStale wedge.
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"unchanged"}, rows


def test_add_retry_with_corrected_ref_succeeds(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """After a failed add, the obvious recovery — retry with a corrected
    reference — must work. The committed bad declaration used to make the
    same `(kind, name)` a conflict, so the retry exited 64 and `grim remove`
    was the only escape (mentioned in no error message).
    """
    runner, bad_ref, good_ref = _seed_working_project(
        grim_at, project_dir, registry, unique_repo
    )
    assert runner.run("add", "--kind", "skill", bad_ref, check=False).returncode != 0

    out = runner.json("add", "--kind", "skill", good_ref)
    assert out["name"] == "code-review"
    assert out["status"] == "added"
    assert (project_dir / ".claude/skills/code-review/SKILL.md").is_file()
