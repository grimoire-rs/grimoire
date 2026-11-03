# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-test of the dependency fixtures: ``push_manifest`` and ``make_dependent``.

Needs only the session registry, no grim binary.
"""

from __future__ import annotations

import json

from src.helpers import make_dependent, members_layer
from src.registry import (
    LAYER_MEDIA_TYPE,
    fetch_blob,
    fetch_manifest,
    push_manifest,
    tag_digest,
)

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
