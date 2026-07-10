# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim add` / `grim remove` acceptance tests — edit config + lock."""
from __future__ import annotations

from pathlib import Path

from src.assertions import assert_not_exists
from src.helpers import make_artifact, write_config


def test_add_no_install_declares_and_locks_only(
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
    # reference's last path segment (`code-review`). `--no-install` keeps this
    # to the declare + lock step (no materialization).
    out = runner.json("add", "--no-install", sk.fq)
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "added"
    assert "@sha256:" in out["pinned"]

    # The config declares it and the lock pins it — but nothing is on disk.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "code-review" in cfg
    assert (project_dir / "grimoire.lock").is_file()
    assert_not_exists(project_dir / ".claude/skills/code-review")
    status = runner.json("status")["items"]
    cr = next(r for r in status if r["name"] == "code-review")
    assert cr["state"] == "missing"


def test_add_installs_by_default(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    # No `--no-install`: `grim add` declares, locks, AND materializes the
    # artifact into the detected clients in one step.
    out = runner.json("add", sk.fq)
    assert out["status"] == "added"
    assert (project_dir / ".claude/skills/code-review/SKILL.md").is_file()

    # The install state reports it installed; a follow-up `install` is a
    # clean no-op since the artifact is already materialized.
    cr = next(r for r in runner.json("status")["items"] if r["name"] == "code-review")
    assert cr["state"] == "installed"
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"unchanged"}


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

    runner.json("add", "--no-install", ru.fq)
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


def test_add_same_name_conflicting_reference_refuses(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A declared name is a true per-scope-unique key: re-declaring
    `(kind, name)` against a *different* identifier must refuse loudly
    (exit 64, UsageError) instead of silently clobbering the first
    declaration. Both artifacts share the last path segment
    (`code-review`), so `add` infers the same default binding name for
    both — a realistic name collision, not a contrived one.
    """
    sk_a = make_artifact(
        f"{unique_repo}/vendor-a/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: vendor a\n---\n# CR a\n"},
        tag="stable",
    )
    sk_b = make_artifact(
        f"{unique_repo}/vendor-b/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: vendor b\n---\n# CR b\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    out = runner.json("add", "--no-install", sk_a.fq)
    assert out["name"] == "code-review"

    result = runner.run("add", "--no-install", sk_b.fq, check=False)
    assert result.returncode == 64, (
        f"conflicting re-declare must exit 64 (UsageError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "code-review" in result.stderr
    assert "--name" in result.stderr

    # The refusal must not mutate the config — vendor-a's declaration
    # survives untouched, vendor-b never lands.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "vendor-a" in cfg
    assert "vendor-b" not in cfg


def test_add_same_name_same_reference_is_idempotent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Re-declaring the exact same `(kind, name, identifier)` stays a clean
    no-op overwrite — the conflict guard only fires on a genuine mismatch.
    """
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)

    first = runner.json("add", "--no-install", sk.fq)
    assert first["status"] == "added"

    second = runner.json("add", "--no-install", sk.fq)
    assert second["status"] == "added"
    assert second["pinned"] == first["pinned"]


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

    # `--no-install` defers materialization so the batch installs in one
    # `grim install` pass below (the reason `--no-install` exists).
    runner.json("add", "--no-install", sk.fq)
    runner.json("add", "--no-install", ru.fq)

    # The lock carries both; install materializes both cleanly.
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}
    assert (project_dir / ".claude/skills/code-review/SKILL.md").is_file()
    assert (project_dir / ".claude/rules/rust-style.md").is_file()
