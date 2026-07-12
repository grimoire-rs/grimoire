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
    # Declared forward-slash (`as_posix`): a TOML basic string treats `\U`
    # as a unicode escape, and the config grammar is forward-slash-only.
    skill = _skill(project_dir, "my-skill")
    _config(project_dir, "skills", "my-skill", skill.as_posix())

    runner = _offline(grim_at(project_dir))
    result = runner.run("lock")
    assert "absolute path source" in result.stderr, result.stderr
    lock = (project_dir / "grimoire.lock").read_text()
    assert f'path = "{skill.as_posix()}"' in lock


def test_mixed_registry_and_path_config(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # `registry` fixture: skips (not errors) when no registry is reachable
    # (e.g. the Windows CI runner, which cannot host the Linux registry
    # container).
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
    # `as_posix`: config values are forward-slash-only (and `\U` in a TOML
    # basic string is a unicode escape).
    _write(grim_home / "grimoire.toml", f'[skills]\nmy-skill = "{shared.as_posix()}"\n')

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


# ---------------------------------------------------------------------------
# Local-path bundles: a `[bundles]` value points at a local bundle-source
# TOML whose members are ordinary registry refs (plan_local_bundles_tui_group).
# ---------------------------------------------------------------------------


def test_local_bundle_lock_writes_path_hash_entry_and_installs_members(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    from src.helpers import make_artifact

    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n"},
        tag="stable",
    )
    _write(project_dir / "bundles" / "x.toml", f'[skills]\ncode-review = "{sk.fq}"\n')
    _config(project_dir, "bundles", "x", "./bundles/x.toml")
    (project_dir / ".claude").mkdir()

    runner = grim_at(project_dir)
    runner.run("lock")

    lock = (project_dir / "grimoire.lock").read_text()
    assert "[[bundle]]" in lock
    assert 'path = "./bundles/x.toml"' in lock
    assert 'hash = "sha256:' in lock
    # The bundle entry took the path arm, not the registry arm — and this
    # bundle has exactly one member/contributor, so the member's own
    # provenance uses the legacy `bundle`/`bundle_tag` pair (never the
    # `bundles = [{ repo = ..., tag = ... }]` array), so a bare `repo = "`
    # can only ever come from a registry-arm bundle entry.
    assert 'repo = "' not in lock, f"a local bundle must not emit a repo key: {lock}"
    assert sk.digest in lock, "the member keeps its own registry pin"

    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}
    installed = project_dir / ".claude" / "skills" / "code-review" / "SKILL.md"
    assert installed.is_file()

    status_rows = runner.json("status")["items"]
    member = next(r for r in status_rows if r["name"] == "code-review")
    assert member["state"] == "installed"
    bundle_row = next(r for r in status_rows if r["kind"] == "bundle")
    assert bundle_row["name"] == "x"


def test_local_bundle_update_after_editing_members_rolls_lock_forward(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    from src.helpers import make_artifact

    first = make_artifact(
        f"{unique_repo}/first",
        "skill",
        {"first/SKILL.md": "---\nname: first\n---\n# one\n"},
        tag="stable",
    )
    _write(project_dir / "bundles" / "x.toml", f'[skills]\nfirst = "{first.fq}"\n')
    _config(project_dir, "bundles", "x", "./bundles/x.toml")

    runner = grim_at(project_dir)
    runner.run("lock")
    lock_before = (project_dir / "grimoire.lock").read_text()
    assert "first" in lock_before
    assert "second" not in lock_before

    second = make_artifact(
        f"{unique_repo}/second",
        "skill",
        {"second/SKILL.md": "---\nname: second\n---\n# two\n"},
        tag="stable",
    )
    _write(
        project_dir / "bundles" / "x.toml",
        f'[skills]\nfirst = "{first.fq}"\nsecond = "{second.fq}"\n',
    )
    runner.run("update")

    lock_after = (project_dir / "grimoire.lock").read_text()
    assert lock_after != lock_before, "editing the member set must roll the lock forward"
    assert "second" in lock_after
    assert second.digest in lock_after


def test_local_bundle_relative_member_rejected_65(
    grim_at, project_dir: Path
) -> None:
    # ADR sub-decision 5: a local bundle has no registry directory to
    # late-bind a `./`/`../` member against — relative members are only
    # valid inside a REGISTRY bundle.
    _write(project_dir / "bundles" / "x.toml", '[skills]\nfoo = "./y:1"\n')
    _config(project_dir, "bundles", "x", "./bundles/x.toml")

    runner = _offline(grim_at(project_dir))
    result = runner.run("lock", check=False)
    assert result.returncode == 65, result.stderr
    message = result.stderr.lower()
    assert "relative" in message or "absolute" in message, (
        f"error must clearly name the relative-member problem: {result.stderr}"
    )


def test_local_bundle_traversal_member_name_rejected_65(
    grim_at, project_dir: Path
) -> None:
    # CWE-22: a local bundle member's config table KEY becomes an install-path
    # component. A traversal key ("../../evil") must be rejected at `grim lock`
    # (exit 65) before it can materialize outside the client dir — the same
    # `SkillName` guard the registry branch enforces on member names.
    _write(
        project_dir / "bundles" / "x.toml",
        '[skills]\n"../../evil" = "ghcr.io/acme/code-review:1"\n',
    )
    _config(project_dir, "bundles", "x", "./bundles/x.toml")
    (project_dir / ".claude").mkdir()

    runner = _offline(grim_at(project_dir))
    result = runner.run("lock", check=False)
    assert result.returncode == 65, result.stderr

    # Fail-closed: the traversal member never materialized anywhere in or above
    # the project (the guard fires before any file is written).
    for candidate in (project_dir / "evil", project_dir.parent / "evil"):
        assert not candidate.exists(), f"traversal target must never be written: {candidate}"


def test_local_bundle_offline_reinstall_is_network_free(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # After an online lock+install, a bare offline `grim install` with the
    # output INTACT must succeed without any network access — proving the
    # local-bundle install path adds no new network dependency (it does not
    # re-expand/re-resolve the bundle online). Mirrors
    # test_install.py::test_offline_warm_blob_cache_succeeds.
    #
    # NOTE: offline RE-materialize AFTER deleting the output would require a
    # manifest cache grim keeps for NO artifact kind (registry or local) — a
    # general v1 limitation, not local-bundle-specific and out of scope here.
    from src.helpers import make_artifact

    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n"},
        tag="stable",
    )
    _write(project_dir / "bundles" / "x.toml", f'[skills]\ncode-review = "{sk.fq}"\n')
    _config(project_dir, "bundles", "x", "./bundles/x.toml")
    (project_dir / ".claude").mkdir()

    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install", "--client", "claude")

    installed = project_dir / ".claude" / "skills" / "code-review" / "SKILL.md"
    assert installed.is_file()

    # Output intact: a bare offline reinstall is a network-free re-verify no-op.
    offline_runner = _offline(grim_at(project_dir))
    result = offline_runner.run("install", "--client", "claude", check=False)
    assert result.returncode == 0, (
        f"offline reinstall of a local bundle must succeed network-free, got "
        f"{result.returncode}; {result.stderr}"
    )
    assert installed.is_file(), "offline reinstall must keep the member installed"


def test_registry_only_lock_has_no_local_bundle_wire_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Compat contract: a project declaring no local (path-sourced) bundle
    # must never see `path`/`hash` bundle keys or a `[[bundle]]` section at
    # all — the frozen registry-only lock shape stays byte-identical.
    from src.helpers import make_artifact

    sk = make_artifact(
        f"{unique_repo}/s",
        "skill",
        {"s/SKILL.md": "---\nname: s\n---\n"},
        tag="stable",
    )
    _write(project_dir / "grimoire.toml", f'[skills]\ns = "{sk.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock")

    lock = (project_dir / "grimoire.lock").read_text()
    assert "[[bundle]]" not in lock
    # `[metadata].declaration_hash = "sha256:…"` always contains "hash =", so a
    # bare `"hash =" not in lock` false-positives. A real top-level bundle
    # `path`/`hash` key starts a line — assert the leading-newline forms so the
    # check targets only top-level keys, not the metadata field.
    assert "\npath = " not in lock
    assert "\nhash = " not in lock


def test_add_local_bundle_is_rejected_with_guidance(
    grim_at, project_dir: Path
) -> None:
    # v1 descope: a local bundle's supported path is a declared `[bundles]`
    # entry resolved by `grim lock`, not `grim add`. The reject is a usage
    # error (64) whose message guides the user to the supported flow.
    _write(project_dir / "bundles" / "x.toml", '[skills]\ncode-review = "ghcr.io/acme/code-review:1"\n')
    _write(project_dir / "grimoire.toml", "[bundles]\n")

    runner = _offline(grim_at(project_dir))
    result = runner.run("add", "./bundles/x.toml", "--kind", "bundle", "--name", "x", check=False)
    assert result.returncode == 64, result.stderr
    assert "[bundles]" in result.stderr, result.stderr
    assert "grim lock" in result.stderr, result.stderr

    # The reject leaves the config untouched — no binding written.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "./bundles/x.toml" not in cfg


def test_remove_local_bundle_evicts_member_from_lock_and_install_does_not_resurrect_it(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # B1 (post /swarm-review 2026-07-11, plan_local_bundles_tui_group.md):
    # `legacy_drop_from_lock` skipped `evict_bundle_members` for a path
    # bundle (no `(repo, tag)` to key eviction on), dropped the `[[bundle]]`
    # snapshot, and restamped `declaration_hash` fresh anyway — the lock
    # read as current while STILL listing a member only that bundle
    # provided, so a following `grim install` re-materialized the removed
    # bundle. The lock must never list a member no surviving declaration
    # provides under a fresh hash, and a following `grim install` must
    # never bring it back. (Member *files* already on disk may remain —
    # out of scope; this test proves the lock is truthful and a REINSTALL
    # cannot resurrect the file, not that the pre-existing file vanishes.)
    from src.helpers import make_artifact

    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n"},
        tag="stable",
    )
    _write(project_dir / "bundles" / "x.toml", f'[skills]\ncode-review = "{sk.fq}"\n')
    _config(project_dir, "bundles", "x", "./bundles/x.toml")
    (project_dir / ".claude").mkdir()

    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install", "--client", "claude")

    installed = project_dir / ".claude" / "skills" / "code-review" / "SKILL.md"
    assert installed.is_file()

    result = runner.run("remove", "bundle", "x")
    assert result.returncode == 0, result.stderr

    cfg = (project_dir / "grimoire.toml").read_text()
    assert '"./bundles/x.toml"' not in cfg, "the bundle declaration must be dropped"

    lock = (project_dir / "grimoire.lock").read_text()
    assert "[[bundle]]" not in lock, "the path-bundle cache entry must be dropped"
    assert 'name = "code-review"' not in lock, (
        "the lock must not retain a member that only the removed bundle "
        f"provided:\n{lock}"
    )

    # Simulate a fresh checkout: with the member's own lock entry gone, a
    # following `grim install` must NOT bring the file back — nothing in
    # the (now-truthful) lock names it any more.
    installed.unlink()
    runner.run("install", "--client", "claude")
    assert not installed.exists(), (
        "grim install must not resurrect a member the removed bundle used to provide"
    )


def test_local_bundle_status_shows_path_source_not_direct(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # B3 (post /swarm-review 2026-07-11): `grim status`'s declared-bundle
    # loop hardcodes `source = "direct"` for every bundle row, even a local
    # (path-declared) one. A local bundle's row must mirror the
    # skill/rule/agent loop and report its path source instead.
    from src.helpers import make_artifact

    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\n---\n# CR\n"},
        tag="stable",
    )
    _write(project_dir / "bundles" / "x.toml", f'[skills]\ncode-review = "{sk.fq}"\n')
    _config(project_dir, "bundles", "x", "./bundles/x.toml")

    runner = grim_at(project_dir)
    runner.run("lock")

    items = runner.json("status")["items"]
    bundle_row = next(r for r in items if r["kind"] == "bundle")
    assert bundle_row["source"] == "path: ./bundles/x.toml", (
        f"a local bundle's status row must report its path source, not "
        f"{bundle_row['source']!r}"
    )

    plain = runner.plain("status").stdout
    assert "path: ./bundles/x.toml" in plain, (
        f"the plain status table must show the bundle's path source too:\n{plain}"
    )


def test_relative_out_of_tree_path_source_warns_security(
    grim_at, tmp_path: Path
) -> None:
    # B4 (post /swarm-review 2026-07-11): `warn_untrusted_path_sources` only
    # checked `path.is_absolute()`, so a RELATIVE escape out of the
    # workspace (e.g. `../../outside-skill`) never warned even though it
    # reads a file the workspace boundary does not contain. Fix decision:
    # extend the warning to relative out-of-tree escapes, reframed as a
    # SECURITY message (posture: DOCUMENT — no new error path, exit stays
    # 0; ADR sub-decision 3's trust model — absolute + relative both
    # allowed — is unchanged).
    project = tmp_path / "nested" / "project"
    outside = tmp_path / "outside-skill"
    _write(
        outside / "SKILL.md",
        "---\nname: outside-skill\ndescription: Outside.\n---\n# Outside\n",
    )
    _write(
        project / "grimoire.toml",
        '[skills]\noutside-skill = "../../outside-skill"\n',
    )

    runner = _offline(grim_at(project))
    result = runner.run("lock")
    assert result.returncode == 0, result.stderr

    message = result.stderr.lower()
    assert "security" in message, (
        f"a relative path source that escapes the workspace must warn with "
        f"a SECURITY-framed message: {result.stderr}"
    )
    assert "outside" in message or "workspace" in message, (
        f"the warning must name that the source resolves outside the "
        f"workspace: {result.stderr}"
    )


def test_update_named_path_entry_is_not_rejected_as_undeclared(
    grim_at, project_dir: Path
) -> None:
    # W4 coverage: `resolve_lock_partial`'s per-name guard
    # (`all_work.iter().any(...) || has_path_entry(set, name)`) must accept
    # a path-sourced binding name in a NAMED `grim update <name>` — it must
    # not be rejected with `TagNotFound` ("tag not found") just because a
    # path entry produces no registry work item. May already pass today
    # (the `has_path_entry` guard already exists) — coverage, not proven
    # regression, until run.
    _skill(project_dir, "my-skill")
    (project_dir / ".claude").mkdir()
    _config(project_dir, "skills", "my-skill", "./skills/my-skill")

    runner = _offline(grim_at(project_dir))
    runner.run("lock")
    runner.run("install", "--client", "claude")

    result = runner.run("update", "my-skill", "--client", "claude", check=False)
    assert result.returncode == 0, result.stderr
    assert "tag not found" not in result.stderr.lower(), result.stderr
