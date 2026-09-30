# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Regression tests for the WP-01 review round of the hooks revival
(`design_hooks_pr98_revival.md`): the spec review's B1/B2 and the security
review's B1/W2, each reproduced against the real binary.

Every negative is paired with a positive control, so a build that simply does
nothing cannot pass.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "python3 guard.py"
timeout = 5
"""

GUARD_PY = "import sys\nsys.stdin.read()\nprint('{}')\n"

# `validate_registries` rejects the uncompilable glob, so loading it exits 78.
MALFORMED_GLOBAL_CONFIG = (
    '[[registries]]\nalias = "acme"\noci = "ghcr.io/acme"\ninclude = ["acme{unclosed"]\n'
)

# A Codex hook file the user wrote by hand — grim never wrote it.
USER_CODEX_HOOKS = '{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo mine"}]}]}}\n'

posix_only = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)


def _publish_hook(unique_repo: str):
    files = {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.py": GUARD_PY}
    return make_artifact(f"{unique_repo}/shell-guard", "hook", files, tag="1")


def _dispatch_clients(grim_home: Path) -> set[str]:
    path = grim_home / "hooks" / "dispatch.json"
    if not path.is_file():
        return set()
    table = json.loads(path.read_text())
    return {row["client"] for root in table["roots"].values() for row in root["hooks"]}


# ---------------------------------------------------------------------------
# Spec review B1 — hook-free commands never read the global config
# ---------------------------------------------------------------------------


def test_hook_free_commands_ignore_a_malformed_global_config(grim_at, project_dir: Path) -> None:
    """**Spec B1.** A hook-free project under a global config that exits 78 on
    load: `add ./path`, `install` and `update` exit 0 exactly as main's do —
    with the feature off nothing can arm, so the hook consent pass has no reason
    to read the global config. `add` also completes its install instead of
    failing after writing `grimoire.toml` and the lock."""
    skill_dir = project_dir / "skills" / "demo"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("---\nname: demo\ndescription: A demo skill.\n---\n\nBody.\n")
    write_config(project_dir)
    runner = grim_at(project_dir)
    (Path(runner.grim_home) / "grimoire.toml").write_text(MALFORMED_GLOBAL_CONFIG)

    for args in (("add", "./skills/demo"), ("install",), ("update",)):
        result = runner.run(*args, check=False)
        assert result.returncode == 0, f"grim {' '.join(args)}: rc={result.returncode} {result.stderr}"
    assert (project_dir / ".claude" / "skills" / "demo" / "SKILL.md").is_file(), "add must install the skill"

    # Positive control: the same broken file still surfaces once the feature
    # is on, because then the global `insecure` hosts decide what may arm.
    runner.run("config", "set", "options.experimental.hooks", "true")
    refused = runner.run("install", check=False)
    assert refused.returncode == 78, f"rc={refused.returncode} {refused.stderr}"


# ---------------------------------------------------------------------------
# Spec review B2 — a skill that ships a hook.toml stays a skill
# ---------------------------------------------------------------------------


def test_a_skill_shipping_a_hook_toml_still_builds_as_a_skill(grim, tmp_path: Path) -> None:
    """**Spec B2.** `SKILL.md` wins kind inference over `hook.toml`, so a skill
    whose tree carries an (ignored) `hook.toml` builds as a skill, exit 0 — as
    it did before the hook kind existed. `--kind hook` still selects the hook."""
    skill = tmp_path / "hookish"
    skill.mkdir()
    (skill / "SKILL.md").write_text("---\nname: hookish\ndescription: Ships a stray hook.toml.\n---\n\nBody.\n")
    (skill / "hook.toml").write_text('command = "not a hook manifest"\n')
    (skill / ".grimignore").write_text("hook.toml\n")

    report = grim.json("build", str(skill))
    assert report["kind"] == "skill", report

    forced = grim.run("build", str(skill), "--kind", "hook", check=False)
    assert forced.returncode == 65, f"--kind hook must still parse it as a hook: {forced.stderr}"


# ---------------------------------------------------------------------------
# Security review W2 — `--no-trust-hooks` is never persisted as consent
# ---------------------------------------------------------------------------


def test_add_no_trust_hooks_writes_no_consent_record(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**Security W2.** `grim add --no-trust-hooks <hook>` is this run's "no":
    it must not write the workspace consent record the next plain
    `grim install` would arm on. Positive control: a plain `grim add` of the
    same hook does write it."""
    hook = _publish_hook(unique_repo)
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    consent = Path(runner.grim_home) / "hooks" / "consent"

    runner.run("add", "--no-trust-hooks", "--kind", "hook", hook.fq)
    assert not consent.exists() or not any(consent.iterdir()), sorted(consent.iterdir())

    runner.run("remove", "hook", "shell-guard")
    runner.run("add", "--kind", "hook", hook.fq)
    assert consent.is_dir() and len(list(consent.iterdir())) == 1, "POSITIVE CONTROL: add records consent"


# ---------------------------------------------------------------------------
# Security review B1 — a user's own Codex hooks.json is never deleted or
# overwritten
# ---------------------------------------------------------------------------


def _codex_home(grim) -> Path:
    codex = Path(grim.home) / ".codex"
    codex.mkdir(parents=True, exist_ok=True)
    return codex


@posix_only
def test_flag_off_install_leaves_a_user_codex_hooks_file_alone(grim, registry: str, unique_repo: str) -> None:
    """**Security B1, flag off.** A global `grim install` with the feature off
    converges Codex's own-file surface to "nothing". A `~/.codex/hooks.json`
    grim did not write is the user's, so it survives byte for byte — the
    path-is-ownership model deleted it."""
    hooks_json = _codex_home(grim) / "hooks.json"
    hooks_json.write_text(USER_CODEX_HOOKS)
    skill = make_artifact(f"{unique_repo}/demo", "skill", {"demo/SKILL.md": "---\nname: demo\n---\n"})
    (Path(grim.grim_home) / "grimoire.toml").write_text(f'[skills]\ndemo = "{skill.fq}"\n')
    grim.run("lock", "--global")

    grim.run("install", "--global")

    assert hooks_json.read_text() == USER_CODEX_HOOKS, "a user's Codex hooks.json must never be deleted"


@posix_only
def test_arming_never_overwrites_a_user_codex_hooks_file(grim, registry: str, unique_repo: str) -> None:
    """**Security B1, flag on.** Arming a hook globally over a user-authored
    `~/.codex/hooks.json` leaves that file untouched, arms nothing for Codex,
    and `grim status` names the cause `surface-user-owned`. Positive control:
    Claude, whose registration is a marked splice, still arms."""
    hooks_json = _codex_home(grim) / "hooks.json"
    hooks_json.write_text(USER_CODEX_HOOKS)
    (Path(grim.home) / ".claude").mkdir(parents=True, exist_ok=True)
    hook = _publish_hook(unique_repo)
    grim_home = Path(grim.grim_home)
    (grim_home / "grimoire.toml").write_text(
        f'[hooks]\nshell-guard = "{hook.fq}"\n\n[options.experimental]\nhooks = true\n'
    )
    grim.run("lock", "--global")

    result = grim.run("install", "--global", "--trust-hooks")

    assert hooks_json.read_text() == USER_CODEX_HOOKS, "a user's Codex hooks.json must never be overwritten"
    assert "move that file aside" in result.stderr, result.stderr
    clients = _dispatch_clients(grim_home)
    assert "claude" in clients, f"POSITIVE CONTROL: claude must arm — {clients}"
    assert "codex" not in clients, f"no dispatch row without a registration — {clients}"
    item = next(i for i in grim.json("status", "--global")["items"] if i["kind"] == "hook")
    causes = {a["client"]: a["cause"] for a in item["arming"]}
    assert causes.get("codex") == "surface-user-owned", item

    # Once the user moves their file aside, grim writes — and owns — its own.
    hooks_json.unlink()
    grim.run("install", "--global", "--trust-hooks")
    assert "codex" in _dispatch_clients(grim_home) and hooks_json.is_file()
