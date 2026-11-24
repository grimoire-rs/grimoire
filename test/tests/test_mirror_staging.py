# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""S-015: units the mirror cannot carry faithfully are refused with a reason, the rest keep their bytes."""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from src.git_fixture import BASE_DATE, GitRepo, skill_md
from src.helpers import write_config
from src.registry import fetch_manifest

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")
LFS_POINTER = "version https://git-lfs.github.com/spec/v1\noid sha256:" + "0" * 64 + "\nsize 12345\n"


def _sync(grim_mirror, tmp_path: Path, registry: str, unique_repo: str, repo: GitRepo) -> dict:
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo.url}"\nskills = ["skills/*"]\n',
        encoding="utf-8",
        newline="\n",
    )
    return json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)


def _item(report: dict, name: str) -> dict:
    return next(i for i in report["items"] if i["name"] == name)


def test_template_only_skill_is_refused_unsupported_build_step(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = GitRepo.init(tmp_path / "up")
    repo.record("tmpl", {"skills/tmpl/SKILL.src.md": "# {{ name }}\n", "LICENSE": MIT_TEXT}, date=BASE_DATE)
    item = _item(_sync(grim_mirror, tmp_path, registry, unique_repo, repo), "tmpl")
    assert (item["status"], item["reason"]) == ("refused", "unsupported-build-step")


def test_lfs_pointer_blob_is_refused_unsupported_build_step(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = GitRepo.init(tmp_path / "up")
    files = {"skills/lfs/SKILL.md": skill_md("lfs"), "skills/lfs/big.bin": LFS_POINTER, "LICENSE": MIT_TEXT}
    repo.record("lfs", files, date=BASE_DATE)
    item = _item(_sync(grim_mirror, tmp_path, registry, unique_repo, repo), "lfs")
    assert (item["status"], item["reason"]) == ("refused", "unsupported-build-step")


@pytest.mark.skipif(sys.platform == "win32", reason="symlinks need POSIX")
def test_symlink_entry_is_refused_unsupported_entry(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = GitRepo.init(tmp_path / "up")
    repo.write("skills/link/SKILL.md", skill_md("link"))
    repo.write("LICENSE", MIT_TEXT)
    os.symlink("SKILL.md", repo.path / "skills" / "link" / "alias.md")
    repo.record("symlink", date=BASE_DATE)
    item = _item(_sync(grim_mirror, tmp_path, registry, unique_repo, repo), "link")
    assert (item["status"], item["reason"]) == ("refused", "unsupported-entry")


@pytest.mark.skipif(sys.platform == "win32", reason="backslash is a path separator there")
def test_unit_path_with_a_backslash_is_refused_unsafe_path(grim_mirror, tmp_path, registry, unique_repo) -> None:
    repo = GitRepo.init(tmp_path / "up")
    repo.record("odd", {"skills/back\\slash/SKILL.md": skill_md("slash"), "LICENSE": MIT_TEXT}, date=BASE_DATE)
    report = _sync(grim_mirror, tmp_path, registry, unique_repo, repo)
    refused = [i for i in report["items"] if i["reason"] == "unsafe-path"]
    assert len(refused) == 1 and refused[0]["status"] == "refused", report["items"]


def test_plugin_manifest_at_a_skill_root_is_dropped_with_a_warning(grim_mirror, grim_at, project_dir, tmp_path, registry, unique_repo) -> None:
    repo = GitRepo.init(tmp_path / "up")
    files = {
        "skills/manifested/SKILL.md": skill_md("manifested"),
        "skills/manifested/.claude-plugin/plugin.json": json.dumps({"name": "manifested"}),
        "LICENSE": MIT_TEXT,
    }
    repo.record("manifest", files, date=BASE_DATE)

    item = _item(_sync(grim_mirror, tmp_path, registry, unique_repo, repo), "manifested")

    assert item["status"] == "published", item
    assert "plugin-manifest-dropped" in {w["code"] for w in item["warnings"]}
    write_config(project_dir)
    grim_at(project_dir).run("add", item["ref"])
    assert not (project_dir / ".claude" / "skills" / "manifested" / ".claude-plugin").exists()


@pytest.mark.skipif(sys.platform == "win32", reason="exec bit is POSIX")
def test_mirrored_layer_equals_a_native_publish_of_the_same_tree(grim_mirror, grim_at, project_dir, tmp_path, registry, unique_repo) -> None:
    repo = GitRepo.init(tmp_path / "up")
    script = repo.write("skills/runner/scripts/run.sh", "#!/bin/sh\necho hi\n")
    script.chmod(0o755)
    repo.write("skills/runner/SKILL.md", skill_md("runner"))
    repo.write("LICENSE", MIT_TEXT)
    repo.record("exec", date=BASE_DATE)

    item = _item(_sync(grim_mirror, tmp_path, registry, unique_repo, repo), "runner")
    assert item["status"] == "published", item

    # the mirror stages the repo-root licence into the unit; the native tree gets the same file
    (tmp_path / "up" / "skills" / "runner" / "LICENSE").write_text(MIT_TEXT, encoding="utf-8", newline="\n")
    native_repo = f"{unique_repo}-native"
    grim_at(project_dir).run("release", str(tmp_path / "up" / "skills" / "runner"), f"{registry}/{native_repo}:1.0.0", "--kind", "skill")

    mirrored_repo, _, mirrored_tag = item["ref"].removeprefix(f"{registry}/").rpartition(":")
    layers = lambda r, t: [layer["digest"] for layer in fetch_manifest(r, t)["layers"]]  # noqa: E731
    assert layers(mirrored_repo, mirrored_tag) == layers(native_repo, "1.0.0")
