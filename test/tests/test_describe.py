# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim describe` acceptance tests — manifest-level metadata, no blob pull."""
from __future__ import annotations

import json
import uuid
from pathlib import Path

from src.helpers import make_artifact, make_description


SKILL_DOC = (
    "---\n"
    "name: describe-demo\n"
    "description: Demo skill for describe tests.\n"
    "---\n"
    "# Describe Demo\n"
)

FULL_ANNOTATIONS = {
    "org.opencontainers.image.title": "describe-demo",
    "org.opencontainers.image.description": "Demo skill for describe tests.",
    "com.grimoire.summary": "terse blurb",
    "org.opencontainers.image.version": "1.2.0",
    "org.opencontainers.image.licenses": "Apache-2.0",
    "org.opencontainers.image.source": "https://github.com/acme/describe-demo",
    "com.grimoire.keywords": "review, quality",
    "com.grimoire.deprecated": "use acme/describe-demo-2",
    "com.grimoire.replaced-by": "ghcr.io/acme/skills/describe-demo-2",
}


def _publish(registry: str, unique_repo: str, *, tags=("latest",), annotations=None) -> str:
    """Publish a describe-demo skill under one or more tags; return its ref."""
    repo = f"{unique_repo}/skills/describe-demo"
    for tag in tags:
        make_artifact(
            repo,
            "skill",
            {"describe-demo/SKILL.md": SKILL_DOC},
            tag=tag,
            annotations=annotations,
        )
    return f"{registry}/{repo}:{tags[0]}"


def test_describe_json_reports_all_curated_fields(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    ref = _publish(registry, unique_repo, annotations=FULL_ANNOTATIONS)
    runner = grim_at(project_dir)
    doc = runner.json("describe", ref)

    # Single object (not an items envelope), every field present.
    assert isinstance(doc, dict)
    assert "items" not in doc and "error" not in doc
    assert doc["kind"] == "skill"
    assert doc["name"] == "describe-demo"
    assert doc["title"] == "describe-demo"
    assert doc["description"] == "Demo skill for describe tests."
    assert doc["summary"] == "terse blurb"
    assert doc["version"] == "1.2.0"
    assert doc["license"] == "Apache-2.0"
    assert doc["repository"] == "https://github.com/acme/describe-demo"
    assert doc["keywords"] == ["review", "quality"], "split + trimmed"
    assert doc["deprecated"] == "use acme/describe-demo-2"
    assert doc["replaced_by"] == "ghcr.io/acme/skills/describe-demo-2"
    assert doc["digest"].startswith("sha256:")
    # The verbatim annotation map is carried whole.
    assert doc["annotations"]["com.grimoire.replaced-by"] == "ghcr.io/acme/skills/describe-demo-2"
    # git-provenance keys are absent here ⇒ explicit null (always-present policy).
    assert doc["revision"] is None
    assert doc["created"] is None


def test_describe_bare_manifest_nulls_kind_not_error(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A foreign manifest (unrecognized kind, no grimoire annotations) does
    not hard-error: kind is null and the curated fields fall to null."""
    repo = f"{unique_repo}/mystery/opaque"
    make_artifact(repo, "widget", {"opaque.md": "opaque\n"}, tag="latest")
    runner = grim_at(project_dir)
    doc = runner.json("describe", f"{registry}/{repo}:latest")

    assert doc["kind"] is None, "unrecognized kind ⇒ null, no error"
    assert doc["summary"] is None
    assert doc["deprecated"] is None
    assert doc["replaced_by"] is None
    assert doc["keywords"] == [], "no keywords ⇒ empty array"
    assert doc["tags"] == ["latest"]
    assert doc["name"] == "opaque"


def test_describe_lists_all_tags_sorted(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    ref = _publish(registry, unique_repo, tags=("latest", "1.0.0", "1.2.0"))
    runner = grim_at(project_dir)
    doc = runner.json("describe", ref)
    assert doc["tags"] == ["1.0.0", "1.2.0", "latest"], "tags sorted"


def test_describe_deprecated_and_replaced_by_together(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`deprecated` and `replaced_by` are independent and both surface."""
    ref = _publish(
        registry,
        unique_repo,
        annotations={
            "com.grimoire.deprecated": "retired",
            "com.grimoire.replaced-by": "ghcr.io/acme/skills/successor",
        },
    )
    runner = grim_at(project_dir)
    doc = runner.json("describe", ref)
    assert doc["deprecated"] == "retired"
    assert doc["replaced_by"] == "ghcr.io/acme/skills/successor"


def test_describe_plain_is_key_value_table(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    ref = _publish(registry, unique_repo, tags=("latest", "1.0.0"), annotations=FULL_ANNOTATIONS)
    runner = grim_at(project_dir)
    out = runner.plain("describe", ref).stdout
    assert out.splitlines()[0].startswith("Key"), "flat key/value table like grim context"
    assert "skill" in out
    assert "1.0.0,latest" in out, "tags comma-joined"


def test_describe_has_description_true_when_companion_present(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`has_description` is true when the repository carries a `__grimoire`
    companion — derived from the tag listing describe already fetches, and the
    internal tag stays hidden from `tags[]`."""
    repo = f"{unique_repo}/skills/describe-demo"
    make_artifact(repo, "skill", {"describe-demo/SKILL.md": SKILL_DOC}, tag="latest")
    make_description(repo, {"README.md": b"# Repo\n"})
    runner = grim_at(project_dir)

    doc = runner.json("describe", f"{registry}/{repo}:latest")
    assert doc["has_description"] is True
    assert "__grimoire" not in doc["tags"], "the internal companion tag stays hidden from tags[]"


def test_describe_has_description_false_when_absent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`has_description` is an always-present bool — false when no companion."""
    ref = _publish(registry, unique_repo)
    runner = grim_at(project_dir)
    doc = runner.json("describe", ref)
    assert doc["has_description"] is False


def test_describe_offline_uncached_is_offline_blocked_81(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """GRIM_OFFLINE=1 on an uncached reference is offline-blocked (81), an
    error document on stdout — not a misleading not-found."""
    ref = _publish(registry, unique_repo)
    runner = grim_at(project_dir)
    runner.env["GRIM_OFFLINE"] = "1"  # nothing is cached in the fresh grim_home
    result = runner.run("--format", "json", "describe", ref, check=False)
    assert result.returncode == 81, result.stderr
    doc = json.loads(result.stdout)
    assert doc["error"]["code"] == "offline-blocked"
    assert doc["error"]["exit"] == 81


def test_describe_unknown_flag_exits_64_without_json(
    grim_at, project_dir: Path
) -> None:
    """A clap parse failure (unknown flag) is the pre-contract boundary:
    exit 64, plain usage on stderr, no JSON error document on stdout."""
    runner = grim_at(project_dir)
    ref = f"localhost:5000/grim-test/{uuid.uuid4().hex[:12]}/skills/x:latest"
    result = runner.run("--format", "json", "describe", ref, "--bogus", check=False)
    assert result.returncode == 64
    assert result.stdout == "", f"no JSON document before the parse boundary: {result.stdout!r}"
    assert result.stderr.strip(), "clap usage message on stderr"
