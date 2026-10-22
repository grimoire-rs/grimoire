# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Git index transport: a hostile locator must not widen what git may do.

An index locator is configuration, but a cloned repository carries its own
``grimoire.toml``, so the clone argument is untrusted input. The ``ext::``
test plants a marker-writing command behind an inherited allow-list and
asserts the marker never appears; the others pin the local-clone, relative
path and credential rules.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from src.index_fixture import index_config, package, search_envelope

posix_only = pytest.mark.skipif(
    sys.platform == "win32", reason="the planted command is a POSIX `touch`"
)


def _git_index_repo(tmp_path: Path) -> Path:
    """A local repository whose path ends in ``.git`` so it classifies as a
    git index locator, holding one package pointer."""
    repo = tmp_path / "index-repo.git"
    entry = repo / "index" / "github.com" / "acme" / "tool"
    entry.mkdir(parents=True)
    (entry / "metadata.json").write_text(
        json.dumps(package("tool", "skill", "ghcr.io/acme/skills/tool", "A tool"))
    )
    for args in (
        ["init", "-q", str(repo)],
        ["-C", str(repo), "add", "-A"],
        [
            "-C",
            str(repo),
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "seed",
        ],
    ):
        subprocess.run(["git", *args], check=True, capture_output=True)
    return repo


def _only_source(envelope: dict) -> dict:
    assert len(envelope["sources"]) == 1, envelope["sources"]
    return envelope["sources"][0]


@posix_only
def test_an_inherited_allow_list_does_not_reenable_ext(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    marker = tmp_path / "pwned"
    index_config(project_dir, f"git+ext::sh -c touch% {marker.as_posix()}")
    runner = grim_at(project_dir)
    runner.env["GIT_ALLOW_PROTOCOL"] = "ext"

    search_envelope(runner)

    assert not marker.exists(), "an inherited GIT_ALLOW_PROTOCOL=ext re-enabled ext::"


def test_a_local_path_index_clones_without_hardlinks(
    grim_at, grim_home: Path, project_dir: Path, tmp_path: Path
) -> None:
    repo = _git_index_repo(tmp_path)
    index_config(project_dir, repo.as_posix())

    envelope = search_envelope(grim_at(project_dir))

    assert [r["repo"] for r in envelope["items"]] == ["ghcr.io/acme/skills/tool"]
    objects = [
        p
        for clone in (grim_home / "index-git").iterdir()
        for p in (clone / ".git" / "objects").rglob("*")
        if p.is_file()
    ]
    assert objects, "the clone holds no objects"
    shared = [p.name for p in objects if p.stat().st_nlink > 1]
    assert not shared, f"a local clone must copy objects, not link them: {shared}"


def test_a_relative_local_path_index_still_clones(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    repo = _git_index_repo(tmp_path)
    index_config(project_dir, f"../{repo.name}")

    envelope = search_envelope(grim_at(project_dir))

    assert [r["repo"] for r in envelope["items"]] == ["ghcr.io/acme/skills/tool"]


def test_a_failed_clone_error_carries_no_url_credential(
    grim_at, project_dir: Path
) -> None:
    secret = "s3cret-token"
    index_config(project_dir, f"git+http://user:{secret}@127.0.0.1:9/index.git")

    source = _only_source(search_envelope(grim_at(project_dir)))

    assert source["ok"] is False
    assert secret not in (source["error"] or ""), source["error"]
