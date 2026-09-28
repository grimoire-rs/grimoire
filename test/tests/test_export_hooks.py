# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim export plugin` with a hook member (design record S-109, C-140…C-142).

An exported plugin runs without grim, and a hook needs grim's runtime, so a
hook member is declined: a warning once per hook per plugin (C-141), the
existing README `Omitted for …` line and JSON `omitted[]` row (C-142), with
the reason per family (C-140) — `not-representable` for the Claude family
(its format has a hook file that cannot carry one), `no-format-surface` for
Agent Plugins (no hook file at all).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner

HOOK_TOML = """\
schema = 1
name = "g"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

GUARD_SH = "#!/bin/sh\ncat > /dev/null\nprintf '{}'\n"
WARNING = "hook 'g' omitted from plugin 'team': exported plugins run without grim, and hooks need grim's hook runtime"


def _project(
    grim_at, root: Path, registry: str, unique_repo: str, *, skill: bool
) -> GrimRunner:
    """A locked project declaring hook `g` (and skill `a` when ``skill``), hooks flag on."""
    make_artifact(
        f"{unique_repo}/g",
        "hook",
        {"g/hook.toml": HOOK_TOML, "g/guard.sh": GUARD_SH},
        tag="1",
    )
    skills = {}
    if skill:
        make_artifact(
            f"{unique_repo}/a",
            "skill",
            {"a/SKILL.md": "---\nname: a\ndescription: Demo skill.\n---\n# A\n"},
            tag="1",
        )
        skills = {"a": f"{registry}/{unique_repo}/a:1"}
    root.mkdir(parents=True, exist_ok=True)
    write_config(root, skills=skills, hooks={"g": f"{registry}/{unique_repo}/g:1"})
    config = root / "grimoire.toml"
    config.write_text(
        config.read_text()
        + '\n[plugin]\nname = "team"\n\n[options.experimental]\nhooks = true\n'
    )
    runner = grim_at(root)
    runner.run("lock")
    return runner


def _export(runner: GrimRunner, *clients: str) -> subprocess.CompletedProcess[str]:
    args = [a for c in clients for a in ("--client", c)]
    return runner.run(
        "export", "plugin", "--project", *args, "-o", "dist", format="json", check=False
    )


def test_s109_hook_member_is_warned_listed_and_exit_0(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, skill=True)

    result = _export(runner, "claude", "codex")

    assert result.returncode == 0, result.stderr
    # C-141: one warning per hook per plugin, not one per client.
    assert result.stderr.count(WARNING) == 1, result.stderr
    items = {item["client"]: item for item in json.loads(result.stdout)["items"]}
    assert items["claude"]["omitted"] == [
        {"kind": "hook", "name": "g", "reason": "not-representable"}
    ]
    assert items["codex"]["omitted"] == [
        {"kind": "hook", "name": "g", "reason": "no-format-surface"}
    ]
    for client in ("claude", "codex"):
        root = proj / "dist" / f"team.{client}"
        assert f"Omitted for {client}: hook g." in (root / "README.md").read_text()
        assert (root / "skills" / "a" / "SKILL.md").is_file()
        assert not (root / "hooks").exists(), "nothing of the hook is staged"


def test_s109_hooks_only_plugin_is_empty_65(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    proj = tmp_path / "proj"
    runner = _project(grim_at, proj, registry, unique_repo, skill=False)

    result = _export(runner, "claude")

    assert result.returncode == 65, result.stderr
    assert "has no member the 'claude' plugin format can carry" in result.stderr, (
        result.stderr
    )
    assert not (proj / "dist" / "team.claude").exists()
