# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Local path dependencies — declare, lock, install, drift, update.

No registry involved: every source lives on disk and the whole flow runs
with GRIM_OFFLINE=1 to prove path deps never touch the network.
"""
from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

import pytest


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _skill(project_dir: Path, name: str, body: str = "# Body v1\n") -> Path:
    d = project_dir / "skills" / name
    _write(
        d / "SKILL.md",
        f"---\nname: {name}\ndescription: Demo skill.\n---\n{body}",
    )
    return d


def _offline(runner):
    runner.env["GRIM_OFFLINE"] = "1"
    return runner


def _config(project_dir: Path, table: str, name: str, value: str) -> None:
    _write(project_dir / "grimoire.toml", f'[{table}]\n{name} = "{value}"\n')


def test_lock_pins_path_skill_offline(grim_at, project_dir: Path) -> None:
    _skill(project_dir, "my-skill")
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")

    lock = (project_dir / "grimoire.lock").read_text()
    assert 'path = "./skills/my-skill"' in lock
    assert 'hash = "sha256:' in lock
    assert "pinned" not in lock, "a path entry carries no registry pin"


def test_relock_is_byte_identical(grim_at, project_dir: Path) -> None:
    _skill(project_dir, "my-skill")
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    first = (project_dir / "grimoire.lock").read_bytes()
    runner.run("lock")
    assert (project_dir / "grimoire.lock").read_bytes() == first


def test_install_materializes_path_skill_offline(
    grim_at, project_dir: Path
) -> None:
    _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    rendered = project_dir / ".claude" / "skills" / "my-skill" / "SKILL.md"
    assert rendered.is_file()
    assert "# Body v1" in rendered.read_text()


def test_path_rule_with_support_dir(grim_at, project_dir: Path) -> None:
    _write(
        project_dir / "rules" / "house-style.md",
        "---\npaths: ['**/*.rs']\n---\n# Style\nsee ./house-style/x.md\n",
    )
    _write(project_dir / "rules" / "house-style" / "x.md", "# extra\n")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "rules", "house-style", "./rules/house-style.md")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    assert (project_dir / ".claude" / "rules" / "house-style.md").is_file()
    assert (project_dir / ".claude" / "rules" / "house-style" / "x.md").is_file()


def test_path_agent_via_kind_table(grim_at, project_dir: Path) -> None:
    _write(
        project_dir / "agents" / "reviewer.md",
        "---\nname: reviewer\ndescription: Reviews.\n---\nYou review.\n",
    )
    (project_dir / ".claude").mkdir()
    _config(project_dir, "agents", "reviewer", "./agents/reviewer.md")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")
    assert (project_dir / ".claude" / "agents" / "reviewer.md").is_file()


def test_source_edit_flags_outdated_and_update_rerenders(
    grim_at, project_dir: Path
) -> None:
    skill = _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    entry = runner.json("status")["items"][0]
    assert entry["state"] == "installed"
    assert entry["source"] == "path: ./skills/my-skill"
    assert entry["pinned"] is None

    _write(
        skill / "SKILL.md",
        "---\nname: my-skill\ndescription: Demo skill.\n---\n# Body v2\n",
    )
    assert runner.json("status")["items"][0]["state"] == "outdated"

    out = runner.json("update", "--client", "claude")
    assert out["items"][0]["action"] == "updated"
    rendered = project_dir / ".claude" / "skills" / "my-skill" / "SKILL.md"
    assert "# Body v2" in rendered.read_text()
    assert runner.json("status")["items"][0]["state"] == "installed"


def test_update_unchanged_source_reports_unchanged(
    grim_at, project_dir: Path
) -> None:
    _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")
    out = runner.json("update", "--client", "claude")
    assert out["items"][0]["action"] == "unchanged"


def test_missing_source_fails_lock_with_65(grim_at, project_dir: Path) -> None:
    _config(project_dir, "skills", "ghost", "./skills/ghost")
    runner = _offline(grim_at(project_dir))
    result = runner.run("lock", check=False)
    assert result.returncode == 65, result.stderr


def test_mcp_path_value_rejected_65(grim_at, project_dir: Path) -> None:
    _config(project_dir, "mcp", "x", "./mcp/x.toml")
    runner = _offline(grim_at(project_dir))
    result = runner.run("lock", check=False)
    assert result.returncode == 65, result.stderr


def test_parent_relative_source_works(grim_at, tmp_path: Path) -> None:
    # The source lives OUTSIDE the project dir (a sibling checkout).
    project = tmp_path / "project"
    project.mkdir()
    shared = tmp_path / "shared" / "skills" / "my-skill"
    _write(
        shared / "SKILL.md",
        "---\nname: my-skill\ndescription: Shared.\n---\n# Shared\n",
    )
    (project / ".claude").mkdir()
    _write(
        project / "grimoire.toml",
        '[skills]\nmy-skill = "../shared/skills/my-skill"\n',
    )
    runner = _offline(grim_at(project))
    runner.run("lock")
    runner.run("install", "--client", "claude")
    assert (project / ".claude" / "skills" / "my-skill" / "SKILL.md").is_file()


def test_absolute_source_warns_in_project_scope(
    grim_at, project_dir: Path
) -> None:
    skill = _skill(project_dir, "my-skill")
    _config(project_dir, "skills", "my-skill", str(skill))

    runner = _offline(grim_at(project_dir))
    result = runner.run("lock")
    assert "absolute path source" in result.stderr, result.stderr
    lock = (project_dir / "grimoire.lock").read_text()
    assert f'path = "{skill}"' in lock


def test_mixed_registry_and_path_config(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    # A registry entry and a path entry coexist; the registry line keeps
    # its pinned form and the path line its path/hash form.
    from src.helpers import make_artifact

    sk = make_artifact(
        f"{unique_repo}/reg-skill",
        "skill",
        {
            "reg-skill/SKILL.md": (
                "---\nname: reg-skill\ndescription: d\n---\n# reg\n"
            )
        },
        tag="1",
    )
    _skill(project_dir, "my-skill")
    _write(
        project_dir / "grimoire.toml",
        "[skills]\n"
        f'reg-skill = "{sk.fq}"\n'
        'my-skill = "./skills/my-skill"\n',
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = (project_dir / "grimoire.lock").read_text()
    assert sk.digest in lock, "registry entry keeps its manifest-digest pin"
    assert 'path = "./skills/my-skill"' in lock


def test_add_path_from_subdir_writes_config_relative(
    grim_at, project_dir: Path
) -> None:
    _skill(project_dir, "quick-notes")
    (project_dir / ".claude").mkdir()
    _write(project_dir / "grimoire.toml", "[skills]\n")
    sub = project_dir / "skills" / "quick-notes"

    runner = _offline(grim_at(sub))
    out = runner.json("add", "../../skills/quick-notes", "--no-install")
    assert out["kind"] == "skill"
    assert out["name"] == "quick-notes"
    assert out["pinned"].startswith("./skills/quick-notes@sha256:")

    cfg = (project_dir / "grimoire.toml").read_text()
    assert 'quick-notes = "./skills/quick-notes"' in cfg


def test_add_path_conflict_with_registry_binding_is_64(
    grim_at, project_dir: Path
) -> None:
    _skill(project_dir, "my-skill")
    _write(
        project_dir / "grimoire.toml",
        '[skills]\nmy-skill = "ghcr.io/acme/my-skill:1"\n',
    )
    runner = _offline(grim_at(project_dir))
    result = runner.run("add", "./skills/my-skill", check=False)
    assert result.returncode == 64, result.stderr


def test_remove_and_uninstall_path_dep(grim_at, project_dir: Path) -> None:
    _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    runner.run("uninstall", "skill", "my-skill")
    assert not (project_dir / ".claude" / "skills" / "my-skill").exists()
    assert "my-skill" not in (project_dir / "grimoire.toml").read_text()


def test_path_dep_installs_into_all_detected_clients(
    grim_at, project_dir: Path
) -> None:
    # One local source fans out into every detected vendor's project dir —
    # the multi-vendor render engine sits downstream of the source branch.
    _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    (project_dir / ".opencode").mkdir()
    _write(project_dir / ".github" / "copilot-instructions.md", "# ci\n")
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install")

    for out in (
        project_dir / ".claude" / "skills" / "my-skill" / "SKILL.md",
        project_dir / ".opencode" / "skills" / "my-skill" / "SKILL.md",
        project_dir / ".github" / "skills" / "my-skill" / "SKILL.md",
    ):
        assert out.is_file(), f"missing vendor output: {out}"


def test_global_scope_path_dep(grim_at, grim_home: Path, tmp_path: Path) -> None:
    # A personal skill declared in the GLOBAL config via an absolute path
    # (machine-local file — no portability warning expected there). No
    # --client: nothing detected in the isolated home, so the install
    # falls back to ALL clients' native user-level dirs.
    shared = tmp_path / "dotfiles" / "skills" / "my-skill"
    _write(
        shared / "SKILL.md",
        "---\nname: my-skill\ndescription: Personal.\n---\n# Mine\n",
    )
    grim_home.mkdir(parents=True, exist_ok=True)
    _write(grim_home / "grimoire.toml", f'[skills]\nmy-skill = "{shared}"\n')

    runner = _offline(grim_at(tmp_path))
    result = runner.run("--global", "lock")
    assert "absolute path source" not in result.stderr
    runner.run("--global", "install")
    for out in (
        runner.home / ".claude" / "skills" / "my-skill" / "SKILL.md",
        runner.home / ".config" / "opencode" / "skills" / "my-skill" / "SKILL.md",
        runner.home / ".copilot" / "skills" / "my-skill" / "SKILL.md",
    ):
        assert out.is_file(), f"missing vendor output: {out}"


def test_declared_install_state_record_has_no_dev_marker(
    grim_at, project_dir: Path
) -> None:
    # F1: a normal declared (path-sourced) install writes `dev: false` —
    # checked via the PARSED state.json, not a substring match. `dev` is
    # omitted from the wire when false (`skip_serializing_if`), so absence
    # counts as `false` too.
    _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    state = json.loads((project_dir / ".grimoire" / "state.json").read_text())
    records = state["records"]
    assert len(records) == 1, f"exactly one record expected: {records}"
    assert records[0]["name"] == "my-skill"
    assert records[0].get("dev", False) is False


def test_bare_install_refuses_when_source_content_drifted_since_lock(
    grim_at, project_dir: Path
) -> None:
    # F5: a bare `grim install` (not `update`) must fail-closed with exit
    # 65 when the locked path source's content drifted since `grim lock`
    # wrote its pin — never silently install stale (or worse, mismatched)
    # content.
    skill = _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")

    _write(
        skill / "SKILL.md",
        "---\nname: my-skill\ndescription: Demo skill.\n---\n# Body v2\n",
    )
    result = runner.run("install", "--client", "claude", check=False)
    assert result.returncode == 65, result.stderr
    assert "changed since the lock was written" in result.stderr


def test_bare_install_refuses_when_source_missing(
    grim_at, project_dir: Path
) -> None:
    # F5: a bare `grim install` must fail-closed with exit 65 when the
    # locked path source no longer exists at all.
    skill = _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")

    shutil.rmtree(skill)
    result = runner.run("install", "--client", "claude", check=False)
    assert result.returncode == 65, result.stderr


def test_declared_path_status_flags_deleted_source_as_problem(
    grim_at, project_dir: Path
) -> None:
    # F6: a DECLARED (non-dev) path skill whose source is deleted after the
    # lock+install must surface a problem in `grim status` — never a clean
    # `installed`. `path_source_drifted`'s Err arm reads the vanished source
    # as drift (outdated), mirroring the dev arm. Read-only status stays
    # exit-0 (state is data).
    skill = _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    shutil.rmtree(skill)

    entry = runner.json("status")["items"][0]
    assert entry["name"] == "my-skill"
    assert entry["state"] != "installed", (
        "a deleted declared path source must surface a problem "
        "(outdated/missing), not report installed"
    )


@pytest.mark.skipif(
    sys.platform == "win32", reason="POSIX symlink-skip semantics (CWE-59)"
)
def test_symlinked_out_of_tree_secret_never_installed(
    grim_at, project_dir: Path
) -> None:
    # F4 (CWE-59): a symlink inside a path skill dir pointing at an
    # out-of-tree secret must never be packed/installed — the symlink-skip
    # in `collect_files` is the sole barrier against exfiltrating a victim's
    # secrets via a symlink in a cloned repo. The skill installs (offline),
    # but the secret's content must be absent from the whole client tree.
    secret_marker = "TOP-SECRET-EXFIL-XYZZY"
    secret = project_dir / "outside" / "secret.txt"
    _write(secret, secret_marker)

    skill = _skill(project_dir, "my-skill")
    (skill / "leak.txt").symlink_to(secret)
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    out_dir = project_dir / ".claude" / "skills" / "my-skill"
    assert (out_dir / "SKILL.md").is_file(), "the skill itself must install"
    assert not (out_dir / "leak.txt").exists(), "the symlink must not be installed"
    leaked = [
        p
        for p in out_dir.rglob("*")
        if p.is_file() and secret_marker in p.read_text(errors="ignore")
    ]
    assert not leaked, f"secret content leaked into installed files: {leaked}"
