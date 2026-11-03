# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim release` publishes a declared `requires` as a second layer.

A skill, rule, agent, hook or mcp source that declares `requires` is pushed as
``[content, requires]``: the kind's own layer, then the bundle members document
(canonical bytes, no descriptor annotations, never compressed). A source that
declares nothing is pushed exactly as before.
"""
from __future__ import annotations

import json
import urllib.request
from pathlib import Path

import pytest

from src.helpers import make_artifact, members_layer
from src.registry import (
    LAYER_MEDIA_TYPE,
    REGISTRY_BASE,
    fetch_blob,
    fetch_manifest,
    tag_digest,
)

BUNDLE_TYPE = "application/vnd.grimoire.bundle.v1+json"
MCP_TYPE = "application/vnd.grimoire.mcp.v1+json"
GZIP_TYPE = "application/vnd.grimoire.artifact.layer.v1.tar+gzip"
BASELINE = (
    Path(__file__).resolve().parent.parent
    / "data/golden/pre_dependencies_cdeedcaa/registry/release_baseline.json"
)


def _write(p: Path, body: str) -> Path:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(body.encode())  # no CRLF translation on Windows: digests pin exact bytes
    return p


def _dependent_skill(root: Path, requires: dict[str, str], name: str = "gws-drive") -> Path:
    """A skill whose frontmatter declares ``grim.requires.<table>.<binding>`` keys."""
    meta = "".join(f'  grim.requires.{key}: "{ref}"\n' for key, ref in requires.items())
    _write(
        root / name / "SKILL.md",
        f"---\nname: {name}\ndescription: Drive helpers.\nmetadata:\n{meta}---\n# {name}\n",
    )
    return root / name


def _assert_tag_absent(repo_path: str, tag: str) -> None:
    from urllib.error import HTTPError

    with pytest.raises(HTTPError) as exc:
        tag_digest(repo_path, tag)
    assert exc.value.code == 404


def test_two_layer_release(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _dependent_skill(
        project_dir,
        {
            "skills.gws-shared": "./gws-shared:1",
            "mcp.gdrive": "ghcr.io/acme/mcp/gdrive:2",
        },
    )
    runner = grim_at(project_dir)
    repo_path = f"{unique_repo}/skill/gws-drive"
    reference = f"{registry}/{repo_path}"

    runner.json("release", str(skill), f"{reference}:1.0.0", "--no-cascade")

    manifest = fetch_manifest(repo_path, "1.0.0")
    assert manifest["annotations"]["com.grimoire.kind"] == "skill"
    content, requires = manifest["layers"]
    assert content["mediaType"] == LAYER_MEDIA_TYPE
    assert requires["mediaType"] == BUNDLE_TYPE
    assert "annotations" not in requires
    # Canonical bytes: (kind order, name), a relative id stays relative.
    expected = members_layer(
        [
            ("skill", "gws-shared", "./gws-shared:1"),
            ("mcp", "gdrive", "ghcr.io/acme/mcp/gdrive:2"),
        ]
    )
    assert fetch_blob(repo_path, requires["digest"]) == expected
    assert requires["size"] == len(expected)

    # Compression applies to the content layer only; the requires layer is
    # the same canonical document, never encoded.
    runner.json(
        "release", str(skill), f"{reference}:1.1.0", "--no-cascade", "--compression", "gzip"
    )
    gz_content, gz_requires = fetch_manifest(repo_path, "1.1.0")["layers"]
    assert gz_content["mediaType"] == GZIP_TYPE
    assert gz_requires["mediaType"] == BUNDLE_TYPE
    assert gz_requires["digest"] == requires["digest"]
    assert "annotations" not in gz_requires


def test_pin_freezes_requires(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    dep = make_artifact(
        f"{unique_repo}/skill/gws-shared",
        "skill",
        {"gws-shared/SKILL.md": "---\nname: gws-shared\ndescription: d\n---\n# s\n"},
        tag="1",
    )
    skill = _dependent_skill(project_dir, {"skills.gws-shared": "./gws-shared:1"})
    runner = grim_at(project_dir)
    repo_path = f"{unique_repo}/skill/gws-drive"
    reference = f"{registry}/{repo_path}"

    runner.json("release", str(skill), f"{reference}:1.0.0", "--pin", "--no-cascade")

    requires = fetch_manifest(repo_path, "1.0.0")["layers"][1]
    members = json.loads(fetch_blob(repo_path, requires["digest"]))["members"]
    assert members == [
        {
            "kind": "skill",
            "name": "gws-shared",
            "id": f"{registry}/{unique_repo}/skill/gws-shared:1@{dep.digest}",
        }
    ]
    # Positive control: without --pin the same source keeps the relative id.
    runner.json("release", str(skill), f"{reference}:1.0.1", "--no-cascade")
    unpinned = fetch_manifest(repo_path, "1.0.1")["layers"][1]
    assert json.loads(fetch_blob(repo_path, unpinned["digest"]))["members"][0]["id"] == (
        "./gws-shared:1"
    )


def test_rerelease_dependent_adopted(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _dependent_skill(project_dir, {"skills.gws-shared": "./gws-shared:1"})
    runner = grim_at(project_dir)
    repo_path = f"{unique_repo}/skill/gws-drive"
    reference = f"{registry}/{repo_path}:1.0.0"

    first = runner.json("release", str(skill), reference, "--no-cascade")
    again = runner.json("release", str(skill), reference, "--no-cascade")
    assert again["manifest_digest"] == first["manifest_digest"]

    # Same content under the other compression is adopted: the tag keeps its
    # manifest, and the release succeeds instead of refusing with TagExists.
    flipped = runner.json(
        "release", str(skill), reference, "--no-cascade", "--compression", "gzip"
    )
    assert flipped["manifest_digest"] == first["manifest_digest"]
    assert tag_digest(repo_path, "1.0.0") == first["manifest_digest"]

    # Positive control: a different requires layer is not the same content.
    changed = _dependent_skill(
        project_dir / "changed", {"skills.gws-shared": "./gws-shared:2"}
    )
    refused = runner.run("release", str(changed), reference, "--no-cascade", check=False)
    assert refused.returncode == 65, refused.stderr
    assert tag_digest(repo_path, "1.0.0") == first["manifest_digest"]


def test_mcp_release_keeps_tag_exists(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    base = 'description = "d"\n[server]\ntransport = "stdio"\ncommand = "grim"\n'
    src = _write(
        project_dir / "gdrive.toml",
        base + '[requires.skills]\nhelper = "./helper:1"\n',
    )
    runner = grim_at(project_dir)
    repo_path = f"{unique_repo}/mcp/gdrive"
    reference = f"{registry}/{repo_path}:1.0.0"

    first = runner.json("release", str(src), reference, "--no-cascade", "--kind", "mcp")
    content, requires = fetch_manifest(repo_path, "1.0.0")["layers"]
    assert content["mediaType"] == MCP_TYPE
    assert requires["mediaType"] == BUNDLE_TYPE
    assert fetch_blob(repo_path, requires["digest"]) == members_layer(
        [("skill", "helper", "./helper:1")]
    )
    assert runner.json("release", str(src), reference, "--no-cascade", "--kind", "mcp")["manifest_digest"] == first["manifest_digest"]

    other = _write(
        project_dir / "other/gdrive.toml",
        base + '[requires.skills]\nhelper = "./helper:2"\n',
    )
    refused = runner.run("release", str(other), reference, "--no-cascade", "--kind", "mcp", check=False)
    assert refused.returncode == 65, refused.stderr
    assert tag_digest(repo_path, "1.0.0") == first["manifest_digest"]


def test_unresolvable_ref_refused(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Positive control: a relative ref that stays under the registry root releases.
    ok = _dependent_skill(project_dir / "ok", {"skills.dep": "./dep:1"})
    runner = grim_at(project_dir)
    repo_path = f"{unique_repo}/skill/gws-drive"
    runner.json("release", str(ok), f"{registry}/{repo_path}:1.0.0", "--no-cascade")

    # Climbs above the registry root of the release target.
    bad = _dependent_skill(project_dir / "bad", {"skills.dep": "../../../../../dep:1"})
    result = runner.run(
        "release", str(bad), f"{registry}/{repo_path}:2.0.0", "--no-cascade", check=False
    )
    assert result.returncode == 65, result.stderr
    assert "does not resolve" in result.stderr
    _assert_tag_absent(repo_path, "2.0.0")


def _raw_manifest(repo_path: str, tag: str) -> bytes:
    req = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo_path}/manifests/{tag}",
        headers={"Accept": "application/vnd.oci.image.manifest.v1+json"},
    )
    with urllib.request.urlopen(req) as resp:
        return resp.read()


def test_dependency_free_release_byte_identical(
    grim_at, tmp_path: Path, registry: str
) -> None:
    """The recorded 0.15.1 manifest bytes for the fixture's dependency-free skill."""
    baseline = json.loads(BASELINE.read_text())
    # Outside any git tree, so the manifest carries no revision/created annotation.
    src = tmp_path / "src" / baseline["source_dir_name"]
    for rel, body in baseline["source_files"].items():
        _write(src / rel, body)
    reference = baseline["reference"]
    assert baseline["command"].endswith("--no-cascade")

    # The reference keeps the recorded pull name; the bytes land on the test
    # registry through the push endpoint.
    repo_path = reference.split("/", 1)[1].rsplit(":", 1)[0]
    runner = grim_at(tmp_path)
    report = runner.json(
        "release", str(src), reference, "--no-cascade", "--push-registry", registry
    )

    assert report["manifest_digest"] == baseline["manifest_digest"]
    assert _raw_manifest(repo_path, "1.0.0").decode() == baseline["manifest"]
