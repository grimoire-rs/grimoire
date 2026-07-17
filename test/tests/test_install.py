# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim install` acceptance tests."""
from __future__ import annotations

import json
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

    rows = runner.json("install")["items"]
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


def test_project_install_warns_on_global_scope_shadow(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A project-scope install of a (kind, name) already installed at
    global scope for an overlapping client warns about the shadow —
    both copies are visible to the client, its own precedence decides."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    runner = grim_at(project_dir)

    # Install at global scope first (isolated $HOME; detection falls back
    # to all clients, so the global record covers claude).
    runner.run("add", "--global", sk.fq)

    # Now the same (kind, name) at project scope.
    write_config(project_dir)
    result = runner.run("add", sk.fq)
    assert "also installed at global scope" in result.stderr, (
        f"project install shadowing a global copy must warn; stderr:\n"
        f"{result.stderr}"
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


def test_codex_only_rule_install_skips_before_fetch_when_offline_cold(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Fetch-before-gate (plan C3.3): a Codex-only rule install must compute
    the supporting-client set BEFORE fetching the artifact. Codex declines
    rules, so the install must report `skipped` with zero outputs and never
    touch the network/blob cache — even against a cold offline cache, which
    would otherwise refuse with exit 81 if a fetch were attempted."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)  # online: pins resolved

    # Fresh GRIM_HOME ⇒ blob cache is cold; a real fetch attempt against it
    # while --offline would refuse with exit 81 (see
    # test_offline_cold_cache_blocks_install_exit_81 above).
    runner.env["GRIM_HOME"] = str(project_dir / "cold-home")
    result = runner.run("--offline", "install", "--client", "codex", format="json", check=False)
    assert result.returncode == 0, (
        "a Codex-only rule install must never attempt to fetch the artifact "
        f"(fetch-before-gate, plan C3.3); got rc={result.returncode}; stderr: {result.stderr}"
    )
    rows = json.loads(result.stdout)["items"]
    assert rows[0]["status"] == "skipped", rows
    assert rows[0]["target"] is None, rows
