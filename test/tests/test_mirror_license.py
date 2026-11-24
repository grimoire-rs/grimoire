# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""S-010: the mirror decides a unit's licence from evidence, or refuses it."""
from __future__ import annotations

import json
from pathlib import Path

from src.git_fixture import BASE_DATE, GitRepo, skill_md

LICENSES = Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses"
MIT_TEXT = (LICENSES / "MIT.txt").read_text(encoding="utf-8")


def _repo(path: Path, files: dict[str, str]) -> GitRepo:
    repo = GitRepo.init(path)
    repo.record("initial", files, date=BASE_DATE)
    return repo


def _sync(grim_mirror, tmp_path: Path, registry: str, unique_repo: str, repo: GitRepo, upstream_extra: str = "", mirror_extra: str = "") -> dict:
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n{mirror_extra}'
        f'[[upstream]]\nname = "up"\nrepo = "{repo.url}"\nskills = ["skills/*"]\n{upstream_extra}',
        encoding="utf-8",
        newline="\n",
    )
    return json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)


def _alpha(report: dict) -> dict:
    return next(i for i in report["items"] if i["name"] == "alpha")


def test_detected_mit_file_publishes_as_detected(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": MIT_TEXT})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo))
    assert item["status"] == "published", item
    assert item["license"] == {"expression": "MIT", "source": "detected"}


def test_declared_license_is_used_when_nothing_is_detected(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha")})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo, 'license = "MIT"\n'))
    assert item["status"] == "published", item
    assert item["license"] == {"expression": "MIT", "source": "declared"}


def test_no_evidence_and_no_declaration_is_refused_license_missing(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha")})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo))
    assert (item["status"], item["reason"]) == ("refused", "license-missing")


def test_unrecognised_license_file_is_refused(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": "All rights reserved by nobody in particular.\n"})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo))
    assert (item["status"], item["reason"]) == ("refused", "license-unrecognised")


def test_unit_licenses_declares_a_unit_whose_file_is_unrecognised(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": "Custom terms, see the website.\n"})
    extra = '[upstream.unit-licenses]\n"skills/alpha" = "MIT"\n'
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo, extra))
    assert item["status"] == "published", item
    assert item["license"] == {"expression": "MIT", "source": "declared"}


def test_entry_license_does_not_override_an_unrecognised_file(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": "Custom terms, see the website.\n"})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo, 'license = "MIT"\n'))
    assert (item["status"], item["reason"]) == ("refused", "license-unrecognised")


def test_mit_with_a_non_commercial_clause_is_never_published_as_mit(grim_mirror, tmp_path, registry, unique_repo) -> None:
    text = MIT_TEXT + "\nThe Software may not be used for commercial purposes.\n"
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": text})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo))
    assert item["status"] == "refused", item
    assert item["reason"] == "license-unrecognised"


def test_license_outside_the_allowlist_is_refused_not_allowed(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": MIT_TEXT})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo, mirror_extra='licenses = ["Apache-2.0"]\n'))
    assert (item["status"], item["reason"]) == ("refused", "license-not-allowed")


def test_sources_at_one_level_naming_different_licenses_conflict(grim_mirror, tmp_path, registry, unique_repo) -> None:
    skill = "---\nname: alpha\ndescription: A fixture skill.\nlicense: Apache-2.0\n---\n\n# alpha\n"
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill, "skills/alpha/LICENSE": MIT_TEXT})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo))
    assert (item["status"], item["reason"]) == ("refused", "license-conflict")


def test_declaration_beside_detection_warns_and_detection_wins(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _repo(tmp_path / "up", {"skills/alpha/SKILL.md": skill_md("alpha"), "LICENSE": MIT_TEXT})
    item = _alpha(_sync(grim_mirror, tmp_path, registry, unique_repo, repo, 'license = "Apache-2.0"\n'))
    assert item["status"] == "published", item
    assert item["license"] == {"expression": "MIT", "source": "detected"}
    assert "declaration-ignored" in {w["code"] for w in item["warnings"]}
