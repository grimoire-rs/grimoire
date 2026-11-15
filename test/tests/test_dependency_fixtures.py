# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-test of the dependency fixtures: ``push_manifest``, ``make_dependent``
and the version-claim publish helpers.

The dependency acceptance scenarios trust these helpers to tag the registry the
way ``grim release`` does, so each tag a helper writes is read back here. Only
``test_release_with_grim_reports_tags`` needs the grim binary.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path

import pytest

from src.helpers import (
    VERSION_ANNOTATION,
    backport,
    cascade_tags,
    make_artifact,
    make_dependent,
    members_layer,
    publish_lie,
    publish_release,
    release_with_grim,
)
from src.registry import (
    LAYER_MEDIA_TYPE,
    fetch_blob,
    fetch_manifest,
    list_tags,
    push_manifest,
    tag_digest,
)
from src.runner import GrimRunner

BUNDLE_TYPE = "application/vnd.grimoire.bundle.v1+json"


def test_push_manifest_two_and_three_layers(registry: str, unique_repo: str) -> None:
    for tag, layers in {
        "two": [(LAYER_MEDIA_TYPE, b"one"), (BUNDLE_TYPE, b"two")],
        "three": [(LAYER_MEDIA_TYPE, b"a"), (BUNDLE_TYPE, b"b"), (BUNDLE_TYPE, b"c")],
    }.items():
        digest = push_manifest(unique_repo, tag, layers, {"k": "v"})
        assert digest == tag_digest(unique_repo, tag)
        manifest = fetch_manifest(unique_repo, tag)
        assert manifest["annotations"] == {"k": "v"}
        assert "artifactType" not in manifest
        assert [
            (layer["mediaType"], layer["size"]) for layer in manifest["layers"]
        ] == [(media_type, len(blob)) for media_type, blob in layers]
        assert [
            fetch_blob(unique_repo, layer["digest"]) for layer in manifest["layers"]
        ] == [blob for _, blob in layers]


def test_make_dependent_writes_canonical_requires_layer(
    registry: str, unique_repo: str
) -> None:
    requires = [
        ("skill", "zeta", "reg.example/z:1"),
        ("mcp", "gdrive", "reg.example/gdrive:2"),
        ("skill", "alpha", "./alpha:1"),
    ]
    published = make_dependent(
        unique_repo, "v1", "skill", {"s/SKILL.md": "# s\n"}, requires
    )
    assert published.digest == tag_digest(unique_repo, "v1")

    manifest = fetch_manifest(unique_repo, "v1")
    assert manifest["annotations"] == {"com.grimoire.kind": "skill"}
    content, req = manifest["layers"]
    assert content["mediaType"] == LAYER_MEDIA_TYPE
    assert req["mediaType"] == BUNDLE_TYPE
    assert "annotations" not in req

    body = fetch_blob(unique_repo, req["digest"])
    assert body == members_layer(requires)
    assert body.endswith(b"}\n")
    # Sorted by (kind in ArtifactKind order, name): skill before mcp, unlike a
    # text sort. Field order kind/name/id; 2-space pretty JSON.
    assert [(m["kind"], m["name"]) for m in json.loads(body)["members"]] == [
        ("skill", "alpha"),
        ("skill", "zeta"),
        ("mcp", "gdrive"),
    ]
    assert body.startswith(
        b'{\n  "members": [\n    {\n      "kind": "skill",\n      "name": "alpha",\n      "id": '
    )


def _skill(name: str = "code-review") -> dict[str, str | bytes]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: Review code.\n---\n# {name}\n"
    }


def _digests(repo: str, tags: list[str]) -> dict[str, str]:
    return {tag: tag_digest(repo, tag) for tag in tags}


@pytest.mark.parametrize(
    ("version", "tags"),
    [
        ("1.2.3", ["1.2.3", "1.2", "1", "latest"]),
        ("0.1.0", ["0.1.0", "0.1", "0", "latest"]),
        ("10.20.30", ["10.20.30", "10.20", "10", "latest"]),
        ("1.2.3+build.7", ["1.2.3", "1.2", "1", "latest"]),
        ("1.2.3-rc.1", ["1.2.3-rc.1"]),
        ("2.0.0-alpha", ["2.0.0-alpha"]),
    ],
)
def test_cascade_tags_matches_publish_tags(version: str, tags: list[str]) -> None:
    """The rows of `publish_tags`' default-cascade unit tests in `src/oci/release.rs`."""
    assert cascade_tags(version) == tags


@pytest.mark.parametrize("version", ["canary", "1.2", "1", "01.2.3", ""])
def test_cascade_tags_refuses_what_is_not_full_semver(version: str) -> None:
    with pytest.raises(ValueError, match="not a full semver version"):
        cascade_tags(version)


def test_publish_release_stamps_the_version_and_cascades(unique_repo: str) -> None:
    repo = f"{unique_repo}/code-review"
    first = publish_release(repo, "2.3.0", _skill())

    assert first.tag == "2.3.0"
    manifest = fetch_manifest(repo, "2.3.0")
    assert manifest["annotations"] == {
        "com.grimoire.kind": "skill",
        VERSION_ANNOTATION: "2.3.0",
    }
    assert len(manifest["layers"]) == 1
    assert list_tags(repo) == sorted(["2.3.0", "2.3", "2", "latest"])
    assert set(_digests(repo, list_tags(repo)).values()) == {first.digest}

    second = publish_release(repo, "2.4.0", _skill())
    assert second.digest != first.digest, (
        "the annotation must give each version its own digest"
    )
    assert _digests(repo, ["2.3.0", "2.3", "2.4.0", "2.4", "2", "latest"]) == {
        "2.3.0": first.digest,
        "2.3": first.digest,
        "2.4.0": second.digest,
        "2.4": second.digest,
        "2": second.digest,
        "latest": second.digest,
    }
    assert list_tags(repo) == sorted(["2.3.0", "2.3", "2.4.0", "2.4", "2", "latest"])


def test_publish_release_moves_no_float_for_a_prerelease_or_without_cascade(
    unique_repo: str,
) -> None:
    repo = f"{unique_repo}/code-review"
    release = publish_release(repo, "1.0.0", _skill())

    pre = publish_release(repo, "2.0.0-rc.1", _skill())
    exact_only = publish_release(repo, "1.1.0", _skill(), cascade=False)

    assert (
        fetch_manifest(repo, "2.0.0-rc.1")["annotations"][VERSION_ANNOTATION]
        == "2.0.0-rc.1"
    )
    assert _digests(repo, ["2.0.0-rc.1", "1.1.0", "1", "latest"]) == {
        "2.0.0-rc.1": pre.digest,
        "1.1.0": exact_only.digest,
        "1": release.digest,
        "latest": release.digest,
    }
    assert list_tags(repo) == sorted(
        ["1.0.0", "1.0", "1", "latest", "2.0.0-rc.1", "1.1.0"]
    )


def test_publish_release_with_requires_adds_the_requires_layer(
    registry: str, unique_repo: str
) -> None:
    dep = publish_release(f"{unique_repo}/dep", "1.4.0", _skill("dep"))
    parent = publish_release(
        f"{unique_repo}/parent",
        "3.0.0",
        _skill("parent"),
        requires=[("skill", "dep", f"{registry}/{dep.repo}:1")],
    )

    manifest = fetch_manifest(parent.repo, "latest")
    assert manifest["layers"][1]["mediaType"] == BUNDLE_TYPE
    assert manifest["annotations"][VERSION_ANNOTATION] == "3.0.0"
    assert tag_digest(parent.repo, "3") == parent.digest


def test_backport_moves_only_the_major_and_latest(unique_repo: str) -> None:
    repo = f"{unique_repo}/code-review"
    older = publish_release(repo, "2.3.7", _skill())
    newer = publish_release(repo, "2.9.0", _skill())
    assert tag_digest(repo, "2") == newer.digest

    assert backport(repo, "2.3.7") == ["2", "latest"]

    assert _digests(repo, ["2.3.7", "2.3", "2", "latest", "2.9.0", "2.9"]) == {
        "2.3.7": older.digest,
        "2.3": older.digest,
        "2": older.digest,
        "latest": older.digest,
        "2.9.0": newer.digest,
        "2.9": newer.digest,
    }
    assert (
        fetch_manifest(repo, newer.digest)["annotations"][VERSION_ANNOTATION] == "2.9.0"
    )
    assert list_tags(repo) == sorted(["2.3.7", "2.3", "2", "latest", "2.9.0", "2.9"]), (
        "the newer release keeps its exact tag, so no keep tag is written"
    )


def test_a_digest_a_float_leaves_untagged_stays_fetchable(unique_repo: str) -> None:
    repo = f"{unique_repo}/code-review"
    stray = make_artifact(repo, "skill", _skill(), tag="latest")

    release = publish_release(repo, "1.0.0", _skill())

    keep = f"keep-{stray.digest.removeprefix('sha256:')[:12]}"
    assert tag_digest(repo, "latest") == release.digest
    assert tag_digest(repo, keep) == stray.digest
    assert fetch_manifest(repo, stray.digest)["layers"], (
        "the moved-off manifest must survive"
    )
    assert list_tags(repo) == sorted(["1.0.0", "1.0", "1", "latest", keep])


def test_publish_lie_annotation_contradicts_its_tag(unique_repo: str) -> None:
    repo = f"{unique_repo}/code-review"

    lie = publish_lie(repo, "2", "3.1.0", _skill())

    assert lie.tag == "2"
    assert tag_digest(repo, "2") == lie.digest
    assert fetch_manifest(repo, "2")["annotations"][VERSION_ANNOTATION] == "3.1.0"
    assert list_tags(repo) == ["2"]


def test_release_with_grim_reports_tags(
    grim_at: Callable[[Path], GrimRunner], project_dir: Path, unique_repo: str
) -> None:
    """Today's binary, releases in ascending order only: every float moves up."""
    repo = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    first = release_with_grim(runner, repo, "1.2.3", _skill())
    second = release_with_grim(runner, repo, "1.3.0", _skill())

    assert first["pushed"] is True
    assert first["tags"] == cascade_tags("1.2.3")
    assert second["tags"] == cascade_tags("1.3.0")
    assert first["manifest_digest"] != second["manifest_digest"]
    assert fetch_manifest(repo, "1.2.3")["annotations"][VERSION_ANNOTATION] == "1.2.3"
    assert _digests(repo, ["1.2.3", "1.2", "1.3.0", "1.3", "1", "latest"]) == {
        "1.2.3": first["manifest_digest"],
        "1.2": first["manifest_digest"],
        "1.3.0": second["manifest_digest"],
        "1.3": second["manifest_digest"],
        "1": second["manifest_digest"],
        "latest": second["manifest_digest"],
    }
    assert not list(project_dir.glob("**/SKILL.md")), (
        "the source tree stays out of the project"
    )
