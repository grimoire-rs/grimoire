# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Compressed artifact layers — reader-first, opt-in gzip writer.

Contract: ``.agents/adr/adr_compressed_artifact_layers.md``. The reader
accepts three gzip layer types and refuses ``+zstd`` (65,
``unsupported-layer-media-type``); ``--compression gzip`` on ``release`` /
``publish`` writes the custom ``…tar+gzip`` type with a
``com.grimoire.uncompressed.digest`` layer annotation; the release guard
adopts an existing version whose content key matches; ``describe`` reports
an always-present ``compression``. One file because every surface shares
the gzip fixtures below.
"""

from __future__ import annotations

import gzip
import hashlib
import io
import json
import re
import zlib
from collections.abc import Iterator
from pathlib import Path
from typing import Any
from urllib.error import HTTPError

import pytest

from src.assertions import assert_not_exists
from src.helpers import (
    make_artifact,
    make_bundle,
    make_gzip_artifact,
    tar_of,
    write_config,
)
from src.registry import (
    DOCKER_GZIP_LAYER_MEDIA_TYPE,
    GRIMOIRE_GZIP_LAYER_MEDIA_TYPE,
    LAYER_MEDIA_TYPE,
    OCI_GZIP_LAYER_MEDIA_TYPE,
    OCI_ZSTD_LAYER_MEDIA_TYPE,
    UNCOMPRESSED_DIGEST_ANNOTATION,
    fetch_blob,
    fetch_manifest,
    push_artifact,
    tag_digest,
)

UNSUPPORTED_REASON = "unsupported-layer-media-type"

SKILL_FILES: dict[str, str | bytes] = {
    "gz-demo/SKILL.md": "---\nname: gz-demo\ndescription: Gzip demo skill.\n---\n# Gz Demo\n\nBody.\n",
    "gz-demo/references/notes.md": "support file body\n" * 40,
}

MCP_DESCRIPTOR = """\
description = "Compression-ignored MCP descriptor."

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _local_skill(root: Path, name: str = "code-review") -> Path:
    skill = root / name
    _write(
        skill / "SKILL.md",
        f"---\nname: {name}\ndescription: Review code.\n---\n# {name}\n",
    )
    _write(skill / "scripts/run.sh", "echo hi\n" * 50)
    return skill


def _sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def _gunzip_grim_framing(blob: bytes) -> bytes:
    """Decode ``blob`` and assert grim's writer framing: one gzip
    member, header MTIME 0, no FEXTRA/FNAME/FCOMMENT."""
    assert blob[:3] == b"\x1f\x8b\x08", "gzip magic + deflate"
    assert blob[3] & 0x1C == 0, (
        f"FEXTRA/FNAME/FCOMMENT must be unset, FLG={blob[3]:#04x}"
    )
    assert blob[4:8] == b"\0\0\0\0", "header MTIME must be 0"
    decoder = zlib.decompressobj(wbits=31)
    tar = decoder.decompress(blob) + decoder.flush()
    assert decoder.eof and decoder.unused_data == b"", (
        "exactly one gzip member, no trailing bytes"
    )
    return tar


def _only_layer(repo_path: str, tag: str) -> dict[str, Any]:
    layers = fetch_manifest(repo_path, tag)["layers"]
    assert len(layers) == 1, layers
    return layers[0]


def _assert_tag_absent(repo_path: str, tag: str) -> None:
    with pytest.raises(HTTPError) as exc:
        tag_digest(repo_path, tag)
    assert exc.value.code == 404


def _error_doc(result) -> dict[str, Any]:
    return json.loads(result.stdout)["error"]


def _literals(node: Any, root: dict[str, Any]) -> Iterator[Any]:
    """Every ``enum``/``const`` literal reachable from a schema node, following
    local ``$ref``s — robust to how schemars spells an optional enum."""
    if not isinstance(node, dict):
        return
    if "$ref" in node:
        defs = root.get("$defs") or root.get("definitions") or {}
        yield from _literals(defs.get(node["$ref"].rsplit("/", 1)[-1]), root)
    if "const" in node:
        yield node["const"]
    yield from node.get("enum", [])
    for key in ("anyOf", "oneOf", "allOf"):
        for sub in node.get(key, []):
            yield from _literals(sub, root)


# ---------------------------------------------------------------------------
# grim release
# ---------------------------------------------------------------------------


def test_release_gzip_wire_shape(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--compression gzip`: custom `+gzip` layer type, single-member gzip of
    the same tar `--compression none` would push, annotated with that tar's
    digest (== `grim build` layer_digest), and `describe` says gzip."""
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json(
        "release", str(skill), f"{registry}/{repo_path}:1.0.0", "--compression", "gzip"
    )
    assert out["pushed"] is True
    assert out["tags"] == ["1.0.0", "1.0", "1", "latest"]

    manifest = fetch_manifest(repo_path, "1.0.0")
    assert manifest["config"]["mediaType"] == "application/vnd.oci.empty.v1+json"
    assert "artifactType" not in manifest
    assert manifest["annotations"]["com.grimoire.kind"] == "skill"
    layer = manifest["layers"][0]
    assert layer["mediaType"] == GRIMOIRE_GZIP_LAYER_MEDIA_TYPE
    assert set(layer["annotations"]) == {UNCOMPRESSED_DIGEST_ANNOTATION}, layer

    blob = fetch_blob(repo_path, layer["digest"])
    assert layer["size"] == len(blob)
    tar = _gunzip_grim_framing(blob)
    assert layer["annotations"][UNCOMPRESSED_DIGEST_ANNOTATION] == _sha256(tar)
    assert _sha256(tar) == runner.json("build", str(skill))["layer_digest"], (
        "the annotation is the uncompressed tar digest grim build reports"
    )
    assert len(blob) < len(tar)

    assert (
        runner.json("describe", f"{registry}/{repo_path}:1.0.0")["compression"]
        == "gzip"
    )


@pytest.mark.parametrize(
    "flag", [(), ("--compression", "none")], ids=["default", "none"]
)
def test_release_uncompressed_layer_is_unchanged(
    grim_at, project_dir: Path, registry: str, unique_repo: str, flag: tuple[str, ...]
) -> None:
    """Default and explicit `none` push today's descriptor: raw tar type, no
    `annotations` key, layer digest == build layer_digest."""
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    runner.json("release", str(skill), f"{registry}/{repo_path}:1.0.0", *flag)
    layer = _only_layer(repo_path, "1.0.0")
    assert layer["mediaType"] == LAYER_MEDIA_TYPE
    assert "annotations" not in layer, (
        f"an uncompressed layer carries no annotations key: {layer}"
    )
    assert layer["digest"] == runner.json("build", str(skill))["layer_digest"]
    assert (
        runner.json("describe", f"{registry}/{repo_path}:1.0.0")["compression"]
        == "none"
    )


def test_release_default_and_none_are_byte_identical(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--compression none` re-release of a default release is the same manifest."""
    skill = _local_skill(project_dir)
    ref = f"{registry}/{unique_repo}/code-review:1.0.0"
    runner = grim_at(project_dir)

    first = runner.json("release", str(skill), ref)
    second = runner.json("release", str(skill), ref, "--compression", "none")
    assert second["manifest_digest"] == first["manifest_digest"]


def test_release_gzip_is_idempotent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    ref = f"{registry}/{unique_repo}/code-review:1.0.0"
    runner = grim_at(project_dir)

    first = runner.json("release", str(skill), ref, "--compression", "gzip")
    second = runner.json("release", str(skill), ref, "--compression", "gzip")
    assert second["manifest_digest"] == first["manifest_digest"], (
        "gzip output is deterministic"
    )
    assert second["pushed"] is True


def test_release_gzip_dry_run_previews_the_pushed_digest(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The dry-run preview digest includes the layer annotation
    (`preview_manifest_digest` must not drop it)."""
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)

    preview = runner.json(
        "release", str(skill), ref, "--compression", "gzip", "--dry-run"
    )
    assert preview["pushed"] is False
    _assert_tag_absent(repo_path, "1.0.0")
    real = runner.json("release", str(skill), ref, "--compression", "gzip")
    assert preview["manifest_digest"] == real["manifest_digest"]


@pytest.mark.parametrize(("first", "second"), [("none", "gzip"), ("gzip", "none")])
def test_release_compression_flip_on_unchanged_content_adopts(
    grim_at, project_dir: Path, registry: str, unique_repo: str, first: str, second: str
) -> None:
    """A flip on unchanged content is not `TagExists` — exit 0, the
    version tag and its cascade stay on the existing manifest, the report
    shows that digest with `pushed: true`, and stderr names it."""
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)

    existing = runner.json("release", str(skill), ref, "--compression", first)[
        "manifest_digest"
    ]
    result = runner.run(
        "release", str(skill), ref, "--compression", second, format="json", check=False
    )
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["pushed"] is True
    assert report["manifest_digest"] == existing, "adoption reports the existing digest"
    assert existing in result.stderr, "one stderr line names the adopted digest"
    for tag in ("1.0.0", "1.0", "1", "latest"):
        assert tag_digest(repo_path, tag) == existing, (
            f"{tag} must stay on the adopted manifest"
        )


def test_release_content_change_with_compression_flip_is_tag_exists(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)

    existing = runner.json("release", str(skill), ref)["manifest_digest"]
    (skill / "scripts/run.sh").write_text("echo CHANGED\n")
    result = runner.run(
        "release", str(skill), ref, "--compression", "gzip", check=False
    )
    assert result.returncode == 65, result.stderr
    assert tag_digest(repo_path, "1.0.0") == existing


def test_release_force_gzip_moves_tag_and_changes_only_the_layer(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`--force` skips adoption: it converts an existing version, and only the
    layer descriptor differs. Forcing back to `none` restores the old digest."""
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)

    plain = runner.json("release", str(skill), ref)["manifest_digest"]
    plain_manifest = fetch_manifest(repo_path, "1.0.0")

    forced = runner.json("release", str(skill), ref, "--force", "--compression", "gzip")
    assert forced["manifest_digest"] != plain
    assert tag_digest(repo_path, "1.0.0") == forced["manifest_digest"]
    assert tag_digest(repo_path, "latest") == forced["manifest_digest"]
    gzip_manifest = fetch_manifest(repo_path, "1.0.0")
    assert {k: v for k, v in gzip_manifest.items() if k != "layers"} == {
        k: v for k, v in plain_manifest.items() if k != "layers"
    }, "config, media type and manifest annotations are unchanged"
    assert gzip_manifest["layers"][0]["mediaType"] == GRIMOIRE_GZIP_LAYER_MEDIA_TYPE

    back = runner.json("release", str(skill), ref, "--force", "--compression", "none")
    assert back["manifest_digest"] == plain
    assert tag_digest(repo_path, "1.0.0") == plain


@pytest.mark.parametrize("kind", ["rule", "agent"])
def test_release_force_gzip_installs_same_files_as_uncompressed(
    grim_at, project_dir: Path, registry: str, unique_repo: str, kind: str
) -> None:
    """A gzip release of a rule or an agent materializes byte-identical files
    to the uncompressed release of the same source at the same ref. (Hooks
    are not covered: installing one needs the experimental flag and consent.)"""
    name = f"demo-{kind}"
    if kind == "rule":
        body = "---\npaths: ['**/*.md']\n---\n# demo-rule\nbody\n"
        kind_flags: tuple[str, ...] = ()
    else:
        body = "---\nname: demo-agent\ndescription: A test agent.\nmodel: sonnet\n---\n# demo-agent\nBody.\n"
        kind_flags = ("--kind", "agent")
    source = project_dir / "src" / f"{name}.md"
    _write(source, body)
    repo_path = f"{unique_repo}/{name}"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)

    installed: dict[str, dict[str, bytes]] = {}
    for label, flags in (("plain", ()), ("gzip", ("--force", "--compression", "gzip"))):
        runner.json("release", *kind_flags, str(source), ref, *flags)
        proj = project_dir / label
        (proj / ".claude").mkdir(parents=True)
        write_config(proj, **{f"{kind}s": {name: ref}})
        proj_runner = grim_at(proj)
        proj_runner.run("lock")
        proj_runner.run("install", "--client", "claude")
        installed[label] = {
            str(p.relative_to(proj / ".claude")): p.read_bytes()
            for p in sorted((proj / ".claude").rglob(f"{name}.md"))
        }

    assert (
        _only_layer(repo_path, "1.0.0")["mediaType"] == GRIMOIRE_GZIP_LAYER_MEDIA_TYPE
    )
    assert installed["plain"], "the plain release materialized the artifact"
    assert installed["gzip"] == installed["plain"]


@pytest.mark.parametrize("kind", ["mcp", "bundle"])
def test_release_does_not_adopt_a_gzip_layer_on_bundle_or_mcp(
    grim_at, project_dir: Path, registry: str, unique_repo: str, kind: str
) -> None:
    """Bundle and mcp never write gzip, so a version tag already holding a
    gzip-typed layer with the right `uncompressed.digest` and identical manifest
    annotations is not an adoptable match: `release` exits 65 (`TagExists`) and
    the tag stays on the foreign manifest."""
    repo_path = f"{unique_repo}/demo-{kind}"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)
    if kind == "mcp":
        source = project_dir / "demo-mcp.toml"
        _write(source, MCP_DESCRIPTOR)
        release_args: tuple[str, ...] = ("--kind", "mcp", str(source), ref)
    else:
        member = make_artifact(
            f"{unique_repo}/member", "skill", {"member/SKILL.md": "# m\n"}
        )
        source = project_dir / "demo-bundle.toml"
        _write(source, f'[skills]\nmember = "{member.fq}"\n')
        release_args = (str(source), ref)

    runner.json("release", *release_args)
    original = fetch_manifest(repo_path, "1.0.0")
    raw = fetch_blob(repo_path, original["layers"][0]["digest"])
    # Re-publish the same content under a gzip-typed layer at the same tag.
    foreign = push_artifact(
        repo_path,
        "1.0.0",
        gzip.compress(raw, mtime=0),
        kind,
        dict(original["annotations"]),
        layer_media_type=GRIMOIRE_GZIP_LAYER_MEDIA_TYPE,
        layer_annotations={UNCOMPRESSED_DIGEST_ANNOTATION: _sha256(raw)},
    )
    assert tag_digest(repo_path, "1.0.0") == foreign.digest

    result = runner.run("release", *release_args, check=False)
    assert result.returncode == 65, result.stderr
    assert tag_digest(repo_path, "1.0.0") == foreign.digest


def test_release_skip_existing_ignores_compression(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    ref = f"{registry}/{repo_path}:1.0.0"
    runner = grim_at(project_dir)

    existing = runner.json("release", str(skill), ref)["manifest_digest"]
    skipped = runner.json(
        "release", str(skill), ref, "--skip-existing", "--compression", "gzip"
    )
    assert skipped["pushed"] is False
    assert skipped["tags"] == []
    assert tag_digest(repo_path, "1.0.0") == existing
    assert _only_layer(repo_path, "1.0.0")["mediaType"] == LAYER_MEDIA_TYPE


def test_release_compression_zstd_is_usage_error(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    result = runner.run(
        "release",
        str(skill),
        f"{registry}/{repo_path}:1.0.0",
        "--compression",
        "zstd",
        check=False,
    )
    assert result.returncode == 64, result.stderr
    assert "invalid value 'zstd'" in result.stderr, result.stderr
    _assert_tag_absent(repo_path, "1.0.0")


def test_gzip_annotation_equals_path_lock_hash_and_status_stays_installed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The path-source lock `hash`, `grim build` layer_digest and the gzip
    annotation are one digest, and a gzip release of the source leaves the
    path-sourced install `installed`."""
    skill = _local_skill(project_dir / "skills")
    _write(
        project_dir / "grimoire.toml",
        '[skills]\ncode-review = "./skills/code-review"\n',
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install", "--client", "claude")
    match = re.search(
        r'^hash = "(sha256:[0-9a-f]{64})"',
        (project_dir / "grimoire.lock").read_text(),
        re.MULTILINE,
    )
    assert match, "path entry carries a sha256 hash"

    repo_path = f"{unique_repo}/code-review"
    runner.json(
        "release", str(skill), f"{registry}/{repo_path}:1.0.0", "--compression", "gzip"
    )
    annotation = _only_layer(repo_path, "1.0.0")["annotations"][
        UNCOMPRESSED_DIGEST_ANNOTATION
    ]
    assert (
        annotation == match.group(1) == runner.json("build", str(skill))["layer_digest"]
    )
    assert runner.json("status")["items"][0]["state"] == "installed"


# ---------------------------------------------------------------------------
# grim publish + schema
# ---------------------------------------------------------------------------


def _publish_tree(
    project_dir: Path,
    registry: str,
    repo_prefix: str,
    *,
    compression: str | None,
    extra_kinds: bool = False,
) -> Path:
    """A publish.toml with a skill and a rule (plus a bundle and an mcp entry
    when ``extra_kinds``), all under ``repo_prefix/<name>``."""
    _local_skill(project_dir / "skills", "demo-skill")
    _write(
        project_dir / "rules/demo-rule.md",
        "---\npaths: ['**/*.md']\n---\n# demo-rule\nbody\n",
    )
    head = [
        f'registry = "{registry}"',
        f'repository_prefix = "{repo_prefix}"',
        'version = "0.1.0"',
    ]
    if compression is not None:
        head.append(f'compression = "{compression}"')
    tables = ["[skills.demo-skill]", "[rules.demo-rule]"]
    if extra_kinds:
        _write(project_dir / "mcp/demo-mcp.toml", MCP_DESCRIPTOR)
        member = f"{registry}/{repo_prefix}/demo-skill:0.1.0"
        _write(
            project_dir / "bundles/demo-bundle.toml", f'[skills]\nmember = "{member}"\n'
        )
        tables += ["[mcp.demo-mcp]", "[bundles.demo-bundle]"]
    manifest = project_dir / "publish.toml"
    manifest.write_text("\n".join(head) + "\n\n" + "\n\n".join(tables) + "\n")
    return manifest


def test_publish_manifest_compression_gzip(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    manifest = _publish_tree(project_dir, registry, unique_repo, compression="gzip")
    rows = grim_at(project_dir).json("publish", "--manifest", str(manifest))["items"]
    assert {r["status"] for r in rows} == {"pushed"}, rows

    for name in ("demo-skill", "demo-rule"):
        layer = _only_layer(f"{unique_repo}/{name}", "0.1.0")
        assert layer["mediaType"] == GRIMOIRE_GZIP_LAYER_MEDIA_TYPE, name
        assert UNCOMPRESSED_DIGEST_ANNOTATION in layer["annotations"], name


@pytest.mark.parametrize(
    ("manifest_value", "flag", "expected"),
    [
        ("gzip", "none", LAYER_MEDIA_TYPE),
        (None, "gzip", GRIMOIRE_GZIP_LAYER_MEDIA_TYPE),
        ("none", "gzip", GRIMOIRE_GZIP_LAYER_MEDIA_TYPE),
    ],
    ids=[
        "flag-none-beats-manifest-gzip",
        "flag-gzip-no-key",
        "flag-gzip-beats-manifest-none",
    ],
)
def test_publish_compression_flag_overrides_manifest(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    manifest_value: str | None,
    flag: str,
    expected: str,
) -> None:
    manifest = _publish_tree(
        project_dir, registry, unique_repo, compression=manifest_value
    )
    grim_at(project_dir).json(
        "publish", "--manifest", str(manifest), "--compression", flag
    )

    layer = _only_layer(f"{unique_repo}/demo-skill", "0.1.0")
    assert layer["mediaType"] == expected
    assert ("annotations" in layer) == (expected == GRIMOIRE_GZIP_LAYER_MEDIA_TYPE), (
        layer
    )


def test_publish_compression_ignored_for_bundle_and_mcp(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The top-level key applies batch-wide; bundle and mcp layers stay as today."""
    manifest = _publish_tree(
        project_dir, registry, unique_repo, compression="gzip", extra_kinds=True
    )
    rows = grim_at(project_dir).json("publish", "--manifest", str(manifest))["items"]
    assert {r["status"] for r in rows} == {"pushed"}, rows

    assert (
        _only_layer(f"{unique_repo}/demo-skill", "0.1.0")["mediaType"]
        == GRIMOIRE_GZIP_LAYER_MEDIA_TYPE
    )
    for name, media_type in (
        ("demo-bundle", "application/vnd.grimoire.bundle.v1+json"),
        ("demo-mcp", "application/vnd.grimoire.mcp.v1+json"),
    ):
        layer = _only_layer(f"{unique_repo}/{name}", "0.1.0")
        assert layer["mediaType"] == media_type, name
        assert "annotations" not in layer, name


def test_publish_invalid_manifest_compression_is_data_error(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    manifest = _publish_tree(project_dir, registry, unique_repo, compression="zstd")
    result = grim_at(project_dir).run(
        "publish", "--manifest", str(manifest), check=False
    )
    assert result.returncode == 65, result.stderr
    assert "zstd" in result.stderr, result.stderr
    _assert_tag_absent(f"{unique_repo}/demo-skill", "0.1.0")


def test_publish_compression_flag_zstd_is_usage_error(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    manifest = _publish_tree(project_dir, registry, unique_repo, compression=None)
    result = grim_at(project_dir).run(
        "publish", "--manifest", str(manifest), "--compression", "zstd", check=False
    )
    assert result.returncode == 64, result.stderr
    assert "invalid value 'zstd'" in result.stderr, result.stderr
    _assert_tag_absent(f"{unique_repo}/demo-skill", "0.1.0")


def test_schema_publish_has_optional_compression(grim_at, project_dir: Path) -> None:
    schema = json.loads(grim_at(project_dir).run("schema", "--kind", "publish").stdout)
    prop = schema["properties"]["compression"]
    assert {"none", "gzip"} <= set(_literals(prop, schema)), prop
    assert "compression" not in schema.get("required", [])


# ---------------------------------------------------------------------------
# grim describe
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("variant", "expected"),
    [
        ("tar", "none"),
        ("grimoire-gzip", "gzip"),
        ("oci-gzip", "gzip"),
        ("zstd", None),
        ("bundle", "none"),
    ],
)
def test_describe_reports_compression(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    variant: str,
    expected: str | None,
) -> None:
    repo = f"{unique_repo}/gz-demo"
    if variant == "tar":
        make_artifact(repo, "skill", SKILL_FILES)
    elif variant == "grimoire-gzip":
        make_gzip_artifact(repo, "skill", SKILL_FILES)
    elif variant == "oci-gzip":
        make_gzip_artifact(
            repo, "skill", SKILL_FILES, media_type=OCI_GZIP_LAYER_MEDIA_TYPE
        )
    elif variant == "zstd":
        make_artifact(
            repo, "skill", SKILL_FILES, layer_media_type=OCI_ZSTD_LAYER_MEDIA_TYPE
        )
    else:
        member = make_artifact(
            f"{unique_repo}/member", "skill", {"member/SKILL.md": "# m\n"}
        )
        make_bundle(repo, [("skill", "member", member.fq)])

    doc = grim_at(project_dir).json("describe", f"{registry}/{repo}:latest")
    assert "compression" in doc, "always present"
    assert doc["compression"] == expected


def test_describe_plain_has_compression_row(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/gz-demo"
    make_gzip_artifact(repo, "skill", SKILL_FILES)
    out = grim_at(project_dir).plain("describe", f"{registry}/{repo}:latest").stdout
    rows = [line.split() for line in out.splitlines()]
    assert ["compression", "gzip"] in [row[:2] for row in rows], out


# ---------------------------------------------------------------------------
# grim fetch
# ---------------------------------------------------------------------------


def test_fetch_gzip_artifact_matches_uncompressed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    plain_ref = make_artifact(f"{unique_repo}/plain/gz-demo", "skill", SKILL_FILES).fq
    gz_ref = make_gzip_artifact(f"{unique_repo}/gz/gz-demo", "skill", SKILL_FILES).fq
    runner = grim_at(project_dir)

    assert runner.plain("fetch", gz_ref).stdout == SKILL_FILES["gz-demo/SKILL.md"]
    support = runner.plain(
        "fetch", gz_ref, "--path", "gz-demo/references/notes.md"
    ).stdout
    assert support == SKILL_FILES["gz-demo/references/notes.md"]
    plain_doc, gz_doc = runner.json("fetch", plain_ref), runner.json("fetch", gz_ref)
    assert gz_doc["content"] == plain_doc["content"]
    assert gz_doc["files"] == plain_doc["files"]


def test_fetch_digest_only_shape_unchanged_for_gzip(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    published = make_gzip_artifact(f"{unique_repo}/gz-demo", "skill", SKILL_FILES)
    (project_dir / "grimoire.toml").write_text("[skills]\n")
    probe = grim_at(project_dir).json("fetch", published.fq, "--digest-only")
    assert set(probe) == {"ref", "digest"}, (
        f"no compression key on the digest probe: {probe}"
    )
    assert probe["digest"] == published.digest


def test_fetch_zstd_layer_is_unsupported_media_type(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    ref = make_artifact(
        f"{unique_repo}/gz-demo",
        "skill",
        SKILL_FILES,
        layer_media_type=OCI_ZSTD_LAYER_MEDIA_TYPE,
    ).fq
    result = grim_at(project_dir).run("fetch", ref, format="json", check=False)
    assert result.returncode == 65, result.stderr
    err = _error_doc(result)
    assert (err["exit"], err["reason"]) == (65, UNSUPPORTED_REASON), err


# ---------------------------------------------------------------------------
# install / add
# ---------------------------------------------------------------------------


def _assert_installed(project_dir: Path) -> None:
    root = project_dir / ".claude/skills/gz-demo"
    for rel, body in SKILL_FILES.items():
        assert (root / rel.removeprefix("gz-demo/")).read_text() == body, rel


@pytest.mark.parametrize(
    "gzip_options",
    [
        {},
        {"media_type": OCI_GZIP_LAYER_MEDIA_TYPE},
        {"media_type": DOCKER_GZIP_LAYER_MEDIA_TYPE},
        {"media_type": OCI_GZIP_LAYER_MEDIA_TYPE, "annotate": False},
        {"uncompressed_digest": "blake3:" + "0" * 64},
    ],
    ids=[
        "grimoire-gzip",
        "oci-gzip",
        "docker-gzip",
        "oci-gzip-unannotated",
        "unknown-algorithm-skipped",
    ],
)
def test_install_gzip_artifact_materializes_same_files(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    gzip_options: dict[str, Any],
) -> None:
    published = make_gzip_artifact(
        f"{unique_repo}/gz-demo", "skill", SKILL_FILES, **gzip_options
    )
    write_config(project_dir, skills={"gz-demo": published.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}, rows
    _assert_installed(project_dir)
    assert runner.json("status")["items"][0]["state"] == "installed"


def test_add_gzip_artifact_installs(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    published = make_gzip_artifact(f"{unique_repo}/gz-demo", "skill", SKILL_FILES)
    write_config(project_dir)
    added = grim_at(project_dir).json("add", published.fq)
    assert added["kind"] == "skill"
    _assert_installed(project_dir)


def test_install_zstd_layer_is_unsupported_media_type(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    published = make_artifact(
        f"{unique_repo}/gz-demo",
        "skill",
        SKILL_FILES,
        layer_media_type=OCI_ZSTD_LAYER_MEDIA_TYPE,
    )
    write_config(project_dir, skills={"gz-demo": published.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    result = runner.run("install", format="json", check=False)
    assert result.returncode == 65, result.stderr
    err = _error_doc(result)
    assert (err["code"], err["exit"], err["reason"]) == (
        "data",
        65,
        UNSUPPORTED_REASON,
    ), err
    assert_not_exists(project_dir / ".claude/skills/gz-demo")


def _fname_header_gzip(data: bytes) -> bytes:
    buf = io.BytesIO()
    with gzip.GzipFile(filename="layer.tar", mode="wb", fileobj=buf, mtime=0) as gz:
        gz.write(data)
    return buf.getvalue()


_TAR = tar_of(SKILL_FILES)


_DIGEST_MISMATCH = "does not match com.grimoire.uncompressed.digest"
_MALFORMED_ANNOTATION = "malformed com.grimoire.uncompressed.digest"
_NOT_SINGLE_MEMBER = "single-member gzip"


@pytest.mark.parametrize(
    ("gzip_options", "expected"),
    [
        ({"uncompressed_digest": "sha256:" + "0" * 64}, _DIGEST_MISMATCH),
        ({"uncompressed_digest": "sha256:not-hex"}, _MALFORMED_ANNOTATION),
        (
            {
                "uncompressed_digest": "sha256:"
                + hashlib.sha256(_TAR).hexdigest().upper()
            },
            _MALFORMED_ANNOTATION,
        ),
        (
            {"blob": gzip.compress(_TAR, mtime=0) + gzip.compress(b"", mtime=0)},
            _NOT_SINGLE_MEMBER,
        ),
        ({"blob": gzip.compress(_TAR, mtime=0) + b"trailing"}, _NOT_SINGLE_MEMBER),
        ({"blob": _fname_header_gzip(_TAR)}, _NOT_SINGLE_MEMBER),
        ({"blob": b"\x1f\x8b\x08\x00not really gzip"}, _NOT_SINGLE_MEMBER),
    ],
    ids=[
        "digest-mismatch",
        "malformed-annotation",
        "uppercase-hex",
        "second-member",
        "trailing-bytes",
        "fname-header",
        "corrupt",
    ],
)
def test_install_refuses_bad_gzip_layer_without_writing(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    gzip_options: dict[str, Any],
    expected: str,
) -> None:
    """A lying or malformed annotation, or non-strict framing, exits 65 and
    writes nothing, naming the specific failure. These carry no reason slug
    (only `+zstd` does)."""
    published = make_gzip_artifact(
        f"{unique_repo}/gz-demo", "skill", SKILL_FILES, **gzip_options
    )
    write_config(project_dir, skills={"gz-demo": published.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    result = runner.run("install", format="json", check=False)
    assert result.returncode == 65, result.stderr
    err = _error_doc(result)
    assert err.get("reason") != UNSUPPORTED_REASON
    # Only the frozen Display text is matched; the source detail may vary.
    assert expected in err["message"] + result.stderr, (err, result.stderr)
    assert_not_exists(project_dir / ".claude/skills/gz-demo")
