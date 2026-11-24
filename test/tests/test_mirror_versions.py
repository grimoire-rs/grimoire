# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""S-011: tag mode, snapshot mode, watermark and moved-tag handling."""
from __future__ import annotations

import json
import re
from pathlib import Path

from src.git_fixture import BASE_DATE, GitRepo, skill_md
from src.registry import list_tags

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")


def _upstream(path: Path) -> GitRepo:
    repo = GitRepo.init(path)
    repo.record("initial", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": MIT_TEXT}, date=BASE_DATE)
    return repo


def _bump(repo: GitRepo, n: int) -> str:
    return repo.record(f"change {n}", {"skills/alpha/SKILL.md": skill_md("alpha", f"Revision {n}.")})


def _mirror_file(tmp_path: Path, registry: str, unique_repo: str, repo: GitRepo, extra: str = "", mirror_extra: str = "") -> Path:
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n{mirror_extra}'
        f'[[upstream]]\nname = "up"\nrepo = "{repo.url}"\nskills = ["skills/*"]\n{extra}',
        encoding="utf-8",
        newline="\n",
    )
    return mf


def _sync(grim_mirror, mf: Path) -> dict:
    return json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)


def _versions(report: dict, status: str | None = None) -> list[str]:
    return sorted(i["version"] for i in report["items"] if i["version"] and (status is None or i["status"] == status))


def test_each_new_tag_publishes_its_version_up_to_the_backfill(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _upstream(tmp_path / "up")
    repo.tag("v1.0.0")
    _bump(repo, 1)
    repo.tag("v1.1.0")
    _bump(repo, 2)
    repo.tag("v1.2.0")
    mf = _mirror_file(tmp_path, registry, unique_repo, repo, "backfill = 2\n")

    report = _sync(grim_mirror, mf)

    assert _versions(report, "published") == ["1.1.0", "1.2.0"]
    published = list_tags(f"{unique_repo}/up/skills/alpha")
    assert "1.1.0" in published and "1.2.0" in published and "1.0.0" not in published


def test_untagged_upstream_publishes_a_zero_zero_timestamp_snapshot(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _upstream(tmp_path / "up")
    report = _sync(grim_mirror, _mirror_file(tmp_path, registry, unique_repo, repo))
    (version,) = _versions(report, "published")
    assert re.fullmatch(r"0\.0\.\d{14}", version), version
    assert version == "0.0.20260101000000"


def test_forced_snapshot_over_a_tag_keeps_the_tag_base(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _upstream(tmp_path / "up")
    repo.tag("v1.2.0")
    mf = _mirror_file(tmp_path, registry, unique_repo, repo, 'version = "snapshot"\n')
    (version,) = _versions(_sync(grim_mirror, mf), "published")
    assert version == "1.2.20260101000000"


def test_first_release_tag_still_publishes_after_a_snapshot_phase(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _upstream(tmp_path / "up")
    mf = _mirror_file(tmp_path, registry, unique_repo, repo)
    first = _sync(grim_mirror, mf)
    assert _versions(first, "published") == ["0.0.20260101000000"]
    _bump(repo, 1)
    repo.tag("v0.0.1")

    second = _sync(grim_mirror, mf)

    assert "0.0.1" in _versions(second, "published")


def test_a_tag_moved_between_syncs_is_refused_upstream_moved(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _upstream(tmp_path / "up")
    repo.tag("v1.0.0")
    mf = _mirror_file(tmp_path, registry, unique_repo, repo)
    assert _versions(_sync(grim_mirror, mf), "published") == ["1.0.0"]
    _bump(repo, 1)
    repo.git("tag", "-f", "v1.0.0", "HEAD")

    report = _sync(grim_mirror, mf)

    (item,) = [i for i in report["items"] if i["reason"] == "upstream-moved"]
    assert item["status"] == "refused"
    assert "1.0.0" in item["message"]


def test_prerelease_tags_are_ignored_with_one_warning(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _upstream(tmp_path / "up")
    repo.tag("v1.0.0")
    _bump(repo, 1)
    repo.tag("v1.1.0-rc.1")
    pattern = "tag-pattern = '^v?(\\d+\\.\\d+\\.\\d+(?:-rc\\.\\d+)?)$'\n"
    mf = _mirror_file(tmp_path, registry, unique_repo, repo, pattern + "backfill = 5\n")

    report = _sync(grim_mirror, mf)

    assert _versions(report, "published") == ["1.0.0"]
    warnings = [w for w in report["upstreams"][0]["warnings"] if w["code"] == "tags-ignored"]
    assert len(warnings) == 1
    assert "v1.1.0-rc.1" in warnings[0]["message"]
