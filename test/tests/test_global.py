# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`--global` scope acceptance tests.

The global scope operates on ``$GRIM_HOME/grimoire.toml`` and its own
lock, fully independent of any project config (the two are never merged).

Global installs land in vendor-native user-level discovery paths:
- Claude:    skills → ``$HOME/.claude/skills/<name>/``
             rules  → ``$HOME/.claude/rules/<name>.md``
- OpenCode:  skills → ``$XDG_CONFIG_HOME/opencode/skills/<name>/`` (default ``$HOME/.config/opencode/skills/``)
             rules  → ``$GRIM_HOME/.opencode/rules/<name>.md`` (loaded via absolute glob in
                       ``$XDG_CONFIG_HOME/opencode/opencode.json``)
- Copilot:   skills → ``$HOME/.copilot/skills/<name>/``
             rules  → ``$HOME/.copilot/instructions/<name>.instructions.md``

Vendor env-var overrides (tested at the bottom of this file):
- ``CLAUDE_CONFIG_DIR`` replaces the entire ``~/.claude`` tree (skills + rules)
- ``OPENCODE_CONFIG_DIR`` is the preferred skills install target over the XDG default
- ``COPILOT_HOME`` replaces ``~/.copilot`` for Copilot skills
- ``OPENCODE_CONFIG`` is the global ``opencode.json`` edit target (file path)
- ``XDG_CONFIG_HOME`` drives the OpenCode skills root and config location
- empty values are treated as unset
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact
from src.registry import retag
from src.runner import GrimRunner


def test_global_scope_is_independent_of_project(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/global-rule"
    ru = make_artifact(
        repo,
        "rule",
        {"global-rule.md": "---\npaths: ['**']\n---\n# global\n"},
        tag="v1",
    )
    # Global config under $GRIM_HOME, no project config anywhere.
    (grim_home / "grimoire.toml").write_text(
        f'[rules]\nglobal-rule = "{ru.fq}"\n'
    )
    runner = GrimRunner(grim_binary, grim_home)
    # Claude must be *detected* globally for the default target set to
    # include it; without any marker grim resolves to the generic `agents`
    # client (skills-only pool).
    (runner.home / ".claude").mkdir(parents=True, exist_ok=True)

    lock_rows = runner.json("lock", "--global")["items"]
    assert lock_rows[0]["name"] == "global-rule"
    assert (grim_home / "grimoire.lock").is_file()
    assert "@sha256:" in (grim_home / "grimoire.lock").read_text()

    install_rows = runner.json("install", "--global")["items"]
    assert install_rows[0]["status"] == "installed"
    # Global Claude rules land in the vendor-native ~/.claude/rules/ path,
    # not under $GRIM_HOME/.claude/.
    assert (runner.home / ".claude/rules/global-rule.md").is_file(), (
        "global Claude rule must materialize in $HOME/.claude/rules/"
    )
    assert not (grim_home / ".claude/rules/global-rule.md").exists(), (
        "global rule must NOT land under $GRIM_HOME/.claude/ (old layout)"
    )

    status_rows = runner.json("status", "--global")["items"]
    assert status_rows[0]["state"] == "installed"


def test_global_install_without_lock_exits_79(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/r"
    ru = make_artifact(repo, "rule", {"r.md": "# r\n"}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[rules]\nr = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)

    result = runner.run("install", "--global", check=False)
    assert result.returncode == 79, (
        f"global install without a lock must exit 79, got "
        f"{result.returncode}; {result.stderr}"
    )


def test_global_install_claude_skill_lands_in_home_dot_claude(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed Claude skill materializes in ``$HOME/.claude/skills/``."""
    repo = f"{unique_repo}/my-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"my-skill/SKILL.md": "---\nname: my-skill\ndescription: test skill\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nmy-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    (runner.home / ".claude").mkdir(parents=True, exist_ok=True)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global")["items"]
    assert install_rows[0]["status"] == "installed"

    skill_dir = runner.home / ".claude/skills/my-skill"
    assert skill_dir.is_dir(), (
        f"global Claude skill must materialize in $HOME/.claude/skills/; got nothing at {skill_dir}"
    )
    assert (skill_dir / "SKILL.md").is_file()
    # Must NOT land under $GRIM_HOME/.claude/ (old layout).
    assert not (grim_home / ".claude/skills/my-skill").exists(), (
        "global skill must NOT land under $GRIM_HOME/.claude/ (old layout)"
    )


def test_global_install_claude_rule_lands_in_home_dot_claude(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed Claude rule materializes in ``$HOME/.claude/rules/``."""
    repo = f"{unique_repo}/my-rule"
    ru = make_artifact(
        repo,
        "rule",
        {"my-rule.md": "---\npaths: ['**/*.rs']\n---\n# Rust style\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\nmy-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    # Claude must be *detected* globally for the default target set to
    # include it; without any marker grim resolves to the generic `agents`
    # client (skills-only pool).
    (runner.home / ".claude").mkdir(parents=True, exist_ok=True)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global")["items"]
    assert install_rows[0]["status"] == "installed"

    rule_file = runner.home / ".claude/rules/my-rule.md"
    assert rule_file.is_file(), (
        f"global Claude rule must materialize in $HOME/.claude/rules/; got nothing at {rule_file}"
    )
    content = rule_file.read_text()
    assert "Rust style" in content


def test_global_install_opencode_skill_lands_in_xdg_config(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed OpenCode skill materializes in ``$XDG_CONFIG_HOME/opencode/skills/``."""
    repo = f"{unique_repo}/oc-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"oc-skill/SKILL.md": "---\nname: oc-skill\ndescription: opencode skill\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\noc-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert install_rows[0]["status"] == "installed"

    # Skills go to $XDG_CONFIG_HOME/opencode/skills/ (set to $HOME/.config by runner)
    skill_dir = runner.home / ".config/opencode/skills/oc-skill"
    assert skill_dir.is_dir(), (
        f"global OpenCode skill must materialize in $XDG_CONFIG_HOME/opencode/skills/; "
        f"got nothing at {skill_dir}"
    )
    assert (skill_dir / "SKILL.md").is_file()


def test_global_install_opencode_rule_stays_in_grim_home_and_registers_glob(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed OpenCode rule writes to ``$GRIM_HOME/.opencode/rules/``
    and registers an absolute glob in ``$XDG_CONFIG_HOME/opencode/opencode.json``."""
    repo = f"{unique_repo}/oc-rule"
    ru = make_artifact(
        repo,
        "rule",
        {"oc-rule.md": "---\npaths: ['**']\n---\n# OpenCode rule\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\noc-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert install_rows[0]["status"] == "installed"

    # Rule file stays under $GRIM_HOME/.opencode/rules/ (loaded via absolute glob)
    rule_file = grim_home / ".opencode/rules/oc-rule.md"
    assert rule_file.is_file(), (
        f"global OpenCode rule must stay in $GRIM_HOME/.opencode/rules/; "
        f"got nothing at {rule_file}"
    )

    # The absolute glob must be registered in $XDG_CONFIG_HOME/opencode/opencode.json
    opencode_cfg = runner.home / ".config/opencode/opencode.json"
    assert opencode_cfg.is_file(), (
        f"opencode.json must be created at $XDG_CONFIG_HOME/opencode/opencode.json; "
        f"got nothing at {opencode_cfg}"
    )
    cfg = json.loads(opencode_cfg.read_text())
    instructions = cfg.get("instructions", [])
    assert any(str(grim_home) in entry for entry in instructions), (
        f"opencode.json instructions must contain an absolute glob pointing at $GRIM_HOME; "
        f"instructions={instructions}"
    )


def test_global_install_copilot_skill_lands_in_home_dot_copilot(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed Copilot skill materializes in ``$HOME/.copilot/skills/``."""
    repo = f"{unique_repo}/cp-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"cp-skill/SKILL.md": "---\nname: cp-skill\ndescription: copilot skill\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncp-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "copilot")["items"]
    assert install_rows[0]["status"] == "installed"

    skill_dir = runner.home / ".copilot/skills/cp-skill"
    assert skill_dir.is_dir(), (
        f"global Copilot skill must materialize in $HOME/.copilot/skills/; "
        f"got nothing at {skill_dir}"
    )
    assert (skill_dir / "SKILL.md").is_file()


def test_global_copilot_rule_installs_to_native_instructions_path(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed Copilot rule lands in the native
    ``~/.copilot/instructions/`` dir (Copilot CLI discovery), not the
    inert ``$GRIM_HOME`` workspace layout."""
    repo = f"{unique_repo}/cp-rule"
    ru = make_artifact(repo, "rule", {"cp-rule.md": "# guidance\n"}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[rules]\ncp-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "copilot")["items"]
    assert rows[0]["status"] == "installed"
    native = runner.home / ".copilot/instructions/cp-rule.instructions.md"
    assert native.is_file(), f"global Copilot rule must land at {native}"
    assert not (grim_home / ".github/instructions/cp-rule.instructions.md").exists(), (
        "the inert $GRIM_HOME workspace layout must no longer be written"
    )


def test_global_copilot_rule_native_root_emits_no_fallback_warning(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Pass arm of the Copilot global-rule warn conditional (installer.rs
    ~594): when the native Copilot root resolves (HOME set), the rule
    installs to the native instructions path and NO fallback warning is
    emitted."""
    repo = f"{unique_repo}/cp-warn-rule"
    ru = make_artifact(repo, "rule", {"cp-warn-rule.md": "# guidance\n"}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[rules]\ncp-warn-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    result = runner.run(
        "install", "--global", "--client", "copilot", format="json", log_level="warn"
    )
    assert (runner.home / ".copilot/instructions/cp-warn-rule.instructions.md").is_file()
    assert "no resolvable Copilot root" not in result.stderr, (
        "a resolvable native root must not emit the workspace-fallback warning\n"
        f"stderr: {result.stderr.strip()}"
    )


def test_global_copilot_rule_without_resolvable_root_warns_and_falls_back(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Decline arm of the Copilot global-rule warn conditional (installer.rs
    ~594): with neither COPILOT_HOME nor HOME set no native Copilot root
    resolves, so the rule falls back to the inert workspace layout and the
    installer warns."""
    repo = f"{unique_repo}/cp-nowarn-rule"
    ru = make_artifact(repo, "rule", {"cp-nowarn-rule.md": "# guidance\n"}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[rules]\ncp-nowarn-rule = "{ru.fq}"\n')
    # cwd in an isolated dir so the workspace-layout fallback write lands in
    # the temp tree, not the pytest working directory.
    workdir = tmp_path / "work"
    workdir.mkdir()
    runner = GrimRunner(grim_binary, grim_home, cwd=workdir)
    # Drop the isolated HOME/USERPROFILE so home_dir() returns None and, with
    # COPILOT_HOME also unset, global_native_root(None, None) is None.
    runner.env.pop("HOME", None)
    runner.env.pop("USERPROFILE", None)
    runner.json("lock", "--global")

    result = runner.run(
        "install", "--global", "--client", "copilot", format="json", log_level="warn", check=False
    )
    assert "no resolvable Copilot root" in result.stderr, (
        "an unresolvable native root must emit the workspace-fallback warning\n"
        f"rc={result.returncode} stderr: {result.stderr.strip()}"
    )


def test_global_copilot_rule_layout_migration_reaps_old_output(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """ADR render-layout-stability upgrade fixture: a pre-move install
    (record anchored to ``grim-home``, file at the old
    ``$GRIM_HOME/.github/instructions/`` layout, bytes unchanged) migrates
    on ``grim update`` — the rule re-materializes at the native path, the
    orphaned old output is reaped, the record re-anchors, and
    ``status``/``uninstall`` round-trip cleanly."""
    repo = f"{unique_repo}/mig-rule"
    ru = make_artifact(repo, "rule", {"mig-rule.md": "# guidance\n"}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[rules]\nmig-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "copilot")

    native = runner.home / ".copilot/instructions/mig-rule.instructions.md"
    assert native.is_file()

    # Simulate the pre-move on-disk state: move the file to the old
    # workspace layout under $GRIM_HOME and re-anchor the state record
    # (bytes unchanged, so the recorded content hash still round-trips).
    old = grim_home / ".github/instructions/mig-rule.instructions.md"
    old.parent.mkdir(parents=True)
    old.write_bytes(native.read_bytes())
    native.unlink()
    state_path = grim_home / "state/global.json"
    state = json.loads(state_path.read_text())
    [record] = [r for r in state["records"] if r["name"] == "mig-rule"]
    [output] = record["outputs"]
    output["target"] = {
        "anchor": "grim-home",
        "relative": ".github/instructions/mig-rule.instructions.md",
    }
    state_path.write_text(json.dumps(state))

    # Make Copilot (and only Copilot) detected for the update pass.
    (runner.home / ".copilot/skills").mkdir(parents=True)

    runner.json("update", "--global")

    assert native.is_file(), "update must re-materialize at the new native path"
    assert not old.exists(), "the unmodified old-layout output must be reaped"
    state = json.loads(state_path.read_text())
    [record] = [r for r in state["records"] if r["name"] == "mig-rule"]
    anchors = [o["target"]["anchor"] for o in record["outputs"]]
    assert anchors == ["copilot-root"], f"record must re-anchor, got {anchors}"

    status = runner.json("status", "--global")["items"]
    assert status[0]["state"] == "installed", status

    runner.json("uninstall", "rule", "mig-rule", "--global")
    assert not native.exists(), "uninstall must remove the migrated output"


# ---------------------------------------------------------------------------
# Vendor env-var directory overrides
# ---------------------------------------------------------------------------


def test_global_claude_install_honors_claude_config_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``$CLAUDE_CONFIG_DIR`` replaces the entire ``~/.claude`` tree, so a
    global Claude skill AND rule both land under it instead of ``$HOME``."""
    repo_s = f"{unique_repo}/env-skill"
    repo_r = f"{unique_repo}/env-rule"
    sk = make_artifact(
        repo_s,
        "skill",
        {"env-skill/SKILL.md": "---\nname: env-skill\ndescription: env override\n---\n# body\n"},
        tag="v1",
    )
    ru = make_artifact(
        repo_r,
        "rule",
        {"env-rule.md": "---\npaths: ['**/*.rs']\n---\n# env rule\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\nenv-skill = "{sk.fq}"\n\n[rules]\nenv-rule = "{ru.fq}"\n'
    )
    runner = GrimRunner(grim_binary, grim_home)
    config_dir = grim_home.parent / "claude-config"
    runner.env["CLAUDE_CONFIG_DIR"] = str(config_dir)
    # `CLAUDE_CONFIG_DIR` replaces the whole `~/.claude` tree, detection
    # included: the override dir must exist for Claude to be detected.
    config_dir.mkdir(parents=True, exist_ok=True)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global")["items"]
    assert all(r["status"] == "installed" for r in install_rows)

    assert (config_dir / "skills/env-skill/SKILL.md").is_file(), (
        "skill must land in $CLAUDE_CONFIG_DIR/skills/"
    )
    assert (config_dir / "rules/env-rule.md").is_file(), (
        "rule must land in $CLAUDE_CONFIG_DIR/rules/"
    )
    # Default location must stay untouched.
    assert not (runner.home / ".claude/skills/env-skill").exists()
    assert not (runner.home / ".claude/rules/env-rule.md").exists()


def test_global_opencode_skill_honors_opencode_config_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``$OPENCODE_CONFIG_DIR`` (OpenCode's additive scan dir) wins over the
    XDG default for global skill installs."""
    repo = f"{unique_repo}/oc-env-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"oc-env-skill/SKILL.md": "---\nname: oc-env-skill\ndescription: env override\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\noc-env-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    config_dir = grim_home.parent / "opencode-config"
    runner.env["OPENCODE_CONFIG_DIR"] = str(config_dir)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert install_rows[0]["status"] == "installed"

    assert (config_dir / "skills/oc-env-skill/SKILL.md").is_file(), (
        "skill must land in $OPENCODE_CONFIG_DIR/skills/"
    )
    assert not (runner.home / ".config/opencode/skills/oc-env-skill").exists(), (
        "XDG default must stay untouched when OPENCODE_CONFIG_DIR is set"
    )


def test_global_copilot_skill_honors_copilot_home(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``$COPILOT_HOME`` replaces ``~/.copilot`` entirely for global skills."""
    repo = f"{unique_repo}/cp-env-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"cp-env-skill/SKILL.md": "---\nname: cp-env-skill\ndescription: env override\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncp-env-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    copilot_home = grim_home.parent / "copilot-home"
    runner.env["COPILOT_HOME"] = str(copilot_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "copilot")["items"]
    assert install_rows[0]["status"] == "installed"

    assert (copilot_home / "skills/cp-env-skill/SKILL.md").is_file(), (
        "skill must land in $COPILOT_HOME/skills/"
    )
    assert not (runner.home / ".copilot/skills/cp-env-skill").exists(), (
        "default ~/.copilot must stay untouched when COPILOT_HOME is set"
    )


def test_global_uninstall_removes_files_from_env_override_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Uninstall uses the recorded absolute path: files installed under an
    env-override dir are removed even though resolution re-runs later."""
    repo = f"{unique_repo}/env-un-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"env-un-skill/SKILL.md": "---\nname: env-un-skill\ndescription: x\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nenv-un-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    config_dir = grim_home.parent / "claude-config-un"
    runner.env["CLAUDE_CONFIG_DIR"] = str(config_dir)
    # `CLAUDE_CONFIG_DIR` replaces the whole `~/.claude` tree, detection
    # included: the override dir must exist for Claude to be detected.
    config_dir.mkdir(parents=True, exist_ok=True)
    runner.json("lock", "--global")
    runner.json("install", "--global")
    assert (config_dir / "skills/env-un-skill/SKILL.md").is_file(), (
        "install step must have written the skill before uninstall can be tested"
    )

    runner.json("uninstall", "skill", "env-un-skill", "--global")
    assert not (config_dir / "skills/env-un-skill").exists(), (
        "uninstall must remove the env-override install dir"
    )


def test_global_empty_env_override_is_treated_as_unset(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``CLAUDE_CONFIG_DIR=""`` must behave exactly like an unset variable:
    the install lands in the default ``~/.claude`` tree, never in a path
    built from an empty string (which would resolve relative to CWD)."""
    repo = f"{unique_repo}/empty-env-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"empty-env-skill/SKILL.md": "---\nname: empty-env-skill\ndescription: x\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nempty-env-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    # Claude must be *detected* globally for the default target set to
    # include it; without any marker grim resolves to the generic `agents`
    # client (skills-only pool).
    (runner.home / ".claude").mkdir(parents=True, exist_ok=True)
    runner.env["CLAUDE_CONFIG_DIR"] = ""
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global")["items"]
    assert install_rows[0]["status"] == "installed"

    assert (runner.home / ".claude/skills/empty-env-skill/SKILL.md").is_file(), (
        "empty CLAUDE_CONFIG_DIR must fall back to the default ~/.claude tree"
    )


def test_global_update_rematerializes_into_env_override_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``grim update --global`` re-resolves the install target from the
    current environment — with ``CLAUDE_CONFIG_DIR`` set, the updated file
    must land in the override dir, not the ``$HOME`` default."""
    repo = f"{unique_repo}/env-up-skill"
    v1 = make_artifact(
        repo,
        "skill",
        {"env-up-skill/SKILL.md": "---\nname: env-up-skill\ndescription: x\n---\nv1\n"},
        tag="stable",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nenv-up-skill = "{v1.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    config_dir = grim_home.parent / "claude-config-up"
    runner.env["CLAUDE_CONFIG_DIR"] = str(config_dir)
    # `CLAUDE_CONFIG_DIR` replaces the whole `~/.claude` tree, detection
    # included: the override dir must exist for Claude to be detected.
    config_dir.mkdir(parents=True, exist_ok=True)
    runner.json("lock", "--global")
    runner.json("install", "--global")
    installed = config_dir / "skills/env-up-skill/SKILL.md"
    assert installed.is_file() and "v1" in installed.read_text()

    # Roll the floating tag onto v2, then update.
    v2 = make_artifact(
        repo,
        "skill",
        {"env-up-skill/SKILL.md": "---\nname: env-up-skill\ndescription: x\n---\nv2\n"},
        tag="2.0.0",
    )
    retag(repo, "stable", v2.digest)

    rows = runner.json("update", "--global")["items"]
    assert rows[0]["action"] == "updated"
    assert "v2" in installed.read_text(), (
        "update must rematerialize into $CLAUDE_CONFIG_DIR, not the $HOME default"
    )
    assert not (runner.home / ".claude/skills/env-up-skill").exists(), (
        "update must not fall back to the default ~/.claude tree"
    )


def test_global_opencode_rule_honors_opencode_config_file(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``$OPENCODE_CONFIG`` (OpenCode's custom config *file* path) is the
    edit target for global rule registration — the managed glob must land
    there, not in the XDG default, and deregister from there too."""
    repo = f"{unique_repo}/oc-cfg-rule"
    ru = make_artifact(
        repo,
        "rule",
        {"oc-cfg-rule.md": "---\npaths: ['**']\n---\n# rule\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\noc-cfg-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    custom_cfg = grim_home.parent / "custom" / "oc.json"
    custom_cfg.parent.mkdir(parents=True)
    runner.env["OPENCODE_CONFIG"] = str(custom_cfg)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert install_rows[0]["status"] == "installed"

    assert custom_cfg.is_file(), (
        "managed instructions glob must be registered in $OPENCODE_CONFIG"
    )
    instructions = json.loads(custom_cfg.read_text()).get("instructions", [])
    assert any(str(grim_home) in e for e in instructions), (
        f"absolute glob missing from $OPENCODE_CONFIG; instructions={instructions}"
    )
    assert not (runner.home / ".config/opencode/opencode.json").exists(), (
        "XDG-default opencode.json must stay untouched when OPENCODE_CONFIG is set"
    )

    # Deregistration converges on the same file.
    runner.json("uninstall", "rule", "oc-cfg-rule", "--global")
    cfg_after = json.loads(custom_cfg.read_text())
    assert "instructions" not in cfg_after, (
        f"managed glob must deregister from $OPENCODE_CONFIG; got {cfg_after}"
    )


def test_global_opencode_honors_custom_xdg_config_home(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A custom ``$XDG_CONFIG_HOME`` (different from the ``~/.config``
    default) drives BOTH the OpenCode skills root and the ``opencode.json``
    edit target — proving grim reads the variable instead of hardcoding
    ``~/.config``."""
    repo_s = f"{unique_repo}/xdg-skill"
    repo_r = f"{unique_repo}/xdg-rule"
    sk = make_artifact(
        repo_s,
        "skill",
        {"xdg-skill/SKILL.md": "---\nname: xdg-skill\ndescription: x\n---\n# body\n"},
        tag="v1",
    )
    ru = make_artifact(
        repo_r,
        "rule",
        {"xdg-rule.md": "---\npaths: ['**']\n---\n# rule\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\nxdg-skill = "{sk.fq}"\n\n[rules]\nxdg-rule = "{ru.fq}"\n'
    )
    runner = GrimRunner(grim_binary, grim_home)
    xdg = grim_home.parent / "custom-xdg"
    runner.env["XDG_CONFIG_HOME"] = str(xdg)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert all(r["status"] == "installed" for r in install_rows)

    assert (xdg / "opencode/skills/xdg-skill/SKILL.md").is_file(), (
        "skill must land in $XDG_CONFIG_HOME/opencode/skills/"
    )
    cfg = xdg / "opencode/opencode.json"
    assert cfg.is_file(), (
        "opencode.json must be created under the custom $XDG_CONFIG_HOME"
    )
    instructions = json.loads(cfg.read_text()).get("instructions", [])
    assert any(str(grim_home) in e for e in instructions)
    # The ~/.config default must stay untouched.
    assert not (runner.home / ".config/opencode").exists(), (
        "default ~/.config must stay untouched when XDG_CONFIG_HOME points elsewhere"
    )


# ---------------------------------------------------------------------------
# Codex global scope
# ---------------------------------------------------------------------------


def _codex_agent_doc(name: str = "cx-agent") -> str:
    return (
        f"---\nname: {name}\ndescription: A codex agent.\nmodel: gpt-5\n---\n"
        f"# {name}\nCodex body text.\n"
    )


def test_global_install_codex_skill_lands_in_home_dot_agents(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed Codex skill materializes at
    ``$HOME/.agents/skills/<name>/`` — the cross-vendor open standard."""
    repo = f"{unique_repo}/cx-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"cx-skill/SKILL.md": "---\nname: cx-skill\ndescription: codex skill\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncx-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "codex")["items"]
    assert install_rows[0]["status"] == "installed"

    skill_dir = runner.home / ".agents/skills/cx-skill"
    assert (skill_dir / "SKILL.md").is_file(), (
        f"global Codex skill must materialize at $HOME/.agents/skills/; nothing at {skill_dir}"
    )
    # NOT under $CODEX_HOME-style or $GRIM_HOME layouts.
    assert not (runner.home / ".codex/skills/cx-skill").exists()
    assert not (grim_home / ".agents/skills/cx-skill").exists()


def test_global_install_codex_agent_lands_in_home_dot_codex(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A globally-installed Codex agent materializes at
    ``$HOME/.codex/agents/<name>.toml`` when ``CODEX_HOME`` is unset."""
    import tomllib

    repo = f"{unique_repo}/cx-agent"
    ag = make_artifact(repo, "agent", {"cx-agent.md": _codex_agent_doc("cx-agent")}, tag="v1")
    (grim_home / "grimoire.toml").write_text(f'[agents]\ncx-agent = "{ag.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "codex")["items"]
    assert install_rows[0]["status"] == "installed"

    toml_file = runner.home / ".codex/agents/cx-agent.toml"
    assert toml_file.is_file(), (
        f"global Codex agent must materialize at $HOME/.codex/agents/; nothing at {toml_file}"
    )
    parsed = tomllib.loads(toml_file.read_text())
    assert parsed["name"] == "cx-agent"
    assert "Codex body text." in parsed["developer_instructions"]


def test_global_codex_home_relocates_agent_but_not_skill(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``CODEX_HOME`` relocates the Codex **agent** root but NOT skills:
    the agent lands under ``$CODEX_HOME/agents`` while the skill stays at the
    ``$HOME/.agents/skills`` cross-vendor standard."""
    sk = make_artifact(
        f"{unique_repo}/cx2-skill",
        "skill",
        {"cx2-skill/SKILL.md": "---\nname: cx2-skill\ndescription: s\n---\n# body\n"},
        tag="v1",
    )
    ag = make_artifact(
        f"{unique_repo}/cx2-agent",
        "agent",
        {"cx2-agent.md": _codex_agent_doc("cx2-agent")},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\ncx2-skill = "{sk.fq}"\n[agents]\ncx2-agent = "{ag.fq}"\n'
    )
    codex_home = grim_home.parent / "codex_home"
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["CODEX_HOME"] = str(codex_home)
    runner.json("lock", "--global")

    install_rows = runner.json("install", "--global", "--client", "codex")["items"]
    assert all(r["status"] == "installed" for r in install_rows), install_rows

    # Agent follows $CODEX_HOME.
    assert (codex_home / "agents/cx2-agent.toml").is_file(), (
        "CODEX_HOME must relocate the Codex agent root"
    )
    assert not (runner.home / ".codex/agents/cx2-agent.toml").exists(), (
        "with CODEX_HOME set, the agent must NOT land in the ~/.codex default"
    )
    # Skill stays at $HOME/.agents/skills — CODEX_HOME does not move it.
    assert (runner.home / ".agents/skills/cx2-skill/SKILL.md").is_file(), (
        "CODEX_HOME must NOT relocate Codex skills (cross-vendor $HOME standard)"
    )
    assert not (codex_home / "skills/cx2-skill").exists(), (
        "Codex skills must never land under $CODEX_HOME"
    )


def test_global_codex_home_relocates_mcp_config_too(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """``CODEX_HOME`` also relocates the Codex MCP registration target:
    ``config.toml`` shares the ``CodexRoot`` anchor with the `agents/`
    dir, so a Codex-scoped MCP install must follow it too (plan C1)."""
    import tomllib

    descriptor_dir = tmp_path / "src"
    descriptor = descriptor_dir / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["mcp"]\n'
    )
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    codex_home = grim_home.parent / "codex_home"
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["CODEX_HOME"] = str(codex_home)
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    install_rows = runner.json("install", "--global", "--client", "codex")["items"]
    assert install_rows[0]["status"] == "installed", install_rows

    config = codex_home / "config.toml"
    assert config.is_file(), f"CODEX_HOME must relocate the Codex MCP config target; nothing at {config}"
    assert not (runner.home / ".codex/config.toml").exists(), (
        "with CODEX_HOME set, the MCP config must NOT land in the ~/.codex default"
    )
    parsed = tomllib.loads(config.read_text())
    assert parsed["mcp_servers"]["grim-mcp"]["command"] == "grim"


def test_global_no_client_flag_installs_to_detected_clients_only(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """No ``--client`` at global scope installs only to the detected
    clients. With only ``$CODEX_HOME`` present (no ``~/.claude``,
    ``~/.copilot/skills``, or an OpenCode config anywhere), a globally
    installed skill materializes ONLY at the cross-vendor
    ``$HOME/.agents/skills`` path — mirrors
    ``test_no_clients_config_installs_to_detected_clients`` (project scope,
    test_clients.py) at global scope."""
    repo = f"{unique_repo}/cx-only-skill"
    sk = make_artifact(
        repo,
        "skill",
        {"cx-only-skill/SKILL.md": "---\nname: cx-only-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncx-only-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)

    codex_home = grim_home.parent / "codex_home"
    codex_home.mkdir(parents=True)
    runner.env["CODEX_HOME"] = str(codex_home)

    runner.json("lock", "--global")
    install_rows = runner.json("install", "--global")["items"]
    assert install_rows[0]["status"] == "installed", install_rows

    assert (runner.home / ".agents/skills/cx-only-skill/SKILL.md").is_file(), (
        "the only detected client (Codex, via $CODEX_HOME) must receive the skill"
    )
    assert not (runner.home / ".claude/skills/cx-only-skill").exists(), (
        "Claude was not detected (no ~/.claude) and must not receive the skill"
    )
    assert not (runner.home / ".copilot/skills/cx-only-skill").exists(), (
        "Copilot was not detected and must not receive the skill"
    )
    assert not (runner.home / ".config/opencode/skills/cx-only-skill").exists(), (
        "OpenCode was not detected and must not receive the skill"
    )


# ---------------------------------------------------------------------------
# Wave-1 vendor global-scope roots
#
# Global paths default to $HOME-derived layouts. Two vendor overrides ARE
# honored and are covered by the env-override tests further below:
# ``KIRO_HOME`` (replaces ~/.kiro outright) and ``GEMINI_CLI_HOME`` (replaces
# $HOME, so the ``.gemini`` segment still applies). The rest
# (CURSOR_CONFIG_DIR, JUNIE_*_LOCATIONS) are not honored — CURSOR_CONFIG_DIR is
# watchlisted and GEMINI_CONFIG_DIR does not exist upstream.
# Client-native skill dirs: Cursor/Kiro/Junie. Shared $HOME/.agents/skills
# pool: Gemini/Zed/Amp (+ Codex).
# ---------------------------------------------------------------------------

_MCP_STDIO_DESCRIPTOR = (
    'description = "A stdio MCP server."\n\n'
    "[server]\n"
    'transport = "stdio"\n'
    'command = "grim"\n'
    'args = ["mcp"]\n'
)


def _release_global_mcp(runner: GrimRunner, tmp_path: Path, registry: str, unique_repo: str) -> str:
    descriptor = tmp_path / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(_MCP_STDIO_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


# ── Cursor ────────────────────────────────────────────────────────────────


def test_global_install_cursor_skill_lands_in_home_dot_cursor(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Cursor skill → ``$HOME/.cursor/skills/<name>/`` (native dir)."""
    sk = make_artifact(
        f"{unique_repo}/cur-skill",
        "skill",
        {"cur-skill/SKILL.md": "---\nname: cur-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncur-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "cursor")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".cursor/skills/cur-skill/SKILL.md").is_file(), (
        "global Cursor skill must land in $HOME/.cursor/skills/"
    )
    assert not (grim_home / ".cursor/skills/cur-skill").exists()


def test_global_install_cursor_rule_lands_in_home_dot_cursor_rules(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Cursor rule → ``$HOME/.cursor/rules/<name>.mdc``."""
    ru = make_artifact(
        f"{unique_repo}/cur-rule",
        "rule",
        {"cur-rule.md": "---\npaths: ['**/*.rs']\n---\n# Cursor rule\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\ncur-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "cursor")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".cursor/rules/cur-rule.mdc").is_file(), (
        "global Cursor rule must land in $HOME/.cursor/rules/<name>.mdc"
    )


def test_global_install_cursor_mcp_lands_in_home_dot_cursor(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Global Cursor MCP → ``$HOME/.cursor/mcp.json``."""
    runner = GrimRunner(grim_binary, grim_home)
    ref = _release_global_mcp(runner, tmp_path, registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "cursor")["items"]
    assert rows[0]["status"] == "installed", rows
    cfg = runner.home / ".cursor/mcp.json"
    assert cfg.is_file(), "global Cursor MCP entry must land in $HOME/.cursor/mcp.json"
    assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"


# ── Kiro ──────────────────────────────────────────────────────────────────


def test_global_install_kiro_skill_lands_in_home_dot_kiro(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Kiro skill → ``$HOME/.kiro/skills/<name>/`` (native dir)."""
    sk = make_artifact(
        f"{unique_repo}/kiro-skill",
        "skill",
        {"kiro-skill/SKILL.md": "---\nname: kiro-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nkiro-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "kiro")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".kiro/skills/kiro-skill/SKILL.md").is_file(), (
        "global Kiro skill must land in $HOME/.kiro/skills/"
    )


def test_global_install_kiro_rule_lands_in_home_dot_kiro_steering(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Kiro rule → ``$HOME/.kiro/steering/<name>.md``."""
    ru = make_artifact(
        f"{unique_repo}/kiro-rule",
        "rule",
        {"kiro-rule.md": "---\npaths: ['**/*.rs']\n---\n# Kiro steering\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[rules]\nkiro-rule = "{ru.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "kiro")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".kiro/steering/kiro-rule.md").is_file(), (
        "global Kiro rule must land in $HOME/.kiro/steering/<name>.md"
    )


def test_global_kiro_home_relocates_the_whole_kiro_root(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``KIRO_HOME`` replaces ``~/.kiro`` **outright** — no ``.kiro`` segment is
    appended — so skills and steering both hang off the relocated root and
    nothing lands in the ``~/.kiro`` default.

    This is the only test that pins the env variable's *name*; the unit tests
    inject values into the pure resolver and would pass with a typo'd name."""
    sk = make_artifact(
        f"{unique_repo}/kh-skill",
        "skill",
        {"kh-skill/SKILL.md": "---\nname: kh-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    ru = make_artifact(
        f"{unique_repo}/kh-rule",
        "rule",
        {"kh-rule.md": "---\npaths: ['**/*.rs']\n---\n# Kiro steering\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\nkh-skill = "{sk.fq}"\n[rules]\nkh-rule = "{ru.fq}"\n'
    )
    kiro_home = grim_home.parent / "kiro_home"
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["KIRO_HOME"] = str(kiro_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "kiro")["items"]
    assert all(r["status"] == "installed" for r in rows), rows

    assert (kiro_home / "skills/kh-skill/SKILL.md").is_file(), (
        "KIRO_HOME must relocate global Kiro skills to $KIRO_HOME/skills/"
    )
    assert (kiro_home / "steering/kh-rule.md").is_file(), (
        "KIRO_HOME must relocate global Kiro steering to $KIRO_HOME/steering/"
    )
    # The root is replaced outright — never nested under a `.kiro` segment.
    assert not (kiro_home / ".kiro").exists(), (
        "KIRO_HOME replaces ~/.kiro outright; no `.kiro` segment is appended"
    )
    assert not (runner.home / ".kiro/skills/kh-skill").exists(), (
        "with KIRO_HOME set, nothing may land in the ~/.kiro default"
    )
    assert not (runner.home / ".kiro/steering/kh-rule.md").exists(), (
        "with KIRO_HOME set, nothing may land in the ~/.kiro default"
    )


def test_global_kiro_home_relocates_mcp_config_too(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """``KIRO_HOME`` also relocates the Kiro MCP registration target:
    ``settings/mcp.json`` shares the ``KiroRoot`` anchor with skills and
    steering, so a Kiro-scoped MCP install must follow it too (mirrors
    ``test_global_codex_home_relocates_mcp_config_too``)."""
    descriptor_dir = tmp_path / "kiro_src"
    descriptor = descriptor_dir / "mcp" / "kh-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["mcp"]\n'
    )
    ref = f"{registry}/{unique_repo}/mcp/kh-mcp:1.0.0"
    kiro_home = grim_home.parent / "kiro_home_mcp"
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["KIRO_HOME"] = str(kiro_home)
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\nkh-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    install_rows = runner.json("install", "--global", "--client", "kiro")["items"]
    assert install_rows[0]["status"] == "installed", install_rows

    config = kiro_home / "settings/mcp.json"
    assert config.is_file(), (
        f"KIRO_HOME must relocate the Kiro MCP config target; nothing at {config}"
    )
    assert not (runner.home / ".kiro/settings/mcp.json").exists(), (
        "with KIRO_HOME set, the MCP config must NOT land in the ~/.kiro default"
    )
    parsed = json.loads(config.read_text())
    assert parsed["mcpServers"]["kh-mcp"]["command"] == "grim"


def test_global_gemini_cli_home_relocates_config_root_but_not_the_shared_pool(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``GEMINI_CLI_HOME`` replaces ``$HOME``, so the Gemini config root becomes
    ``$GEMINI_CLI_HOME/.gemini`` — the ``.gemini`` segment IS still appended
    (the opposite shape to ``KIRO_HOME``/``CODEX_HOME``).

    Skills deliberately do NOT follow it: the ``.agents/skills`` pool is one
    physical tree shared with Codex/Zed/Amp under the prune refcount guard, so
    it stays keyed on the real ``$HOME``."""
    sk = make_artifact(
        f"{unique_repo}/gch-skill",
        "skill",
        {"gch-skill/SKILL.md": "---\nname: gch-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    ag = make_artifact(
        f"{unique_repo}/gch-agent",
        "agent",
        {"gch-agent.md": "---\nname: gch-agent\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\ngch-skill = "{sk.fq}"\n[agents]\ngch-agent = "{ag.fq}"\n'
    )
    gemini_cli_home = grim_home.parent / "gemini_cli_home"
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["GEMINI_CLI_HOME"] = str(gemini_cli_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "gemini")["items"]
    assert all(r["status"] == "installed" for r in rows), rows

    # The `.gemini` segment survives the override — this is the single most
    # error-prone part of the resolution and the reason this test exists.
    assert (gemini_cli_home / ".gemini/agents/gch-agent.md").is_file(), (
        "GEMINI_CLI_HOME replaces $HOME; the agent lands in $GEMINI_CLI_HOME/.gemini/agents/"
    )
    assert not (gemini_cli_home / "agents/gch-agent.md").exists(), (
        "GEMINI_CLI_HOME is a $HOME replacement, not a config-root replacement"
    )
    assert not (runner.home / ".gemini/agents/gch-agent.md").exists(), (
        "with GEMINI_CLI_HOME set, the agent must NOT land in the ~/.gemini default"
    )
    # The shared pool stays on the real $HOME.
    assert (runner.home / ".agents/skills/gch-skill/SKILL.md").is_file(), (
        "the cross-vendor pool stays keyed on $HOME, never on GEMINI_CLI_HOME"
    )
    assert not (gemini_cli_home / ".agents/skills/gch-skill").exists(), (
        "GEMINI_CLI_HOME must not fork the shared .agents/skills pool"
    )


# ---------------------------------------------------------------------------
# Legacy-root reaper: upgrading into a release that honors the override
# ---------------------------------------------------------------------------


def test_global_kiro_home_upgrade_reaps_the_pre_override_root(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Principle 9 upgrade fixture for the legacy-root reaper.

    A grim that did not yet honor ``KIRO_HOME`` wrote under ``~/.kiro`` while
    the variable was set. The record it left names the SAME ``kiro-root``
    anchor and the SAME relative path the current layout produces — only the
    root *resolution* moved — so the pair-keyed ``reap_moved_outputs`` is blind
    to it. Setting the variable on an existing install reproduces exactly that
    on-disk state.

    ``update`` must re-materialize under the override, reap the pre-override
    copies, keep the record anchored at ``kiro-root``, and round-trip
    ``status``/``uninstall``."""
    sk = make_artifact(
        f"{unique_repo}/kmig-skill",
        "skill",
        {"kmig-skill/SKILL.md": "---\nname: kmig-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    ru = make_artifact(
        f"{unique_repo}/kmig-rule",
        "rule",
        {"kmig-rule.md": "# steering\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\nkmig-skill = "{sk.fq}"\n[rules]\nkmig-rule = "{ru.fq}"\n'
    )
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    # Pre-upgrade: KIRO_HOME unset, so everything lands under ~/.kiro.
    rows = runner.json("install", "--global", "--client", "kiro")["items"]
    assert all(r["status"] == "installed" for r in rows), rows
    old_skill = runner.home / ".kiro/skills/kmig-skill"
    old_rule = runner.home / ".kiro/steering/kmig-rule.md"
    assert (old_skill / "SKILL.md").is_file()
    assert old_rule.is_file()

    # The upgrade: the same record, now resolved against the honored override.
    kiro_home = grim_home.parent / "kiro_home_upgrade"
    kiro_home.mkdir()
    runner.env["KIRO_HOME"] = str(kiro_home)

    runner.json("update", "--global", "--client", "kiro")

    assert (kiro_home / "skills/kmig-skill/SKILL.md").is_file(), (
        "update must re-materialize the skill under the honored override"
    )
    assert (kiro_home / "steering/kmig-rule.md").is_file(), (
        "update must re-materialize the rule under the honored override"
    )
    assert not old_skill.exists(), "the unmodified pre-override skill must be reaped"
    assert not old_rule.exists(), "the unmodified pre-override rule must be reaped"

    # The anchor never moved — only its root did. That is the whole point.
    state = json.loads((grim_home / "state/global.json").read_text())
    anchors = {
        o["target"]["anchor"] for r in state["records"] if r["name"].startswith("kmig-") for o in r["outputs"]
    }
    assert anchors == {"kiro-root"}, f"the recorded anchor must be unchanged, got {anchors}"

    status = runner.json("status", "--global")["items"]
    assert all(r["state"] == "installed" for r in status), status

    runner.json("uninstall", "skill", "kmig-skill", "--global")
    assert not (kiro_home / "skills/kmig-skill").exists(), (
        "uninstall must remove the migrated output"
    )


def test_global_gemini_cli_home_upgrade_reaps_the_pre_override_root(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """The same upgrade fixture for the second relocated root.

    ``GEMINI_CLI_HOME`` has the opposite shape to ``KIRO_HOME`` — it replaces
    ``$HOME`` and the ``.gemini`` segment is still appended — so the legacy
    root must be ``$HOME/.gemini``, not ``$HOME``. This test is what proves
    the row computes that, end to end.

    An **agent**, not a skill: Gemini skills anchor at ``agents-skills`` (the
    shared pool, deliberately keyed on the real ``$HOME``), so only an agent
    or MCP output is anchored at ``gemini-root`` at all.

    The third relocated root — Zed on macOS — has no acceptance fixture: it
    cannot be reproduced on a Linux or Windows host. Its arm is covered by
    ``relocated_vendor_roots_lists_only_the_roots_that_moved``, which injects
    the legacy root instead of reading the platform."""
    ag = make_artifact(
        f"{unique_repo}/gmig-agent",
        "agent",
        {"gmig-agent.md": "---\nname: gmig-agent\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[agents]\ngmig-agent = "{ag.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    # Pre-upgrade: GEMINI_CLI_HOME unset, so the agent lands under ~/.gemini.
    rows = runner.json("install", "--global", "--client", "gemini")["items"]
    assert all(r["status"] == "installed" for r in rows), rows
    old_agent = runner.home / ".gemini/agents/gmig-agent.md"
    assert old_agent.is_file()

    gemini_cli_home = grim_home.parent / "gemini_home_upgrade"
    (gemini_cli_home / ".gemini").mkdir(parents=True)
    runner.env["GEMINI_CLI_HOME"] = str(gemini_cli_home)

    runner.json("update", "--global", "--client", "gemini")

    assert (gemini_cli_home / ".gemini/agents/gmig-agent.md").is_file(), (
        "the agent must re-materialize under $GEMINI_CLI_HOME/.gemini/"
    )
    assert not old_agent.exists(), (
        "the unmodified copy at the pre-override root must be reaped"
    )
    state = json.loads((grim_home / "state/global.json").read_text())
    [record] = [r for r in state["records"] if r["name"] == "gmig-agent"]
    assert [o["target"]["anchor"] for o in record["outputs"]] == ["gemini-root"], (
        f"the recorded anchor must be unchanged: {record['outputs']}"
    )


def test_global_kiro_home_upgrade_keeps_the_only_copy_when_kiro_is_not_written(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """The legacy-root reaper must never delete the *only* copy.

    A pass that does not materialize Kiro writes nothing under the override,
    so ``~/.kiro`` still holds the artifact. Reaping it there would destroy it
    with nothing put in its place. Reachable with no user error at all:
    ``$KIRO_HOME`` pointing at a directory the Kiro CLI has not created yet
    leaves Kiro undetected, so the first run after the upgrade is this case."""
    sk = make_artifact(
        f"{unique_repo}/kkeep-skill",
        "skill",
        {"kkeep-skill/SKILL.md": "---\nname: kkeep-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nkkeep-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "kiro")

    old_skill = runner.home / ".kiro/skills/kkeep-skill"
    assert (old_skill / "SKILL.md").is_file()

    # KIRO_HOME set, but this pass targets a different client entirely.
    kiro_home = grim_home.parent / "kiro_home_untouched"
    kiro_home.mkdir()
    runner.env["KIRO_HOME"] = str(kiro_home)

    runner.json("update", "--global", "--client", "claude")

    assert not (kiro_home / "skills/kkeep-skill").exists(), (
        "a pass that does not target Kiro must not write under the override"
    )
    assert (old_skill / "SKILL.md").is_file(), (
        "the only copy must survive a pass that migrated nothing"
    )


def test_global_kiro_home_upgrade_unsplices_the_stranded_mcp_entry(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """The sharp case: an MCP member spliced into the user's own
    ``~/.kiro/settings/mcp.json`` before the upgrade.

    A file delete cannot fix this — the config file is the user's, and
    ``uninstall`` now resolves ``$KIRO_HOME``, so the stranded member would be
    permanently un-removable. It must be spliced out of the OLD file, and every
    byte the user owns must survive."""
    descriptor_dir = tmp_path / "kiro_mig_src"
    descriptor = descriptor_dir / "mcp" / "kmig-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["mcp"]\n'
    )
    ref = f"{registry}/{unique_repo}/mcp/kmig-mcp:1.0.0"
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\nkmig-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global", "--client", "kiro")["items"]
    assert rows[0]["status"] == "installed", rows

    old_config = runner.home / ".kiro/settings/mcp.json"
    assert json.loads(old_config.read_text())["mcpServers"]["kmig-mcp"]["command"] == "grim"
    # A foreign member the user owns, which must survive the un-splice.
    parsed = json.loads(old_config.read_text())
    parsed["mcpServers"]["user-server"] = {"command": "keep-me"}
    old_config.write_text(json.dumps(parsed, indent=2))

    kiro_home = grim_home.parent / "kiro_home_mcp_upgrade"
    kiro_home.mkdir()
    runner.env["KIRO_HOME"] = str(kiro_home)

    runner.json("update", "--global", "--client", "kiro")

    new_config = kiro_home / "settings/mcp.json"
    assert json.loads(new_config.read_text())["mcpServers"]["kmig-mcp"]["command"] == "grim", (
        "the registration must move to the honored override"
    )
    assert old_config.is_file(), "the user's config file must NEVER be deleted"
    left = json.loads(old_config.read_text())
    assert "kmig-mcp" not in left["mcpServers"], (
        f"the stranded managed member must be spliced out of the old file: {left}"
    )
    assert left["mcpServers"]["user-server"]["command"] == "keep-me", (
        f"every byte the user owns must survive: {left}"
    )


# ── Junie ─────────────────────────────────────────────────────────────────


def test_global_install_junie_skill_lands_in_home_dot_junie(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Junie skill → ``$HOME/.junie/skills/<name>/`` (native dir)."""
    sk = make_artifact(
        f"{unique_repo}/junie-skill",
        "skill",
        {"junie-skill/SKILL.md": "---\nname: junie-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\njunie-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "junie")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".junie/skills/junie-skill/SKILL.md").is_file(), (
        "global Junie skill must land in $HOME/.junie/skills/"
    )


def test_global_install_junie_mcp_lands_in_home_dot_junie_mcp(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Global Junie MCP → ``$HOME/.junie/mcp/mcp.json``."""
    runner = GrimRunner(grim_binary, grim_home)
    ref = _release_global_mcp(runner, tmp_path, registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "junie")["items"]
    assert rows[0]["status"] == "installed", rows
    cfg = runner.home / ".junie/mcp/mcp.json"
    assert cfg.is_file(), "global Junie MCP entry must land in $HOME/.junie/mcp/mcp.json"
    assert json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]["command"] == "grim"


# ── Gemini (skills via shared pool; native agents) ─────────────────────────


def test_global_install_gemini_skill_lands_in_shared_agents_skills(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Gemini skill → the shared ``$HOME/.agents/skills/<name>/`` pool
    (same-tier precedence favors ``.agents/skills`` over ``.gemini/skills``)."""
    sk = make_artifact(
        f"{unique_repo}/gem-skill",
        "skill",
        {"gem-skill/SKILL.md": "---\nname: gem-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\ngem-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "gemini")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".agents/skills/gem-skill/SKILL.md").is_file(), (
        "global Gemini skill must land in the shared $HOME/.agents/skills pool"
    )
    assert not (runner.home / ".gemini/skills/gem-skill").exists(), (
        "Gemini skills must NOT double-install into a native .gemini/skills dir"
    )


def test_global_install_gemini_agent_lands_in_home_dot_gemini(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Gemini agent → ``$HOME/.gemini/agents/<name>.md`` (native file)."""
    ag = make_artifact(
        f"{unique_repo}/gem-agent",
        "agent",
        {"gem-agent.md": "---\nname: gem-agent\ndescription: d\nmodel: sonnet\n---\n# a\nbody\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[agents]\ngem-agent = "{ag.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "gemini")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".gemini/agents/gem-agent.md").is_file(), (
        "global Gemini agent must land in $HOME/.gemini/agents/<name>.md"
    )


# ── Zed (skills via shared pool; MCP under ~/.config/zed) ───────────────────


def test_global_install_zed_skill_lands_in_shared_agents_skills(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Zed skill → the shared ``$HOME/.agents/skills/<name>/`` pool."""
    sk = make_artifact(
        f"{unique_repo}/zed-skill",
        "skill",
        {"zed-skill/SKILL.md": "---\nname: zed-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nzed-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "zed")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".agents/skills/zed-skill/SKILL.md").is_file(), (
        "global Zed skill must land in the shared $HOME/.agents/skills pool"
    )


def test_global_install_zed_mcp_lands_in_config_zed_settings(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Global Zed MCP → ``$HOME/.config/zed/settings.json``, key
    ``context_servers`` — proving the ``~/.config/zed`` global root."""
    runner = GrimRunner(grim_binary, grim_home)
    ref = _release_global_mcp(runner, tmp_path, registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "zed")["items"]
    assert rows[0]["status"] == "installed", rows
    cfg = runner.home / ".config/zed/settings.json"
    assert cfg.is_file(), "global Zed MCP entry must land in $HOME/.config/zed/settings.json"
    assert json.loads(cfg.read_text())["context_servers"]["grim-mcp"]["command"] == "grim"


# ── Amp (skills via shared pool; MCP under ~/.config/amp) ───────────────────


def test_global_install_amp_skill_lands_in_shared_agents_skills(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Global Amp skill → the shared ``$HOME/.agents/skills/<name>/`` pool."""
    sk = make_artifact(
        f"{unique_repo}/amp-skill",
        "skill",
        {"amp-skill/SKILL.md": "---\nname: amp-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\namp-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "amp")["items"]
    assert rows[0]["status"] == "installed"
    assert (runner.home / ".agents/skills/amp-skill/SKILL.md").is_file(), (
        "global Amp skill must land in the shared $HOME/.agents/skills pool"
    )


def test_global_install_amp_mcp_lands_in_config_amp_settings(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Global Amp MCP → ``$HOME/.config/amp/settings.json``, literal dotted
    key ``amp.mcpServers`` — proving the ``~/.config/amp`` global root."""
    runner = GrimRunner(grim_binary, grim_home)
    ref = _release_global_mcp(runner, tmp_path, registry, unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "amp")["items"]
    assert rows[0]["status"] == "installed", rows
    cfg = runner.home / ".config/amp/settings.json"
    assert cfg.is_file(), "global Amp MCP entry must land in $HOME/.config/amp/settings.json"
    doc = json.loads(cfg.read_text())
    assert "amp.mcpServers" in doc, f"Amp global container key must be 'amp.mcpServers': {list(doc)}"
    assert doc["amp.mcpServers"]["grim-mcp"]["command"] == "grim"


def test_global_kiro_home_upgrade_uninstall_reaps_the_pre_override_root(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Ruling R8: the legacy-root reaper must also run on the **uninstall**
    path. Before this, a user who upgraded into a release honoring
    ``$KIRO_HOME`` and then ran ``grim uninstall`` *before* ever re-installing
    stranded the pre-override copy permanently — uninstall resolved only the
    new root, deleted nothing there, and dropped the record that was the only
    way left to find the old file."""
    sk = make_artifact(
        f"{unique_repo}/kuni-skill",
        "skill",
        {"kuni-skill/SKILL.md": "---\nname: kuni-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(f'[skills]\nkuni-skill = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "kiro")

    old_skill = runner.home / ".kiro/skills/kuni-skill"
    assert (old_skill / "SKILL.md").is_file()

    # The upgrade, with NO intervening install/update — straight to uninstall.
    kiro_home = grim_home.parent / "kiro_home_uninstall"
    kiro_home.mkdir()
    runner.env["KIRO_HOME"] = str(kiro_home)

    runner.json("uninstall", "--global", "skill", "kuni-skill")

    assert not old_skill.exists(), (
        "uninstall must reap the copy stranded at the pre-override root, "
        "not leave it unreachable with the record gone"
    )


def test_global_kiro_home_upgrade_uninstall_preserves_a_hand_edited_stray(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """The kept-modified promise holds on the uninstall path too: the reaper
    has no ``--force`` override, so a stranded copy the user edited is
    preserved. ``stability.md`` promises a modified file always survives, and
    that promise is what makes "a layout move is not a compatibility break"
    legitimate.

    Two skills, so the assertion is decidable: the untouched one proves the
    reaper actually ran on this pass, and the edited one proves it declined."""
    edited = make_artifact(
        f"{unique_repo}/kedit-skill",
        "skill",
        {"kedit-skill/SKILL.md": "---\nname: kedit-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    control = make_artifact(
        f"{unique_repo}/kctrl-skill",
        "skill",
        {"kctrl-skill/SKILL.md": "---\nname: kctrl-skill\ndescription: d\n---\n# body\n"},
        tag="v1",
    )
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\nkedit-skill = "{edited.fq}"\nkctrl-skill = "{control.fq}"\n'
    )
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "kiro")

    edited_doc = runner.home / ".kiro/skills/kedit-skill/SKILL.md"
    control_dir = runner.home / ".kiro/skills/kctrl-skill"
    assert edited_doc.is_file() and (control_dir / "SKILL.md").is_file()
    edited_doc.write_text("---\nname: kedit-skill\ndescription: d\n---\n# MINE\n")

    kiro_home = grim_home.parent / "kiro_home_uninstall_edit"
    kiro_home.mkdir()
    runner.env["KIRO_HOME"] = str(kiro_home)

    edited_report = runner.json("uninstall", "--global", "skill", "kedit-skill")
    control_report = runner.json("uninstall", "--global", "skill", "kctrl-skill")

    assert not control_dir.exists(), (
        "the control proves the uninstall-path reaper ran: an unmodified stray is reaped"
    )
    assert edited_doc.read_text().endswith("# MINE\n"), (
        "a hand-edited stranded copy must be preserved, never deleted"
    )
    # `retained` is the documented divergence channel: a non-empty array means
    # state and filesystem deliberately disagree and the listed paths must be
    # removed by hand. Dropping the record while silently leaving the file
    # would make that promise false exactly when it matters.
    retained = [Path(p).resolve() for p in edited_report["retained"]]
    assert edited_doc.parent.resolve() in retained or edited_doc.resolve() in retained, (
        f"the preserved stray must be reported in `retained`: {edited_report}"
    )
    assert control_report["retained"] == [], (
        f"a fully reaped uninstall must report no divergence: {control_report}"
    )


def test_global_kiro_home_upgrade_uninstall_abandons_a_hand_edited_stranded_entry(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """A stranded MCP member the user edited is preserved — and must therefore
    be *reported*. `abandoned_entries` is the field for exactly this: the entry
    is now unrecorded and grim will never remove it, because the kept-modified
    rule has no ``--force`` override. Without the report the only signal is a
    human-readable warning, which no automated consumer can see."""
    descriptor = tmp_path / "kaband_src" / "mcp" / "kaband-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["mcp"]\n'
    )
    ref = f"{registry}/{unique_repo}/mcp/kaband-mcp:1.0.0"
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("release", str(descriptor), ref, "--kind", "mcp")

    (grim_home / "grimoire.toml").write_text(f'[mcp]\nkaband-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    runner.json("install", "--global", "--client", "kiro")

    old_config = runner.home / ".kiro/settings/mcp.json"
    parsed = json.loads(old_config.read_text())
    assert parsed["mcpServers"]["kaband-mcp"]["command"] == "grim"
    # The user edits the managed member itself, so the recorded hash drifts.
    parsed["mcpServers"]["kaband-mcp"]["args"] = ["mcp", "--mine"]
    old_config.write_text(json.dumps(parsed, indent=2))

    kiro_home = grim_home.parent / "kiro_home_uninstall_entry"
    kiro_home.mkdir()
    runner.env["KIRO_HOME"] = str(kiro_home)

    report = runner.json("uninstall", "--global", "mcp", "kaband-mcp")

    left = json.loads(old_config.read_text())
    assert left["mcpServers"]["kaband-mcp"]["args"] == ["mcp", "--mine"], (
        f"the hand-edited stranded member must be preserved: {left}"
    )
    assert report["abandoned_entries"], (
        f"a preserved stranded entry must be reported, not only warned about: {report}"
    )
    entry = report["abandoned_entries"][0]
    assert Path(entry["path"]).resolve() == old_config.resolve(), entry
    assert "kaband-mcp" in entry["pointer"], entry
    assert report["retained"] == [], (
        f"a config file the user owns is never grim's to name in `retained`: {report}"
    )
