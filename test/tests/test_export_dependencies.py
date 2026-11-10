# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim export plugin` / `marketplace` flatten an artifact's dependency closure.

Every node reachable from an exported artifact through its `requires` edges is
written into the plugin as if it were a member. The plugin manifest carries no
`dependencies` key, and a node two exported plugins both reach is written into
both. A closure node demanding a feature this grim does not know refuses the
export (65, `unsupported-artifact-feature`) before anything is placed.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, make_dependent
from src.runner import GrimRunner

FEATURES_KEY = "com.grimoire.requires-features"


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    d = tmp_path / "work"
    d.mkdir()
    return d


def _skill_files(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: Demo {name}.\n---\n# {name}\n"}


def _leaf(unique_repo: str, name: str, **kw):
    return make_artifact(f"{unique_repo}/{name}", "skill", _skill_files(name), tag="1", **kw)


def _dependent(unique_repo: str, name: str, requires: list[tuple[str, str, str]]):
    return make_dependent(f"{unique_repo}/{name}", "1", "skill", _skill_files(name), requires)


def _export(runner: GrimRunner, *args: str):
    return runner.run("export", "plugin", *args, format="json", check=False)


def _skills(plugin_root: Path) -> list[str]:
    d = plugin_root / "skills"
    return sorted(p.name for p in d.iterdir()) if d.exists() else []


def _error(result) -> dict:
    return json.loads(result.stdout)["error"]


def test_plugin_flattens_dependencies(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    rule = make_artifact(f"{unique_repo}/style", "rule", {"style.md": "# Style\n"}, tag="1")
    c = _leaf(unique_repo, "c")
    b = _dependent(unique_repo, "b", [("skill", "c", "./c:1"), ("rule", "style", "./style:1")])
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    runner = grim_at(work)

    result = _export(runner, a.fq, "--name", "stack", "--client", "claude", "-o", "dist")

    assert result.returncode == 0, result.stderr
    root = work / "dist" / "stack.claude"
    # The exported skill and its whole closure, the transitive rule included.
    assert _skills(root) == ["a", "b", "c"]
    assert (root / "skills" / "c" / "SKILL.md").read_text(encoding="utf-8").startswith("---\nname: c\n")
    members = {(m["kind"], m["name"]) for m in json.loads(result.stdout)["items"][0]["members"]}
    omitted = {(m["kind"], m["name"]) for m in json.loads(result.stdout)["items"][0]["omitted"]}
    assert ("skill", "a") in members and ("skill", "c") in members
    assert ("rule", "style") in members | omitted, "the transitive rule is a plugin member, kept or omitted for the client"
    # No dependency edge leaks into the manifest.
    manifest = json.loads((root / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))
    assert "dependencies" not in manifest, manifest
    # Positive control: the closure is what grew the plugin — `c` alone exports just itself.
    solo = _export(runner, c.fq, "--name", "solo", "--client", "claude", "-o", "dist")
    assert solo.returncode == 0, solo.stderr
    assert _skills(work / "dist" / "solo.claude") == ["c"]


def test_export_refuses_unknown_feature(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _leaf(unique_repo, "future", annotations={FEATURES_KEY: "future-thing"})
    a = _dependent(unique_repo, "a", [("skill", "future", "./future:1")])
    runner = grim_at(work)

    result = _export(runner, a.fq, "--name", "stack", "--client", "claude", "-o", "dist")

    assert result.returncode == 65, result.stderr
    error = _error(result)
    assert error["reason"] == "unsupported-artifact-feature", error
    assert "future-thing" in error["message"], error
    assert not (work / "dist").exists() or not list((work / "dist").iterdir()), "nothing placed"
    # Positive control: the same shape without the unknown feature exports.
    _leaf(unique_repo, "ok")
    fine = _dependent(unique_repo, "fine", [("skill", "ok", "./ok:1")])
    good = _export(runner, fine.fq, "--name", "good", "--client", "claude", "-o", "dist")
    assert good.returncode == 0, good.stderr
    assert _skills(work / "dist" / "good.claude") == ["fine", "ok"]


def test_shared_dependency_in_both_plugins(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    _leaf(unique_repo, "shared")
    a = _dependent(unique_repo, "a", [("skill", "shared", "./shared:1")])
    b = _dependent(unique_repo, "b", [("skill", "shared", "./shared:1")])
    (work / "marketplace.toml").write_text(
        '[marketplace]\nname = "acme"\nclients = ["claude"]\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f'[plugins.one]\ninclude = ["{a.fq}"]\n\n[plugins.two]\ninclude = ["{b.fq}"]\n', encoding="utf-8", newline="\n"
    )
    runner = grim_at(work)

    result = runner.run("export", "marketplace", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert _skills(work / "claude" / "one") == ["a", "shared"]
    assert _skills(work / "claude" / "two") == ["b", "shared"]
    for plugin in ("one", "two"):
        manifest = json.loads((work / "claude" / plugin / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))
        assert "dependencies" not in manifest, manifest


def test_plugin_flattens_required_bundle(grim_at, work: Path, registry: str, unique_repo: str) -> None:
    s1 = _leaf(unique_repo, "s1")
    s2 = _leaf(unique_repo, "s2")
    make_bundle(f"{unique_repo}/pack", [("skill", "s1", s1.fq), ("skill", "s2", s2.fq)], tag="1")
    a = _dependent(unique_repo, "a", [("bundle", "pack", "./pack:1")])
    runner = grim_at(work)

    result = _export(runner, a.fq, "--name", "stack", "--client", "claude", "-o", "dist")

    assert result.returncode == 0, result.stderr
    assert _skills(work / "dist" / "stack.claude") == ["a", "s1", "s2"]
