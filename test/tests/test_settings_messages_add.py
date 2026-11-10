# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What ``grim add`` says about a dependency's settings value it would not overwrite.

A value grim wrote and the user then changed is not one "grim did not create",
so the hint names both causes, the way the install refusal does.
"""

from __future__ import annotations

import json
from pathlib import Path

from src.helpers import (
    make_artifact,
    make_dependent,
    make_settings,
    settings_sandbox,
    write_config,
)

LOCAL = ".claude/settings.local.json"
SKILL = "---\nname: {name}\ndescription: d\n---\n# {name}\n"


def test_add_names_a_changed_dependency_value_as_changed(
    grim_at, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    make_settings(
        f"{unique_repo}/team",
        "1",
        {"description": "team", "claude": {"model": "sonnet"}},
    )
    carrier = make_dependent(
        f"{unique_repo}/carrier",
        "1",
        "skill",
        {"carrier/SKILL.md": SKILL.format(name="carrier")},
        [("settings", "team", "./team:1")],
    )
    other = make_artifact(
        f"{unique_repo}/other",
        "skill",
        {"other/SKILL.md": SKILL.format(name="other")},
        tag="1",
    )
    (project_dir / ".claude").mkdir(exist_ok=True)
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.env.update(settings_sandbox(tmp_path / "sandbox", runner.home)[0])
    runner.run("config", "set", "options.experimental.settings", "true")
    runner.run("add", carrier.fq)
    local = project_dir / LOCAL
    assert json.loads(local.read_text(encoding="utf-8"))["model"] == "sonnet"
    local.write_text(json.dumps({"model": "opus"}), encoding="utf-8", newline="\n")

    refused = runner.run("add", "--force", other.fq, check=False)

    assert refused.returncode == 65, refused.stderr
    assert (
        "dependencies set or changed outside grim were not overwritten: settings 'team' /model"
        in refused.stderr
    )
    assert "not created by grim" not in refused.stderr
    assert "run `grim install --force` to overwrite them" in refused.stderr
    assert json.loads(local.read_text(encoding="utf-8")) == {"model": "opus"}
    # Positive control: the remedy the hint names overwrites the value.
    runner.run("install", "--force")
    assert json.loads(local.read_text(encoding="utf-8"))["model"] == "sonnet"
