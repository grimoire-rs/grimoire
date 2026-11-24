# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""S-013: retiring publishes a deprecated successor version and leaves the old exact tag installable."""
from __future__ import annotations

import json
from pathlib import Path

from src.git_fixture import BASE_DATE, GitRepo, skill_md
from src.helpers import write_config

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")
DEPRECATED = "com.grimoire.deprecated"
REPLACED_BY = "com.grimoire.replaced-by"
REASON = "moved upstream"
SUCCESSOR = "ghcr.io/acme/pdf:1"


def _write(tmp_path: Path, registry: str, unique_repo: str, repo: GitRepo, retire: str = "") -> Path:
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo.url}"\nskills = ["skills/*"]\n{retire}',
        encoding="utf-8",
        newline="\n",
    )
    return mf


def _sync(grim_mirror, mf: Path) -> dict:
    return json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)


def _two_skill_upstream(path: Path) -> GitRepo:
    repo = GitRepo.init(path)
    files = {"skills/alpha/SKILL.md": skill_md("alpha"), "skills/beta/SKILL.md": skill_md("beta"), "LICENSE": MIT_TEXT}
    repo.record("initial", files, date=BASE_DATE)
    return repo


def _retire_table(units: str = "[]") -> str:
    return f'retire = {{ reason = "{REASON}", replaced-by = "{SUCCESSOR}", units = {units} }}\n'


def test_retire_publishes_the_next_patch_version_deprecated_with_replacement(grim_mirror, grim_at, tmp_path, registry, unique_repo) -> None:
    repo = _two_skill_upstream(tmp_path / "up")
    first = _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo))
    old = next(i for i in first["items"] if i["name"] == "alpha")["version"]
    assert old == "0.0.20260101000000"

    report = _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo, _retire_table()))

    item = next(i for i in report["items"] if i["name"] == "alpha")
    assert (item["status"], item["version"]) == ("published", "0.0.20260101000001"), item
    ann = grim_at(tmp_path).json("describe", item["ref"])["annotations"]
    assert REASON in ann[DEPRECATED]
    assert ann[REPLACED_BY] == SUCCESSOR


def test_unit_deleted_upstream_is_retired_through_retire_units(grim_mirror, grim_at, tmp_path, registry, unique_repo) -> None:
    repo = _two_skill_upstream(tmp_path / "up")
    _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo))
    repo.remove("skills/beta")
    repo.record("drop beta")

    report = _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo, _retire_table('["skills/beta"]')))

    item = next(i for i in report["items"] if i["name"] == "beta")
    assert item["status"] == "published", item
    assert REASON in grim_at(tmp_path).json("describe", item["ref"])["annotations"][DEPRECATED]


def test_status_check_shows_the_deprecation_and_the_old_exact_tag_still_installs(
    grim_mirror, grim_at, project_dir, tmp_path, registry, unique_repo
) -> None:
    repo = _two_skill_upstream(tmp_path / "up")
    first = _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo))
    old_ref = next(i for i in first["items"] if i["name"] == "alpha")["ref"]
    _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo, _retire_table()))

    write_config(project_dir, skills={"alpha": old_ref})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    assert (project_dir / ".claude" / "skills" / "alpha" / "SKILL.md").exists()

    status = runner.json("--registry", f"{registry}/{unique_repo}", "status", "--check")
    row = next(i for i in status["items"] if i["name"] == "alpha")
    assert row["deprecated"] and REASON in row["deprecated"], row


def test_second_retire_run_reports_unchanged(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = _two_skill_upstream(tmp_path / "up")
    mf = _write(tmp_path, registry, unique_repo, repo, _retire_table())
    _sync(grim_mirror, _write(tmp_path, registry, unique_repo, repo))
    _sync(grim_mirror, mf)

    report = _sync(grim_mirror, mf)

    assert {i["status"] for i in report["items"]} == {"unchanged"}
