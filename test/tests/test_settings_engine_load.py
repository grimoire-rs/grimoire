# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""How often one run reads each settings layer.

The conflict check before anything lands and the settings step after it both
need every armed layer, and a run reads each one once: a warning the layer
gives appears once per run, however many packages are armed.
"""

from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_settings, settings_sandbox, write_config
from src.runner import GrimRunner

UNKNOWN = "settings package 'team' carries a table for 'zed-next', a client this grim does not know"


def test_each_layer_is_read_once_per_run(grim: GrimRunner, grim_home: Path, tmp_path: Path, unique_repo: str) -> None:
    team = make_settings(
        f"{unique_repo}/team", "1", {"description": "d", "copilot": {"model": "m"}, "zed-next": {"x": 1}}
    )
    extra = make_settings(f"{unique_repo}/extra", "1", {"description": "d", "copilot": {"allowedUrls": ["a"]}})
    write_config(
        grim_home,
        settings={"team": team.fq, "extra": extra.fq},
        options={"clients": json.dumps(["copilot"])},
        experimental_settings=True,
    )
    env, roots = settings_sandbox(tmp_path / "s", grim.home)
    grim.env.update(env)
    grim.run("--global", "lock")

    result = grim.run("--global", "install")

    assert result.returncode == 0, result.stderr
    assert result.stderr.count(UNKNOWN) == 1, result.stderr
    settings = json.loads((roots["copilot"] / "settings.json").read_text(encoding="utf-8"))
    assert settings == {"model": "m", "allowedUrls": ["a"]}
