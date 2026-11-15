# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim release` acceptance tests — push + cascade tags.

These build a *real* local skill directory / rule file in a tmp path and
`grim release` it (not `make_artifact`), exercising the full
validate → pack → push → cascade-tag path against the live registry.
"""
from __future__ import annotations

import json
from pathlib import Path

from urllib.error import HTTPError

from src.helpers import publish_release, release_with_grim, write_config
from src.registry import fetch_manifest, retag, tag_digest


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body, encoding="utf-8", newline="\n")


def _assert_tag_absent(repo_path: str, tag: str) -> None:
    """Assert the registry has no manifest for ``repo_path:tag`` — proves a
    refused release pushed nothing."""
    try:
        tag_digest(repo_path, tag)
    except HTTPError as exc:
        assert exc.code == 404, f"expected a 404 for {repo_path}:{tag}, got {exc.code}"
        return
    raise AssertionError(f"{repo_path}:{tag} unexpectedly resolved — the release was not refused")


def _local_skill(project_dir: Path, name: str = "code-review") -> Path:
    skill = project_dir / name
    _write(
        skill / "SKILL.md",
        f"---\nname: {name}\ndescription: Review code.\n"
        f"metadata:\n  keywords: review,quality\n---\n# {name}\n",
    )
    _write(skill / "scripts/run.sh", "echo hi\n")
    return skill


def test_release_wire_shape_empty_config_and_kind_annotation(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Issue #11 axis A: the GitLab-safe wire contract. GitLab's media-type
    allowlist rejects both the custom config type AND the custom top-level
    ``artifactType`` (confirmed against real GitLab SaaS), so the pushed
    manifest carries the OCI empty config and NO ``artifactType`` — the kind
    rides solely on the ``com.grimoire.kind`` annotation.

    zot accepts both shapes, so this asserts the wire shape that
    satisfies GitLab, not GitLab's rejection itself.
    """
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3")
    assert out["pushed"] is True

    manifest = fetch_manifest(repo_path, "1.2.3")
    assert manifest["config"]["mediaType"] == "application/vnd.oci.empty.v1+json", (
        "config descriptor must be the OCI empty type (GitLab allowlist), "
        f"got {manifest['config']['mediaType']}"
    )
    # The empty config blob is the byte-identical `{}` (size 2).
    assert manifest["config"]["size"] == 2, (
        f"empty config blob must be 2 bytes, got {manifest['config']['size']}"
    )
    # No custom artifactType on the wire — GitLab rejects it.
    assert "artifactType" not in manifest, (
        "manifest must NOT carry a custom artifactType (GitLab rejects it), "
        f"got {manifest.get('artifactType')}"
    )
    # The kind rides solely on the registry-agnostic fallback annotation.
    annotations = manifest.get("annotations") or {}
    assert annotations.get("com.grimoire.kind") == "skill", (
        f"manifest must carry com.grimoire.kind=skill, got {annotations}"
    )


def test_release_then_add_infers_skill_kind_from_real_wire_shape(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Issue #11 axis A, the publish-then-read-back guarantee for a *skill*.

    The harness (`registry.py push_artifact`) deliberately stamps a legacy
    `artifactType`, so the `make_artifact`-based add tests resolve kind via
    read tier 1 — they never exercise grim's actual annotation-only output.
    This releases a real skill through `grim release` (no `artifactType`, kind
    in `com.grimoire.kind` only) and then `grim add`s it back with no `--kind`,
    proving grim can still type an artifact it published. Previously only the
    `agent` kind had this end-to-end coverage (`test_agents.py`).
    """
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3")
    assert out["pushed"] is True

    write_config(project_dir)
    added = runner.json("add", f"{repo}:1.2.3")
    assert added["kind"] == "skill", (
        f"grim must infer skill from its own annotation-only manifest, got {added.get('kind')!r}"
    )


def test_release_pushes_with_cascade_tags(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3")
    assert out["pushed"] is True
    assert out["tags"] == ["1.2.3", "1.2", "1", "latest"]
    digest = out["manifest_digest"]
    assert digest.startswith("sha256:")

    # Every cascade tag must resolve to the same manifest digest. Use
    # lock + install per the spine: declare each tag and lock it.
    write_config(
        project_dir,
        skills={
            "cr-exact": f"{repo}:1.2.3",
            "cr-minor": f"{repo}:1.2",
            "cr-major": f"{repo}:1",
            "cr-latest": f"{repo}:latest",
        },
    )
    runner.run("lock", check=False)
    rows = runner.json("status")["items"]
    # `grim lock` pins every declared tag; they must all share one digest.
    locked = runner.json("lock")["items"]
    pins = {r["name"]: r["pinned"] for r in locked}
    assert len(set(pins.values())) == 1, (
        f"all cascade tags must pin the same digest, got {pins}"
    )
    assert rows  # status renders the declared set


def test_rerelease_moves_preexisting_floating_tags(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A higher re-release must MOVE the pre-existing ``:1``/``:latest``.

    The rolling-release headline: ``1.0.0`` is published (cascade creates
    ``:1``/``:latest`` ⇒ digest A). A different-content ``1.1.0`` is then
    released to the SAME repo. ``:1.1.0``/``:1.1`` are new, but ``:1`` and
    ``:latest`` already exist at digest A — the cascade must re-point them
    at the new digest B, otherwise a project pinning the floating ``:1``
    tag never rolls forward.
    """
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    repo_path = f"{unique_repo}/code-review"

    skill = _local_skill(project_dir)
    v1 = runner.json("release", str(skill), f"{repo}:1.0.0")
    digest_a = v1["manifest_digest"]
    for tag in ("1.0.0", "1.0", "1", "latest"):
        assert tag_digest(repo_path, tag) == digest_a, (
            f"1.0.0 cascade must put {tag} at digest A"
        )

    # Lock the project against the floating :1 tag while it still points
    # at digest A — captures the pre-roll pin so a later plain update can
    # be observed rolling forward.
    write_config(project_dir, skills={"code-review": f"{repo}:1"})
    runner.run("lock", check=False)
    assert digest_a in (project_dir / "grimoire.lock").read_text(encoding="utf-8"), (
        "lock at :1 must pin digest A before the rolling release"
    )

    # Different content, higher version, same repo.
    (skill / "scripts/run.sh").write_text("echo v1.1.0 CHANGED\n", encoding="utf-8", newline="\n")
    v2 = runner.json("release", str(skill), f"{repo}:1.1.0")
    digest_b = v2["manifest_digest"]
    assert digest_b != digest_a, "1.1.0 must produce a different digest"
    # New tags created at B; pre-existing :1 and :latest MOVED to B;
    # the older minor :1.0 / exact :1.0.0 stay at A (immutable history).
    assert tag_digest(repo_path, "1.1.0") == digest_b
    assert tag_digest(repo_path, "1.1") == digest_b
    assert tag_digest(repo_path, "1") == digest_b, (
        "pre-existing major tag :1 must roll forward to the new digest"
    )
    assert tag_digest(repo_path, "latest") == digest_b, (
        "pre-existing :latest must roll forward to the new digest"
    )
    assert tag_digest(repo_path, "1.0.0") == digest_a
    assert tag_digest(repo_path, "1.0") == digest_a

    # End-to-end rolling release: the project pinned the floating :1 tag
    # at digest A above; a plain `grim update` (no names) must roll it
    # forward to B now that the cascade moved :1.
    rows = runner.json("update")["items"]
    by_name = {r["name"]: r for r in rows}
    assert by_name["code-review"]["action"] == "updated", (
        "plain `grim update` must roll the floating :1 pin forward"
    )
    assert digest_b in (project_dir / "grimoire.lock").read_text(encoding="utf-8")


def test_release_dry_run_does_not_push(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:2.0.0", "--dry-run")
    assert out["pushed"] is False
    assert out["tags"] == ["2.0.0", "2.0", "2", "latest"]

    # Nothing was pushed: locking the tag must fail to resolve (79).
    write_config(project_dir, skills={"cr": f"{repo}:2.0.0"})
    result = runner.run("lock", check=False)
    assert result.returncode == 79, (
        f"dry-run must not push (lock should 404→79), got "
        f"{result.returncode}; {result.stderr}"
    )


def test_release_is_idempotent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    first = runner.json("release", str(skill), f"{repo}:1.0.0")
    second = runner.json("release", str(skill), f"{repo}:1.0.0")
    assert first["manifest_digest"] == second["manifest_digest"], (
        "re-releasing identical content must yield the same digest"
    )
    assert second["pushed"] is True


def test_release_refuses_overwrite_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    v1 = _local_skill(project_dir, "code-review")
    runner.json("release", str(v1), f"{repo}:1.0.0")

    # Change the content, re-release the SAME version → must refuse.
    (v1 / "scripts/run.sh").write_text("echo CHANGED\n", encoding="utf-8", newline="\n")
    result = runner.run("release", str(v1), f"{repo}:1.0.0", check=False)
    assert result.returncode == 65, (
        f"overwriting an existing version must exit 65, got "
        f"{result.returncode}; {result.stderr}"
    )

    # With --force the move is allowed.
    forced = runner.json("release", str(v1), f"{repo}:1.0.0", "--force")
    assert forced["pushed"] is True


def test_release_rule_file(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    rule = project_dir / "rust-style.md"
    _write(rule, "---\npaths: ['**/*.rs']\n---\n# Rust Style\nbody\n")
    repo = f"{registry}/{unique_repo}/rust-style"
    runner = grim_at(project_dir)

    out = runner.json("release", str(rule), f"{repo}:3.4.5")
    assert out["pushed"] is True
    assert out["tags"] == ["3.4.5", "3.4", "3", "latest"]

    # Install the released rule and assert the canonical file lands.
    write_config(project_dir, rules={"rust-style": f"{repo}:3.4.5"})
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert {r["status"] for r in rows} == {"installed"}
    assert (project_dir / ".claude/rules/rust-style.md").is_file()


def test_release_prerelease_is_exact_tag_only(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{repo}:1.2.3-rc.1", "--dry-run")
    assert out["tags"] == ["1.2.3-rc.1"], (
        "a prerelease must NOT cascade and must NOT move latest"
    )


def test_release_skip_existing_skips_published_version(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--skip-existing: an existing exact-version tag is a success no-op.

    The manifest-driven publisher pattern (grim publish):
    blanket re-runs must skip unbumped versions — even when local content
    changed — and push bumped ones.
    """
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    first = runner.json("release", str(skill), f"{repo}:1.0.0")
    assert first["pushed"] is True
    published = tag_digest(f"{unique_repo}/code-review", "1.0.0")

    # Change content, same version, --skip-existing → skipped, not error,
    # and the registry digest must NOT move.
    (skill / "scripts/run.sh").write_text("echo CHANGED\n", encoding="utf-8", newline="\n")
    skipped = runner.json("release", str(skill), f"{repo}:1.0.0", "--skip-existing")
    assert skipped["pushed"] is False
    assert skipped["tags"] == [], "a skipped release moves no tags"
    assert tag_digest(f"{unique_repo}/code-review", "1.0.0") == published

    # A bumped version with the same flag publishes normally.
    bumped = runner.json("release", str(skill), f"{repo}:1.0.1", "--skip-existing")
    assert bumped["pushed"] is True
    assert bumped["tags"] == ["1.0.1", "1.0", "1", "latest"]


def test_release_skip_existing_conflicts_with_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    result = runner.run(
        "release", str(skill), f"{repo}:1.0.0", "--skip-existing", "--force",
        check=False,
    )
    assert result.returncode != 0, "--skip-existing and --force must conflict"


def test_release_reserved_tag_is_usage_error_before_network(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim release <path> <repo>:__grimoire` targets grim's reserved companion
    tag. The reserved-namespace write guard refuses it as a usage error (64)
    before any network work, so a user can never overwrite or shadow a
    machine-owned companion tag."""
    skill = _local_skill(project_dir)
    repo = f"{registry}/{unique_repo}/code-review"
    runner = grim_at(project_dir)

    result = runner.run("release", str(skill), f"{repo}:__grimoire", check=False)
    assert result.returncode == 64, (
        f"a reserved-tag release must be a usage error (64), got {result.returncode}; stderr: {result.stderr}"
    )
    # Refused before the push: the reserved tag must stay absent on the registry.
    _assert_tag_absent(f"{unique_repo}/code-review", "__grimoire")


def test_release_push_registry_splits_push_and_baked_source(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Issue #39: `grim release --push-registry` pushes to the endpoint while
    every baked/reported name keeps the reference's registry (the pull
    name): the report `ref`, the source-annotation fallback. The pull host
    is the reserved-unresolvable `pull.invalid`, so any network call that
    mistakenly targets the pull name fails loudly."""
    skill = _local_skill(project_dir)
    pull_repo = f"{unique_repo}/code-review"
    pull_ref = f"pull.invalid/{pull_repo}:1.0.0"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), pull_ref, "--push-registry", registry)
    assert out["pushed"] is True
    assert out["ref"] == pull_ref, f"report ref must keep the pull name, got {out['ref']!r}"
    assert out["pushed_to"] == f"{registry}/{pull_repo}:1.0.0", (
        f"pushed_to must carry the push-side reference, got {out['pushed_to']!r}"
    )

    # The artifact (and its full semver cascade) is live on the push endpoint.
    manifest = fetch_manifest(pull_repo, "1.0.0")
    assert tag_digest(pull_repo, "latest") == out["manifest_digest"], (
        "the cascade tags must land on the push endpoint too"
    )
    annotations = manifest.get("annotations") or {}
    assert annotations.get("org.opencontainers.image.source") == f"pull.invalid/{pull_repo}", (
        "the source-annotation fallback must bake the PULL registry/repository, "
        f"got {annotations.get('org.opencontainers.image.source')!r}"
    )


def test_release_without_push_registry_reports_null_pushed_to(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Behavior lock: without `--push-registry` the release is unchanged and
    the additive `pushed_to` field is an explicit null."""
    skill = _local_skill(project_dir)
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)

    out = runner.json("release", str(skill), f"{registry}/{repo_path}:1.0.0")
    assert out["pushed"] is True
    assert "pushed_to" in out, "pushed_to must always be present (additive-field policy)"
    assert out["pushed_to"] is None
    assert tag_digest(repo_path, "1.0.0") == out["manifest_digest"]


# ── a backport never pulls a float below a newer release ─────────────────

_KEPT_ON_2_9_0 = [{"tag": "2", "version": "2.9.0"}, {"tag": "latest", "version": "2.9.0"}]


def _skill_files(version: str) -> dict[str, str]:
    return {
        "code-review/SKILL.md": (
            f"---\nname: code-review\ndescription: Review code.\n---\n# code-review {version}\n"
        )
    }


def test_backport_release_keeps_major_and_latest_on_the_newer_release(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)
    newer = release_with_grim(runner, repo_path, "2.9.0", _skill_files("2.9.0"))

    backport = release_with_grim(runner, repo_path, "2.3.7", _skill_files("2.3.7"))

    assert backport["pushed"] is True
    assert backport["tags"] == ["2.3.7", "2.3"], "kept floats are not in `tags`"
    assert backport["floats_kept"] == _KEPT_ON_2_9_0
    for tag in ("2.3.7", "2.3"):
        assert tag_digest(repo_path, tag) == backport["manifest_digest"], tag
    for tag in ("2.9.0", "2.9", "2", "latest"):
        assert tag_digest(repo_path, tag) == newer["manifest_digest"], tag


def test_backport_release_prints_one_stderr_line_per_kept_float(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)
    release_with_grim(runner, repo_path, "2.9.0", _skill_files("2.9.0"))

    result = runner.run("release", str(_local_skill(project_dir)), f"{registry}/{repo_path}:2.3.7")

    kept = [line for line in result.stderr.splitlines() if "above this release" in line]
    assert kept == [
        "kept '2' on 2.9.0, above this release 2.3.7",
        "kept 'latest' on 2.9.0, above this release 2.3.7",
    ], result.stderr
    assert "above this release" not in result.stdout, "the note stays off the result table"


def test_backport_dry_run_reports_the_kept_floats_and_moves_nothing(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)
    newer = release_with_grim(runner, repo_path, "2.9.0", _skill_files("2.9.0"))

    plan = release_with_grim(runner, repo_path, "2.3.7", _skill_files("2.3.7"), flags=["--dry-run"])

    assert plan["pushed"] is False
    assert plan["tags"] == ["2.3.7", "2.3"]
    assert plan["floats_kept"] == _KEPT_ON_2_9_0
    for tag in ("2.3.7", "2.3"):
        _assert_tag_absent(repo_path, tag)
    for tag in ("2", "latest"):
        assert tag_digest(repo_path, tag) == newer["manifest_digest"], tag


def test_offline_backport_dry_run_exits_0_and_plans_every_float_moving(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Offline no float can be read: a dry run keeps the exit 0 it always had,
    plans every float moving and warns once on stderr, while a release still
    refuses with 81 and moves no tag."""
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)
    newer = release_with_grim(runner, repo_path, "2.9.0", _skill_files("2.9.0"))
    skill = str(_local_skill(project_dir))
    reference = f"{registry}/{repo_path}:2.3.7"

    result = runner.run("--offline", "release", skill, reference, "--dry-run", format="json")

    plan = json.loads(result.stdout)
    assert plan["pushed"] is False
    assert plan["tags"] == ["2.3.7", "2.3", "2", "latest"]
    assert plan["floats_kept"] == []
    warnings = [line for line in result.stderr.splitlines() if "could not check the floats" in line]
    assert len(warnings) == 1, result.stderr

    refused = runner.run("--offline", "release", skill, reference, check=False)
    assert refused.returncode == 81, refused.stderr
    _assert_tag_absent(repo_path, "2.3.7")
    for tag in ("2", "latest"):
        assert tag_digest(repo_path, tag) == newer["manifest_digest"], tag


def test_a_float_pushed_onto_a_prerelease_moves_to_a_lower_release(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """``grim release`` never cascades a prerelease, so a float sits on one only
    when someone pushed it there by hand. The next release takes it whatever
    the precedence: ``latest`` leaves ``2.0.0-rc.1`` and ``1`` leaves
    ``1.9.1-rc.1`` for ``1.9.0``."""
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)
    major = publish_release(repo_path, "2.0.0-rc.1", _skill_files("2.0.0-rc.1"))
    patch = publish_release(repo_path, "1.9.1-rc.1", _skill_files("1.9.1-rc.1"))
    retag(repo_path, "latest", major.digest)
    retag(repo_path, "1", patch.digest)

    release = release_with_grim(runner, repo_path, "1.9.0", _skill_files("1.9.0"))

    assert release["floats_kept"] == []
    assert release["tags"] == ["1.9.0", "1.9", "1", "latest"]
    for tag in ("1.9.0", "1.9", "1", "latest"):
        assert tag_digest(repo_path, tag) == release["manifest_digest"], tag
    assert tag_digest(repo_path, "2.0.0-rc.1") == major.digest
    assert tag_digest(repo_path, "1.9.1-rc.1") == patch.digest


def test_a_release_that_moves_no_float_keeps_none(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`--skip-existing` on a published backport and `--no-cascade` both leave
    the floats alone without judging them, so neither reports one as kept."""
    repo_path = f"{unique_repo}/code-review"
    runner = grim_at(project_dir)
    newer = release_with_grim(runner, repo_path, "2.9.0", _skill_files("2.9.0"))
    release_with_grim(runner, repo_path, "2.3.7", _skill_files("2.3.7"))

    skipped = release_with_grim(
        runner, repo_path, "2.3.7", _skill_files("2.3.7"), flags=["--skip-existing"]
    )
    assert skipped["pushed"] is False
    assert skipped["tags"] == []
    assert skipped["floats_kept"] == []

    exact = release_with_grim(runner, repo_path, "2.3.8", _skill_files("2.3.8"), flags=["--no-cascade"])
    assert exact["pushed"] is True
    assert exact["tags"] == ["2.3.8"]
    assert exact["floats_kept"] == []
    assert tag_digest(repo_path, "2") == newer["manifest_digest"]
