# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""S-012 and S-001: grim-mirror's exit codes, check mode, and `grim mirror` dispatch."""
from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
from pathlib import Path

import pytest

from src.git_fixture import BASE_DATE, GitRepo, skill_md

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")


def _file(tmp_path: Path, registry: str, unique_repo: str, repo_url: str) -> Path:
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo_url}"\nskills = ["skills/*"]\n',
        encoding="utf-8",
        newline="\n",
    )
    return mf


def _upstream(tmp_path: Path, license_text: str | None = MIT_TEXT) -> GitRepo:
    repo = GitRepo.init(tmp_path / "up")
    files = {"skills/alpha/SKILL.md": skill_md("alpha")}
    if license_text is not None:
        files["LICENSE"] = license_text
    repo.record("initial", files, date=BASE_DATE)
    return repo


def test_check_reports_would_publish_and_leaves_the_registry_unchanged(grim_mirror, grim_at, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)

    r = grim_mirror.run("--file", str(mf), "--format", "json", "check")

    report = json.loads(r.stdout)
    (item,) = report["items"]
    assert item["status"] == "would-publish"
    assert grim_at(tmp_path).run("describe", item["ref"], check=False).returncode == 79


def test_offline_mode_exits_81(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)
    grim_mirror.env = {**grim_mirror.env, "GRIM_OFFLINE": "1"}
    assert grim_mirror.run("--file", str(mf), "check", check=False).returncode == 81


def test_missing_mirror_file_exits_79(grim_mirror, tmp_path) -> None:
    assert grim_mirror.run("--file", str(tmp_path / "absent.toml"), "check", check=False).returncode == 79


def test_invalid_mirror_file_exits_78(grim_mirror, tmp_path) -> None:
    mf = tmp_path / "mirror.toml"
    mf.write_text("schema = 2\n", encoding="utf-8", newline="\n")
    r = grim_mirror.run("--file", str(mf), "check", check=False)
    assert r.returncode == 78
    assert "mirror.toml" in r.stderr


@pytest.mark.skipif(sys.platform == "win32", reason="shell script stand-in")
def test_grim_without_the_mirror_capability_exits_69(grim_mirror, tmp_path, registry, unique_repo) -> None:
    fake = tmp_path / "fake-grim"
    fake.write_text(
        '#!/bin/sh\ncase "$*" in\n  *--version*) echo "grim 0.17.0";;\n  *) echo "{}";;\nesac\n',
        encoding="utf-8",
        newline="\n",
    )
    fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)
    grim_mirror.env = {**grim_mirror.env, "GRIM_BIN": str(fake)}

    assert grim_mirror.run("--file", str(mf), "check", check=False).returncode == 69


def test_sync_exits_zero_with_a_standing_refused_unit(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path, license_text=None).url)

    r = grim_mirror.run("--file", str(mf), "--fail-on", "infra", "--format", "json", "sync", check=False)

    assert r.returncode == 0, r.stderr
    assert json.loads(r.stdout)["summary"]["refused"] == 1


def test_check_exits_65_for_a_refused_unit_by_default(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path, license_text=None).url)
    assert grim_mirror.run("--file", str(mf), "check", check=False).returncode == 65


def test_sync_exits_non_zero_when_the_upstream_cannot_be_fetched(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, (tmp_path / "no-such-repo").resolve().as_uri())

    r = grim_mirror.run("--file", str(mf), "--fail-on", "infra", "--format", "json", "sync", check=False)

    assert r.returncode != 0
    assert json.loads(r.stdout)["upstreams"][0]["status"] == "failed"


def test_grim_mirror_check_runs_through_plugin_dispatch(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)

    r = grim_mirror.run_via_grim("--file", str(mf), "--format", "json", "check")

    assert json.loads(r.stdout)["items"][0]["status"] == "would-publish"


def test_grim_format_flag_before_mirror_is_refused_with_a_hint(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)
    env = {**grim_mirror.env, "PATH": f"{grim_mirror.mirror.parent}{os.pathsep}{grim_mirror.env['PATH']}"}

    r = subprocess.run(
        [str(grim_mirror.grim), "--format", "json", "mirror", "--file", str(mf), "check"],
        capture_output=True, text=True, encoding="utf-8", env=env, stdin=subprocess.DEVNULL, check=False,
    )

    assert r.returncode == 64
    assert "--format" in r.stderr and "mirror" in r.stderr


def test_flags_after_the_subcommand_behave_like_flags_before_it(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path, license_text=None).url)

    before = grim_mirror.run("--file", str(mf), "--fail-on", "refused", "--format", "json", "check", check=False)
    after = grim_mirror.run("check", "--file", str(mf), "--fail-on", "refused", "--format", "json", check=False)

    assert before.returncode == 65, before.stderr
    assert after.returncode == before.returncode, after.stderr
    assert json.loads(after.stdout)["summary"] == json.loads(before.stdout)["summary"]


def test_sync_accepts_format_and_file_after_the_subcommand(grim_mirror, grim_at, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)

    r = grim_mirror.run("sync", "--format", "json", "--file", str(mf), check=False)

    assert r.returncode == 0, r.stderr
    (item,) = json.loads(r.stdout)["items"]
    assert item["status"] == "published"
    assert grim_at(tmp_path).json("describe", item["ref"])["kind"] == "skill"


def test_grim_mirror_dispatch_accepts_flags_after_the_subcommand(grim_mirror, tmp_path, registry, unique_repo) -> None:
    mf = _file(tmp_path, registry, unique_repo, _upstream(tmp_path).url)

    r = grim_mirror.run_via_grim("check", "--file", str(mf), "--format", "json", check=False)

    assert r.returncode == 0, r.stderr
    assert json.loads(r.stdout)["items"][0]["status"] == "would-publish"
