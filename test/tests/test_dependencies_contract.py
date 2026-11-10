# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Seams between the dependency pipelines, checked end to end.

``status`` over a dependency-free project, and ``lock`` / ``why`` / ``remove``
over a real transitive chain, each exercised through the shipped binary.
"""
from __future__ import annotations

import json
import os
import re
from pathlib import Path

import pytest

from src import registry as reg
from src.helpers import make_artifact, make_dependent, write_config

# Reuses the golden module's replay and environment, not a second copy of them.
# The module-level guards there fire on import under GRIM_REQUIRE_GOLDEN=1.
from test_golden_pre_dependencies import (
    _GOLDEN_HOST,
    _HOW_TO_RUN,
    FIXTURE,
    _Env,
    _replay_registry,
)


def test_golden_status_fields(grim_binary, grim_at, tmp_path: Path) -> None:
    """Every status item of a 0.15.1 project carries the new fields, `source` unchanged."""
    if reg.REGISTRY_HOST != _GOLDEN_HOST:
        message = f"GRIM_REQUIRE_GOLDEN=1: {_HOW_TO_RUN}"
        if os.environ.get("GRIM_REQUIRE_GOLDEN") == "1":
            pytest.fail(message, pytrace=False)
        pytest.skip(_HOW_TO_RUN)
    _replay_registry()
    env = _Env(grim_at, tmp_path, seed_locks=True)
    env.lock_and_install()

    for scope, runner, args in (
        ("project", env.project_runner, ()),
        ("global", env.global_runner, ("--global",)),
    ):
        recorded = json.loads((FIXTURE / "provenance" / f"status.{scope}.json").read_text(encoding="utf-8"))
        want = {(i["kind"], i["name"]): i["source"] for i in recorded["items"]}
        items = runner.json("status", *args)["items"]
        assert want and items, f"{scope}: no items to check (positive control)"
        for item in items:
            where = f"{scope} {item['kind']}/{item['name']}"
            assert isinstance(item["direct"], bool), f"{where}: `direct` missing or not a bool"
            assert item["required_by"] is not None, f"{where}: `required_by` is null"
        assert {(i["kind"], i["name"]): i["source"] for i in items} == want, (
            f"{scope}: status `source` per item differs from 0.15.1"
        )


def _skill(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}


def _chain(path: list[dict]) -> str:
    return " -> ".join(f"{n['kind']}/{n['name']}" for n in path)


def test_transitive_chain_lock_why_remove(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # a -> b -> c, published with relative edges as a publisher would write them.
    make_artifact(f"{unique_repo}/c", "skill", _skill("c"), tag="1")
    make_dependent(f"{unique_repo}/b", "1", "skill", _skill("b"), [("skill", "c", "./c:1")])
    a = make_dependent(f"{unique_repo}/a", "1", "skill", _skill("a"), [("skill", "b", "./b:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    lock = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    locked = set(re.findall(r'(?m)^\[\[skill\]\]\nname = "([^"]+)"', lock))
    assert locked == {"a", "b", "c"}, "the whole chain is locked from one direct entry"

    plain = runner.plain("why", "skill", "c")
    assert "skill/a -> skill/b -> skill/c" in plain.stdout, plain.stdout
    doc = runner.json("why", "skill", "c")
    assert doc["direct"] is False
    assert [_chain(path) for path in doc["paths"]] == ["skill/a -> skill/b -> skill/c"]

    out = runner.json("remove", "skill", "a")

    assert out["status"] == "removed"
    assert "a" not in (project_dir / "grimoire.toml").read_text(encoding="utf-8").split("[skills]")[-1]
    remaining = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert not re.findall(r'(?m)^\[\[skill\]\]', remaining), (
        "b and c leave the lock with a, nothing else reaches them:\n" + remaining
    )
