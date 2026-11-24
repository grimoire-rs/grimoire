# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Done 2: a declared bundle publishes as one artifact of exact-version members."""
from __future__ import annotations

import json
from pathlib import Path

from src.git_fixture import monorepo, skill_md
from src.helpers import write_config
from src.registry import fetch_blob, fetch_manifest

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")


def _mirror_file(tmp_path: Path, registry: str, unique_repo: str, repo_url: str, members: str, extra: str = "") -> Path:
    path = tmp_path / "mirror.toml"
    path.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo_url}"\n'
        f'skills = ["skills/*"]\nrules = ["rules/*.md"]\nagents = ["agents/*.md"]\n{extra}\n'
        f'[[upstream.bundle]]\nname = "kit"\nmembers = {members}\n',
        encoding="utf-8",
        newline="\n",
    )
    return path


def _sync(grim_mirror, mf: Path, *flags: str, check: bool = True):
    return grim_mirror.run("--file", str(mf), "--format", "json", *flags, check=check)


def _bundle(report: dict) -> dict:
    return next(i for i in report["items"] if i["kind"] == "bundle")


def test_declared_bundle_lists_members_as_absolute_exact_refs(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, '["skills/*", "rules/style.md"]')

    report = json.loads(_sync(grim_mirror, mf, "sync").stdout)

    bundle = _bundle(report)
    assert bundle["name"] == "kit"
    assert bundle["status"] == "published", bundle
    members = {i["ref"] for i in report["items"] if i["kind"] in ("skill", "rule") and i["name"] != "naming"}
    assert members and len(members) == 4  # alpha, beta, gamma, style
    assert set(bundle["members"]) == members
    for ref in bundle["members"]:
        assert ref.startswith(f"{registry}/{unique_repo}/up/"), ref
        assert ref.rsplit(":", 1)[1][0].isdigit(), ref  # exact version, not a floating tag
    # The published layer carries the same references the report names.
    repository, version = bundle["ref"].removeprefix(f"{registry}/").rsplit(":", 1)
    layer = fetch_manifest(repository, version)["layers"][0]["digest"]
    stored = json.loads(fetch_blob(repository, layer))
    assert {m["id"] for m in stored["members"]} == members


def test_grim_add_of_a_mirrored_bundle_installs_every_member(
    grim_mirror, grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, '["skills/*", "rules/style.md", "agents/*.md"]')
    bundle = _bundle(json.loads(_sync(grim_mirror, mf, "sync").stdout))

    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.run("add", bundle["ref"])

    installed = {r["name"] for r in runner.json("status")["items"]}
    assert {"alpha", "beta", "gamma", "style", "reviewer"} <= installed
    assert (project_dir / ".claude" / "skills" / "alpha" / "SKILL.md").is_file()
    assert (project_dir / ".claude" / "skills" / "gamma" / "SKILL.md").is_file()
    assert "Follow the fixture rule." in (project_dir / ".claude" / "rules" / "style.md").read_text(encoding="utf-8")
    assert "You are reviewer." in (project_dir / ".claude" / "agents" / "reviewer.md").read_text(encoding="utf-8")


def test_bundle_without_a_matching_member_is_refused_empty_bundle(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, '["skills/nothing-*"]')

    report = json.loads(_sync(grim_mirror, mf, "check", check=False).stdout)
    row = _bundle(report)
    assert (row["status"], row["reason"]) == ("refused", "empty-bundle")

    r = _sync(grim_mirror, mf, "--fail-on", "refused", "sync", check=False)
    assert r.returncode == 65, r.stderr
    # A standing refusal alone never turns a default sync red.
    assert _sync(grim_mirror, mf, "sync", check=False).returncode == 0


def test_bundle_of_only_refused_members_is_refused_empty_bundle(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = monorepo(tmp_path / "up", skills=(), rules=(), agents=(), license_text=None)
    repo.record("one skill without a licence", {"skills/solo/SKILL.md": skill_md("solo")})
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, '["skills/*"]')

    report = json.loads(_sync(grim_mirror, mf, "sync").stdout)

    solo = next(i for i in report["items"] if i["name"] == "solo")
    assert (solo["status"], solo["reason"]) == ("refused", "license-missing")
    row = _bundle(report)
    assert (row["status"], row["reason"]) == ("refused", "empty-bundle")
    assert solo["bundle"] == "kit"


def test_partial_bundle_publishes_admitted_members_and_reports_skipped_ones(
    grim_mirror, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    repo = monorepo(tmp_path / "up", skills=("alpha", "beta"), rules=(), agents=(), license_text=MIT_TEXT)
    repo.record("a skill directory without SKILL.md", {"skills/broken/README.md": "not a skill\n"})
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url, '["skills/*"]')

    report = json.loads(_sync(grim_mirror, mf, "sync").stdout)

    broken = next(i for i in report["items"] if i["name"] == "broken")
    assert broken["status"] == "refused"
    bundle = _bundle(report)
    assert bundle["status"] == "published", bundle
    refs = {i["ref"] for i in report["items"] if i["name"] in ("alpha", "beta")}
    assert set(bundle["members"]) == refs
    assert not any("broken" in m for m in bundle["members"])
    assert broken["bundle"] == "kit"
