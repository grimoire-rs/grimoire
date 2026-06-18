# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Client-set desync + defensive-tolerance acceptance tests.

The install state records one `ClientOutput` per client targeted *at install
time*. Changing which clients are active afterward (removing a client dir,
adding one, or a vendor config that no longer parses) must not poison status,
silently skip an install, or hard-fail a command whose primary action already
succeeded. These tests pin the end-to-end behavior the user reported broken.
"""
from __future__ import annotations

import shutil
from pathlib import Path

from src.helpers import make_artifact, make_bundle


def _write_config_with_clients(
    project_dir: Path,
    *,
    skills: dict[str, str] | None = None,
    rules: dict[str, str] | None = None,
    bundles: dict[str, str] | None = None,
    clients: list[str] | None = None,
) -> None:
    """Write a grimoire.toml, optionally with an `[options].clients` array."""
    lines: list[str] = []
    if clients is not None:
        joined = ", ".join(f'"{c}"' for c in clients)
        lines.append("[options]")
        lines.append(f"clients = [{joined}]")
        lines.append("")
    if bundles:
        lines.append("[bundles]")
        for name, ref in bundles.items():
            lines.append(f'{name} = "{ref}"')
    lines.append("[skills]")
    for name, ref in (skills or {}).items():
        lines.append(f'{name} = "{ref}"')
    lines.append("[rules]")
    for name, ref in (rules or {}).items():
        lines.append(f'{name} = "{ref}"')
    (project_dir / "grimoire.toml").write_text("\n".join(lines) + "\n")


def test_bundle_status_ignores_removed_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Headline regression (the user's exact report).

    Install a bundle targeting claude + opencode, then remove the opencode
    client directory. `grim status` must still report every member (and the
    bundle) installed, because every *currently-active* client (claude) has
    intact files — the removed client's stale output must not flag `missing`.
    """
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
    bundle = make_bundle(
        f"{unique_repo}/starter",
        [("skill", "code-review", sk.fq), ("rule", "rust-style", ru.fq)],
        tag="latest",
    )
    _write_config_with_clients(
        project_dir, bundles={"starter": bundle.fq}, clients=["claude", "opencode"]
    )

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows
    # Both client layouts received the members.
    assert (project_dir / ".claude/skills/code-review/SKILL.md").is_file()
    assert (project_dir / ".opencode/skills/code-review").is_dir()

    # The user disables the opencode client.
    shutil.rmtree(project_dir / ".opencode")

    status = runner.json("status")
    # Every currently-active client (claude) has intact files ⇒ nothing is
    # missing or modified. Before the fix, the stale opencode outputs made
    # the members read `missing`.
    bad = [r for r in status if r["state"] not in ("installed",)]
    assert not bad, f"removed-client outputs must not poison status: {bad}"


def test_reinstall_after_adding_client_materializes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Adding a client then re-installing must materialize for the new client.

    Install claude-only; later add `.opencode/`; a plain `grim install` (which
    now detects claude + opencode) must write the opencode output instead of
    short-circuiting on `AlreadyInstalled`.
    """
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    _write_config_with_clients(project_dir, rules={"rust-style": ru.fq})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.json("install", "--client", "claude")
    assert (project_dir / ".claude/rules/rust-style.md").is_file()
    assert not (project_dir / ".opencode/rules/rust-style.md").exists()

    # The user enables the opencode client and re-installs (no --client ⇒
    # detection now includes opencode because `.opencode/` is present).
    (project_dir / ".opencode").mkdir()
    runner.run("install", check=False)

    assert (project_dir / ".opencode/rules/rust-style.md").is_file(), (
        "re-install after adding a client must materialize for the new client"
    )
    # The original client output is untouched.
    assert (project_dir / ".claude/rules/rust-style.md").is_file()


def test_install_succeeds_with_unreadable_vendor_config(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An unparseable opencode.json must not fail an install whose files and
    state already persisted — the managed-glob registration is skipped and
    warned, the primary action succeeds (C8 + D4)."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    _write_config_with_clients(project_dir, rules={"rust-style": ru.fq})
    # A config grim cannot parse — registration must be skipped, never clobbered.
    garbage = "not json at all {{{"
    (project_dir / "opencode.json").write_text(garbage)

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    result = runner.run("install", "--client", "opencode", check=False)

    assert result.returncode == 0, (
        f"install must succeed despite an unparseable vendor config: {result.stderr}"
    )
    assert (project_dir / ".opencode/rules/rust-style.md").is_file()
    # The unparseable config is never rewritten (D4: don't clobber on add).
    assert (project_dir / "opencode.json").read_text() == garbage


def test_uninstall_tolerates_missing_target_files(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Deleting the materialized files by hand must not stop `uninstall` from
    converging on "not installed" — the record is dropped, exit 0."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    _write_config_with_clients(project_dir, skills={"code-review": sk.fq})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.json("install", "--client", "claude")

    # The user deletes the materialized files by hand.
    shutil.rmtree(project_dir / ".claude/skills/code-review")

    result = runner.run("uninstall", "skill", "code-review", check=False)
    assert result.returncode == 0, result.stderr
    out = runner.json("status")
    assert all(r["name"] != "code-review" for r in out), "record must be dropped"
