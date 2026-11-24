# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Done 1: one repository holding several kinds publishes one artifact per unit."""
from __future__ import annotations

import json
from pathlib import Path

from src.git_fixture import monorepo
from src.registry import list_tags

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")


def _mirror_file(tmp_path: Path, registry: str, unique_repo: str, repo_url: str) -> Path:
    path = tmp_path / "mirror.toml"
    path.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo_url}"\n'
        'skills = ["skills/*"]\nrules = ["rules/*.md"]\nagents = ["agents/*.md"]\n',
        encoding="utf-8",
        newline="\n",
    )
    return path


def _report(result) -> dict:
    return json.loads(result.stdout)


def test_sync_publishes_one_artifact_per_unit(grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url)

    r = grim_mirror.run_via_grim("--file", str(mf), "--format", "json", "sync")
    report = _report(r)

    items = {(i["kind"], i["name"]): i for i in report["items"]}
    assert set(items) == {
        ("skill", "alpha"), ("skill", "beta"), ("skill", "gamma"),
        ("rule", "style"), ("rule", "naming"), ("agent", "reviewer"),
    }
    assert report["summary"]["published"] == 6
    commit = repo.head()
    kind_dir = {"skill": "skills", "rule": "rules", "agent": "agents"}
    for item in items.values():
        assert item["status"] == "published", item
        assert item["version"] and item["revision"] == commit
        assert item["license"] == {"expression": "MIT", "source": "detected"}
        # Target repository layout: {registry}/{upstream}/{kind-dir}/{name}.
        repository = item["ref"].rsplit(":", 1)[0]
        assert repository == f"{registry}/{unique_repo}/up/{kind_dir[item['kind']]}/{item['name']}"
        assert item["version"] in list_tags(repository.removeprefix(f"{registry}/"))


def test_published_artifact_carries_mirror_annotations(grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url)
    report = _report(grim_mirror.run_via_grim("--file", str(mf), "--format", "json", "sync"))
    item = next(i for i in report["items"] if i["name"] == "alpha")

    d = grim_at(tmp_path).json("describe", item["ref"])
    assert d["revision"] == repo.head()
    assert d["license"] == "MIT"
    ann = d["annotations"]
    assert ann["com.grimoire.mirror.source"] == repo.url
    assert ann["com.grimoire.mirror.path"] == "skills/alpha"
    assert ann["com.grimoire.mirror.license-source"] == "detected"


def test_second_sync_reports_everything_unchanged(grim_mirror, tmp_path: Path, registry: str, unique_repo: str) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url)
    first = _report(grim_mirror.run("--file", str(mf), "--format", "json", "sync"))
    second = grim_mirror.run("--file", str(mf), "--format", "json", "sync")

    assert second.returncode == 0
    report = _report(second)
    assert report["summary"]["unchanged"] == 6
    assert report["summary"]["published"] == 0
    assert {i["status"] for i in report["items"]} == {"unchanged"}
    assert {i["version"] for i in report["items"]} == {i["version"] for i in first["items"]}


def test_check_reports_would_publish_and_pushes_nothing(grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url)

    r = grim_mirror.run("--file", str(mf), "--format", "json", "check")

    assert r.returncode == 0
    report = _report(r)
    assert report["summary"]["would_publish"] == 6
    assert {i["status"] for i in report["items"]} == {"would-publish"}
    for item in report["items"]:
        d = grim_at(tmp_path).run("describe", item["ref"], check=False)
        assert d.returncode == 79, d.stderr


def test_published_skill_payload_equals_the_upstream_file(grim_mirror, grim_at, tmp_path: Path, registry: str, unique_repo: str) -> None:
    repo = monorepo(tmp_path / "up", license_text=MIT_TEXT)
    mf = _mirror_file(tmp_path, registry, unique_repo, repo.url)
    report = _report(grim_mirror.run("--file", str(mf), "--format", "json", "sync"))
    item = next(i for i in report["items"] if i["name"] == "alpha")

    fetched = grim_at(tmp_path).run("fetch", item["ref"]).stdout
    assert fetched == (repo.path / "skills/alpha/SKILL.md").read_text(encoding="utf-8")
