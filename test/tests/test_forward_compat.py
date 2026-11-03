# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Forward compatibility: formats this grim does not know are refused, not guessed.

A manifest may require features this grim lacks, or carry a layer layout it
cannot read. Every path that resolves or materializes content (`lock`,
`install`, `fetch`) exits 65 `unsupported-artifact-feature` and says to update
grim. `describe` only reads metadata, so it keeps working and shows what the
artifact is.
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact, make_dependent, tar_of, write_config
from src.registry import LAYER_MEDIA_TYPE, push_manifest, tag_digest

FEATURES_KEY = "com.grimoire.requires-features"
REQUIRES_TYPE = "application/vnd.grimoire.bundle.v1+json"
SKILL_DOC = "---\nname: compat-demo\ndescription: d\n---\n# Compat\n"


def _skill_files(name: str = "compat-demo") -> dict[str, str | bytes]:
    return {f"{name}/SKILL.md": SKILL_DOC.replace("compat-demo", name)}


def _error_of(result) -> dict:
    return json.loads(result.stdout)["error"]


def _assert_update_grim(error: dict) -> None:
    assert error["exit"] == 65, error
    assert error["reason"] == "unsupported-artifact-feature", error
    assert "update grim" in error["message"], error


def _locked(grim_at, project_dir: Path, skills: dict[str, str]):
    write_config(project_dir, skills=skills)
    runner = grim_at(project_dir)
    runner.run("lock")
    return runner


def _repin(project_dir: Path, stand_in: str, digest: str) -> None:
    """Point the lock at ``digest``, as a lock written before the tag moved.

    `lock` itself refuses the unreadable manifest, so only a lock pinned
    earlier (or by an older grim) can carry it to `install`.
    """
    lock = project_dir / "grimoire.lock"
    text = lock.read_text()
    assert stand_in in text, text
    lock.write_text(text.replace(stand_in, digest))


def _assert_lock_refuses(runner, needle: str) -> None:
    result = runner.run("--format", "json", "lock", check=False)
    assert result.returncode == 65, result.stderr
    error = _error_of(result)
    _assert_update_grim(error)
    assert needle in error["message"], error


def test_install_refuses_unknown_required_feature(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    good = make_artifact(f"{unique_repo}/good", "skill", _skill_files("good"), tag="1")
    stand_in = make_artifact(f"{unique_repo}/future", "skill", _skill_files("future"), tag="1")
    runner = _locked(grim_at, project_dir, {"good": good.fq, "future": stand_in.fq})
    future = make_artifact(
        f"{unique_repo}/future",
        "skill",
        _skill_files("future"),
        tag="1",
        annotations={FEATURES_KEY: "future-thing"},
    )
    _repin(project_dir, stand_in.digest, future.digest)

    result = runner.run("--format", "json", "install", check=False)

    assert result.returncode == 65, result.stderr
    error = _error_of(result)
    _assert_update_grim(error)
    assert "future-thing" in error["message"]
    assert not (project_dir / ".claude/skills/future").exists()
    # Positive control: the sibling without the annotation still installed.
    assert (project_dir / ".claude/skills/good/SKILL.md").is_file()
    _assert_lock_refuses(runner, "future-thing")


def test_fetch_refuses_unknown_required_feature(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    good = make_artifact(f"{unique_repo}/good", "skill", _skill_files("good"), tag="1")
    future = make_artifact(
        f"{unique_repo}/future",
        "skill",
        _skill_files("future"),
        tag="1",
        annotations={FEATURES_KEY: "future-thing"},
    )
    runner = grim_at(project_dir)

    assert runner.run("fetch", good.fq).stdout.startswith("---")
    result = runner.run("--format", "json", "fetch", future.fq, check=False)

    assert result.returncode == 65, result.stderr
    error = _error_of(result)
    _assert_update_grim(error)
    assert "future-thing" in error["message"]


def test_known_feature_is_accepted(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    known = make_artifact(
        f"{unique_repo}/known",
        "skill",
        _skill_files("known"),
        tag="1",
        annotations={FEATURES_KEY: "dependencies"},
    )
    runner = grim_at(project_dir)
    assert runner.run("fetch", known.fq).stdout.startswith("---")


def _three_layer(repo: str) -> str:
    push_manifest(
        repo,
        "1",
        [
            (LAYER_MEDIA_TYPE, tar_of(_skill_files("three"))),
            (REQUIRES_TYPE, b'{"members": []}\n'),
            (REQUIRES_TYPE, b'{"members": []}\n'),
        ],
        {"com.grimoire.kind": "skill"},
    )
    return f"{repo}:1"


def test_three_layer_manifest_refused_at_install_and_fetch(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    stand_in = make_artifact(f"{unique_repo}/three", "skill", _skill_files("three"), tag="1")
    runner = _locked(grim_at, project_dir, {"three": stand_in.fq})
    ref = f"{registry}/{_three_layer(f'{unique_repo}/three')}"
    _repin(project_dir, stand_in.digest, tag_digest(f"{unique_repo}/three", "1"))

    installed = runner.run("--format", "json", "install", check=False)
    fetched = runner.run("--format", "json", "fetch", ref, check=False)

    for result in (installed, fetched):
        assert result.returncode == 65, result.stderr
        error = _error_of(result)
        _assert_update_grim(error)
        assert "3 layers" in error["message"], error
        assert REQUIRES_TYPE in error["message"], error
    assert not (project_dir / ".claude/skills/three").exists()
    _assert_lock_refuses(runner, "3 layers")


def test_describe_ignores_unknown_feature(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    future = make_artifact(
        f"{unique_repo}/future",
        "skill",
        _skill_files("future"),
        tag="1",
        annotations={
            FEATURES_KEY: "future-thing",
            "org.opencontainers.image.description": "still described",
        },
    )
    three = f"{registry}/{_three_layer(f'{unique_repo}/three')}"
    runner = grim_at(project_dir)

    doc = runner.json("describe", future.fq)
    assert doc["kind"] == "skill"
    assert doc["description"] == "still described"
    assert doc["annotations"][FEATURES_KEY] == "future-thing"

    doc = runner.json("describe", three)
    assert doc["kind"] == "skill"
    assert doc["compression"] is None, "no readable content layer in this layout"


def test_fetch_of_a_dependent_artifact_returns_the_content_layer(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    dependent = make_dependent(
        f"{unique_repo}/dep",
        "1",
        "skill",
        _skill_files("dep"),
        [("skill", "base", "./base:1")],
    )
    runner = grim_at(project_dir)

    doc = runner.json("fetch", dependent.fq)

    assert doc["kind"] == "skill"
    assert doc["content"] == SKILL_DOC.replace("compat-demo", "dep")
    assert [f["path"] for f in doc["files"]] == ["dep/SKILL.md"]
