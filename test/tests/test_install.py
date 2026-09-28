# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim install` acceptance tests."""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

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

    # Install at global scope first; the isolated $HOME needs a detected
    # Claude for the global record to overlap the project one below.
    (runner.home / ".claude").mkdir(parents=True, exist_ok=True)
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



def _declare(project_dir: Path, table: str, key: str, ref: str) -> None:
    (project_dir / "grimoire.toml").write_text(
        f'[options]\nclients = ["claude"]\n\n[{table}]\n"{key}" = "{ref}"\n'
    )


@pytest.mark.parametrize(
    "key", ["../../../escaped", "a/../../../../escaped", "/abs", "C:\\\\x", "a//b"]
)
def test_traversal_declaration_key_is_refused_before_any_write(
    grim_at, project_dir: Path, registry: str, unique_repo: str, key: str
) -> None:
    """Issue #90: a key with a traversal-capable part exits 65 and writes nothing."""
    sk = make_artifact(
        f"{unique_repo}/harmless",
        "skill",
        {"harmless/SKILL.md": "---\nname: harmless\n---\n# H\n"},
        tag="1",
    )
    _declare(project_dir, "skills", key, sk.fq)
    runner = grim_at(project_dir)

    for command in (("lock",), ("install", "--client", "claude")):
        result = runner.run(*command, check=False)
        assert result.returncode == 65, (command, result.stderr)

    assert not (project_dir.parent / "escaped").exists()


NESTED_KEYS = [
    ("rules", "rule", "team/style", ".claude/rules/team/style.md"),
    ("skills", "skill", "team/skill", ".claude/skills/team/skill/SKILL.md"),
]
if sys.platform != "win32":
    # `x:y` is drive-relative on Windows, so it is refused there.
    NESTED_KEYS.append(("skills", "skill", "x:y", ".claude/skills/x:y/SKILL.md"))


@pytest.mark.parametrize(("table", "kind", "key", "dest"), NESTED_KEYS)
def test_keys_released_grim_installs_keep_installing_where_they_did(
    grim_at, project_dir: Path, registry: str, unique_repo: str,
    table: str, kind: str, key: str, dest: str,
) -> None:
    """Issue #90 review B1 (Principle 9): nested and colon keys install,
    report, and uninstall exactly as on released grim."""
    files = (
        {"style.md": "---\npaths: ['**/*.rs']\n---\n# style\n"}
        if kind == "rule"
        else {"leaf/SKILL.md": "---\nname: leaf\n---\n# leaf\n"}
    )
    art = make_artifact(f"{unique_repo}/leaf-{kind}", kind, files, tag="1")
    _declare(project_dir, table, key, art.fq)
    runner = grim_at(project_dir)
    runner.run("lock")

    rows = runner.json("install", "--client", "claude")["items"]
    assert {r["status"] for r in rows} == {"installed"}, rows
    assert (project_dir / dest).is_file()

    status = {r["name"]: r for r in runner.json("status")["items"]}
    assert status[key]["state"] == "installed", status

    out = runner.json("uninstall", kind, key)
    assert out["status"] == "uninstalled", out
    assert not (project_dir / dest).exists()


def test_traversal_lock_entry_name_is_refused_with_78(
    grim_at, project_dir: Path
) -> None:
    """Issue #90: a hand-edited lock naming a traversal is refused on load
    (78, the lock tier's class) before install writes anything."""
    (project_dir / "grimoire.toml").write_text('[options]\nclients = ["claude"]\n')
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"
    digest = "sha256:" + "a" * 64
    entry = f'skill = [{{ name = "../../../escaped", pinned = "localhost:5000/x/y@{digest}" }}]'
    body = lock.read_text()
    assert "skill = []" in body, body
    lock.write_text(body.replace("skill = []", entry, 1))

    result = runner.run("install", "--client", "claude", check=False)
    assert result.returncode == 78, result.stderr
    assert "escaped" in result.stderr, result.stderr
    assert not (project_dir.parent / "escaped").exists()
