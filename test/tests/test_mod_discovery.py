# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`serves` on `grim describe` and `grim search` for a mod.

The list comes from the ``com.grimoire.mod.clients`` annotation (sorted), and is
``null`` when the source never published it, and for every other kind.
"""

from __future__ import annotations

from pathlib import Path

from src.helpers import (
    _mod_manifest,
    make_artifact,
    make_mod,
    make_raw_mod,
    tar_of,
)
from src.registry import LAYER_MEDIA_TYPE, REGISTRY_HOST


def _row(runner, unique_repo: str, name: str) -> dict:
    items = runner.json(
        "search",
        unique_repo,
        "--registry",
        f"{REGISTRY_HOST}/{unique_repo}",
        "--refresh",
    )["items"]
    return next(r for r in items if r["repo"].endswith(f"{unique_repo}/{name}"))


def test_describe_and_search_show_serves(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    mod = make_mod(f"{unique_repo}/served", "latest", clients="opencode,claude")
    skill = make_artifact(
        f"{unique_repo}/plain",
        "skill",
        {"plain/SKILL.md": "---\nname: plain\ndescription: d\n---\n# plain\n"},
        tag="latest",
    )
    runner = grim_at(project_dir)

    described = runner.json("describe", mod.fq)
    assert described["kind"] == "mod"
    assert described["serves"] == ["claude", "opencode"]

    assert _row(runner, unique_repo, "served")["serves"] == ["claude", "opencode"]

    # Positive control: another kind in the same listing carries the key as null.
    assert runner.json("describe", skill.fq)["serves"] is None
    assert _row(runner, unique_repo, "plain")["serves"] is None


def test_serves_is_null_without_the_annotation(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    tar = tar_of(
        {
            "bare/mod.toml": _mod_manifest("bare"),
            "bare/claude/index.js": "export default {};\n",
        }
    )
    bare = make_raw_mod(
        f"{unique_repo}/bare", "latest", [(LAYER_MEDIA_TYPE, tar)], clients=None
    )
    served = make_mod(f"{unique_repo}/served", "latest", clients="claude")
    runner = grim_at(project_dir)

    assert runner.json("describe", bare.fq)["serves"] is None
    assert _row(runner, unique_repo, "bare")["serves"] is None

    # Positive control: the same listing reads the annotation where it exists.
    assert runner.json("describe", served.fq)["serves"] == ["claude"]
    assert _row(runner, unique_repo, "served")["serves"] == ["claude"]
