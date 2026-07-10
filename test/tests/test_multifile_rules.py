# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Multi-file rule acceptance tests — index `.md` + sibling support dir.

A rule may carry an optional sibling support directory (`<name>/`) beside
its index `<name>.md`. These build a *real* local rule with a support dir
and `grim release` it (exercising the full pack → push path, including the
packer picking up the sibling directory), then install/uninstall it against
the live registry and assert both the index file and the support tree land
and are removed together.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import write_config


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _local_multifile_rule(project_dir: Path, name: str = "my-rule") -> Path:
    """Write a rule index `<name>.md` plus a sibling `<name>/` support dir."""
    index = project_dir / f"{name}.md"
    _write(
        index,
        "---\npaths: ['**/*.rs']\n---\n"
        f"# {name}\nSee [examples](./{name}/examples.md) and the schema.\n",
    )
    _write(project_dir / name / "examples.md", "# Examples\nworked example\n")
    _write(project_dir / name / "schema.json", '{"version": 1}\n')
    return index


def test_release_install_lands_index_and_support_dir(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = _local_multifile_rule(project_dir)
    repo = f"{registry}/{unique_repo}/my-rule"
    runner = grim_at(project_dir)

    out = runner.json("release", str(index), f"{repo}:1.0.0")
    assert out["pushed"] is True
    digest = out["manifest_digest"]
    assert digest.startswith("sha256:")

    # Install the released rule into a clean workspace.
    write_dir = project_dir / "consumer"
    write_dir.mkdir()
    consumer = grim_at(write_dir)
    write_config(write_dir, rules={"my-rule": f"{repo}:1.0.0"})
    consumer.run("lock", check=False)
    rows = consumer.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}

    # Both the index AND the support tree land beside each other.
    rules = write_dir / ".claude/rules"
    assert (rules / "my-rule.md").is_file()
    assert (rules / "my-rule/examples.md").is_file()
    assert (rules / "my-rule/schema.json").is_file()
    assert (rules / "my-rule/examples.md").read_text() == "# Examples\nworked example\n"

    # A no-op reinstall: intact footprint ⇒ nothing to do.
    again = consumer.json("install")["items"]
    assert {r["status"] for r in again} == {"unchanged"}


def test_support_file_edit_is_detected_as_drift(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = _local_multifile_rule(project_dir)
    repo = f"{registry}/{unique_repo}/my-rule"
    runner = grim_at(project_dir)
    runner.json("release", str(index), f"{repo}:1.0.0")

    write_config(project_dir, rules={"my-rule": f"{repo}:1.0.0"})
    runner.run("lock", check=False)
    runner.json("install")

    # Hand-edit a *support* file (not the index): status must flag drift.
    (project_dir / ".claude/rules/my-rule/examples.md").write_text("tampered\n")
    status = runner.json("status")["items"]
    by_name = {r["name"]: r for r in status}
    assert by_name["my-rule"]["state"] == "modified", (
        f"editing a support file must be detected as drift, got {status}"
    )


def test_uninstall_removes_index_and_support_dir(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = _local_multifile_rule(project_dir)
    repo = f"{registry}/{unique_repo}/my-rule"
    runner = grim_at(project_dir)
    runner.json("release", str(index), f"{repo}:1.0.0")

    write_config(project_dir, rules={"my-rule": f"{repo}:1.0.0"})
    runner.run("lock", check=False)
    runner.json("install")

    rules = project_dir / ".claude/rules"
    assert (rules / "my-rule.md").is_file()
    assert (rules / "my-rule").is_dir()

    out = runner.json("uninstall", "rule", "my-rule")
    assert out["status"] == "uninstalled"

    # The index file AND the support directory are both gone.
    assert not (rules / "my-rule.md").exists()
    assert not (rules / "my-rule").exists()

    # Idempotent: a second uninstall is a reported no-op.
    again = runner.json("uninstall", "rule", "my-rule")
    assert again["status"] == "not-installed"


def test_rerelease_is_idempotent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    index = _local_multifile_rule(project_dir)
    repo = f"{registry}/{unique_repo}/my-rule"
    runner = grim_at(project_dir)

    first = runner.json("release", str(index), f"{repo}:1.0.0")
    second = runner.json("release", str(index), f"{repo}:1.0.0")
    assert first["manifest_digest"] == second["manifest_digest"], (
        "re-releasing an identical multi-file rule must yield the same digest"
    )
