# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Done 3: rules and agents publish beside skills; kinds the mirror refuses are skipped rows."""
from __future__ import annotations

import json
from pathlib import Path

from src.git_fixture import BASE_DATE, GitRepo, agent_md, monorepo, skill_md

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")


def _write(tmp_path: Path, registry: str, unique_repo: str, upstream: str) -> Path:
    path = tmp_path / "mirror.toml"
    path.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n[[upstream]]\nname = "up"\n'
        f'repo = "{upstream}"\n',
        encoding="utf-8",
        newline="\n",
    )
    return path


def _plugin_repo(path: Path) -> GitRepo:
    repo = GitRepo.init(path)
    market = {
        "name": "fixture-market",
        "owner": {"name": "Fixture Owner"},
        "plugins": [{"name": "full", "source": "./plugins/full"}],
    }
    repo.record(
        "plugin carrying every component kind",
        {
            ".claude-plugin/marketplace.json": json.dumps(market),
            "LICENSE": MIT_TEXT,
            "plugins/full/skills/helper/SKILL.md": skill_md("helper"),
            "plugins/full/agents/checker.md": agent_md("checker"),
            "plugins/full/commands/deploy.md": "---\ndescription: Deploy.\n---\nDeploy it.\n",
            "plugins/full/hooks/hooks.json": json.dumps({"hooks": {"PreToolUse": []}}),
            "plugins/full/.mcp.json": json.dumps({"mcpServers": {"srv": {"command": "srv"}}}),
            "plugins/full/settings.json": json.dumps({"agent": "checker"}),
        },
    )
    return repo


def test_rules_and_agents_publish_with_their_own_kind(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = monorepo(tmp_path / "up", skills=("alpha",), rules=("style",), agents=("reviewer",), license_text=MIT_TEXT)
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n[[upstream]]\nname = "up"\n'
        f'repo = "{repo.url}"\nskills = ["skills/*"]\nrules = ["rules/*.md"]\nagents = ["agents/*.md"]\n',
        encoding="utf-8",
        newline="\n",
    )

    report = json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync").stdout)

    assert {(i["kind"], i["status"]) for i in report["items"]} == {
        ("skill", "published"),
        ("rule", "published"),
        ("agent", "published"),
    }
    for item in report["items"]:
        assert grim_at(tmp_path).json("describe", item["ref"])["kind"] == item["kind"]


def test_agent_named_differently_from_its_file_publishes_under_its_declared_name(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = GitRepo.init(tmp_path / "up")
    repo.record(
        "agent whose name is not its file stem",
        {"LICENSE": MIT_TEXT, "agents/review.md": agent_md("reviewer")},
        date=BASE_DATE,
    )
    mf = _write(tmp_path, registry, unique_repo, repo.url)
    mf.write_text(mf.read_text(encoding="utf-8") + 'agents = ["agents/*.md"]\n', encoding="utf-8", newline="\n")

    report = json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)

    (item,) = report["items"]
    assert (item["kind"], item["name"], item["status"]) == ("agent", "reviewer", "published"), item
    assert item["ref"].startswith(f"{registry}/{unique_repo}/up/agents/reviewer:")
    d = grim_at(tmp_path).json("describe", item["ref"])
    assert d["kind"] == "agent"


def test_rule_whose_frontmatter_name_differs_from_its_file_publishes_under_its_file_name(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = GitRepo.init(tmp_path / "up")
    repo.record(
        "rule whose frontmatter name is not its file stem",
        {"LICENSE": MIT_TEXT, "rules/style.md": "---\nname: house-style\ndescription: House style.\n---\n\nBe terse.\n"},
        date=BASE_DATE,
    )
    mf = _write(tmp_path, registry, unique_repo, repo.url)
    mf.write_text(mf.read_text(encoding="utf-8") + 'rules = ["rules/*.md"]\n', encoding="utf-8", newline="\n")

    report = json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)

    (item,) = report["items"]
    assert (item["kind"], item["name"], item["status"]) == ("rule", "style", "published"), item
    assert item["ref"].startswith(f"{registry}/{unique_repo}/up/rules/style:")
    d = grim_at(tmp_path).json("describe", item["ref"])
    assert d["kind"] == "rule"
    assert d["annotations"]["com.grimoire.mirror.path"] == "rules/style.md"


def test_plugin_components_the_mirror_does_not_carry_are_skipped_with_a_reason(
    grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = _plugin_repo(tmp_path / "up")
    mf = _write(tmp_path, registry, unique_repo, repo.url)
    mf.write_text(
        mf.read_text(encoding="utf-8") + 'marketplace = ".claude-plugin/marketplace.json"\nplugins = ["full"]\n',
        encoding="utf-8",
        newline="\n",
    )

    r = grim_mirror.run("--file", str(mf), "--format", "json", "sync")

    assert r.returncode == 0
    report = json.loads(r.stdout)
    rows = {(i["kind"], i["name"]): i for i in report["items"] if i["plugin"] == "full"}
    skipped = {k: v for k, v in rows.items() if v["status"] == "skipped"}
    assert {kind for kind, _ in skipped} == {"command", "hook", "mcp", "settings"}
    for item in skipped.values():
        assert item["reason"] == "kind-unsupported", item
    # The carried kinds still publish beside them.
    assert rows[("skill", "helper")]["status"] == "published"
    assert rows[("agent", "checker")]["status"] == "published"
    assert report["summary"]["skipped"] == 4
    runner = grim_at(tmp_path)
    for kind, name in (("skill", "helper"), ("agent", "checker")):
        assert runner.json("describe", rows[(kind, name)]["ref"])["kind"] == kind
    # A skipped row publishes nothing: no repository exists under its name.
    for kind_dir, name in (("commands", "deploy"), ("hooks", "hooks"), ("mcp", "mcp"), ("settings", "settings")):
        ref = f"{registry}/{unique_repo}/up/full/{kind_dir}/{name}"
        assert runner.run("describe", ref, check=False).returncode == 79, ref
