# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Repository description companion acceptance tests (publish.toml-driven).

A description companion (README + optional logo/changelog/assets) rides the
`grim publish` batch: after each entry's artifact is pushed, its repository's
reserved ``__grimoire`` tag is (re)pointed at a tar of the repo's descriptive
files. The companion is read back through the normal `grim fetch` path — uniform
for every artifact kind — and the internal tag never leaks into user-facing tag
listings.

The companion source is resolved per entry: an explicit ``[description]`` table
(top-level fan-out or a per-entry override) wins over a conventional probe of the
manifest directory; ``description = false`` opts an entry out; ``publish = false``
is the manifest-wide kill switch.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path
from urllib.error import HTTPError

import pytest

from src.registry import tag_digest

README = "# Repo\n\nWhat this repository ships and how to use it.\n"
CHANGELOG = "# Changelog\n\n## 1.0.0\n\n- first release\n"
# A minimal non-UTF-8 asset (PNG signature + a 0xFF byte) rides the layer too.
LOGO = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF])


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _assert_tag_absent(repo_path: str, tag: str) -> None:
    """Assert the registry has no manifest for ``repo_path:tag`` — used to
    prove a failed/refused publish left ZERO registry mutations behind."""
    try:
        tag_digest(repo_path, tag)
    except HTTPError as exc:
        assert exc.code == 404, f"expected a 404 for {repo_path}:{tag}, got {exc.code}"
        return
    raise AssertionError(f"{repo_path}:{tag} unexpectedly resolved — an artifact was pushed")


def _skill(project_dir: Path, name: str) -> None:
    """Write a minimal valid skill source under skills/<name>/."""
    _write(
        project_dir / "skills" / name / "SKILL.md",
        f"---\nname: {name}\ndescription: A test skill for the desc suite.\n---\n# {name}\n",
    )


def _manifest(project_dir: Path, registry: str, body: str) -> None:
    """Write a publish.toml whose top line is the required `registry` field."""
    (project_dir / "publish.toml").write_text(f'registry = "{registry}"\n{body}')


def test_conventional_probe_round_trip(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """With no [description] table, grim probes the manifest directory for
    conventional files and auto-publishes a companion, read back via fetch."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-probe"
    _skill(project_dir, name)
    # Conventional files at the manifest root — probed automatically.
    _write(project_dir / "README.md", README)
    _write(project_dir / "CHANGELOG.md", CHANGELOG)
    (project_dir / "logo.png").write_bytes(LOGO)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "0.1.0"\n')

    runner = grim_at(project_dir)
    out = runner.json("publish")
    descs = out["descriptions"]["items"]
    assert len(descs) == 1, f"one companion expected, got {descs}"
    assert descs[0]["ref"].endswith(":__grimoire")
    assert descs[0]["repository"] == f"{registry}/skills/{name}"
    assert descs[0]["digest"].startswith("sha256:")
    assert set(descs[0]["files"]) == {"README.md", "CHANGELOG.md", "logo.png"}

    repo = f"{registry}/skills/{name}"
    doc = runner.json("fetch", f"{repo}:__grimoire")
    assert doc["kind"] == "desc"
    paths = [f["path"] for f in doc["files"]]
    assert {"README.md", "CHANGELOG.md", "logo.png"} <= set(paths)
    assert doc["content"] == README  # README.md is the default index

    changelog = runner.plain("fetch", f"{repo}:__grimoire", "--path", "CHANGELOG.md")
    assert changelog.returncode == 0, changelog.stderr
    assert changelog.stdout == CHANGELOG


def test_explicit_mapping_source_name_differs_from_packed_name(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An explicit [description] maps arbitrary source paths onto the well-known
    wire names: docs/readme.md → README.md, brand/icon.svg → logo.svg."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-explicit"
    _skill(project_dir, name)
    _write(project_dir / "docs" / "readme.md", README)
    _write(project_dir / "brand" / "icon.svg", "<svg/>\n")
    _manifest(
        project_dir,
        registry,
        "\n[description]\n"
        'readme = "docs/readme.md"\n'
        'logo = "brand/icon.svg"\n'
        f'\n[skills.{name}]\nversion = "0.1.0"\n',
    )

    runner = grim_at(project_dir)
    out = runner.json("publish")
    files = out["descriptions"]["items"][0]["files"]
    assert set(files) == {"README.md", "logo.svg"}, f"source names map to wire names, got {files}"

    doc = runner.json("fetch", f"{registry}/skills/{name}:__grimoire")
    assert doc["content"] == README
    logo = runner.plain("fetch", f"{registry}/skills/{name}:__grimoire", "--path", "logo.svg")
    assert logo.returncode == 0, logo.stderr
    assert logo.stdout == "<svg/>\n"


def test_per_entry_override_and_false_optout(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A per-entry [<kind>.<name>.description] overrides the top-level fan-out;
    `description = false` opts an entry out entirely."""
    prefix = unique_repo.split("/")[-1]
    a, b = f"{prefix}-a", f"{prefix}-b"
    _skill(project_dir, a)
    _skill(project_dir, b)
    _write(project_dir / "README.md", README)  # top-level fan-out source
    _write(project_dir / "skills" / a / "OWN.md", "# entry-specific readme\n")
    _manifest(
        project_dir,
        registry,
        "\n[description]\n"
        'readme = "README.md"\n'
        f"\n[skills.{a}]\nversion = \"0.1.0\"\n"
        f'[skills.{a}.description]\nreadme = "skills/{a}/OWN.md"\n'
        f"\n[skills.{b}]\nversion = \"0.1.0\"\ndescription = false\n",
    )

    runner = grim_at(project_dir)
    out = runner.json("publish")
    repos = {d["repository"] for d in out["descriptions"]["items"]}
    assert repos == {f"{registry}/skills/{a}"}, f"only {a} gets a companion, got {repos}"

    # a's companion carries the per-entry override, not the top-level README.
    doc_a = runner.json("fetch", f"{registry}/skills/{a}:__grimoire")
    assert doc_a["content"] == "# entry-specific readme\n"

    # b opted out: no companion published, fetch is a clean not-found.
    miss = runner.plain("fetch", f"{registry}/skills/{b}:__grimoire", check=False)
    assert miss.returncode != 0
    assert "not found" in miss.stderr.lower()


def test_fanout_same_companion_on_every_repo(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A top-level [description] fans out to every entry's repository with
    byte-identical content ⇒ identical manifest digest on each repo."""
    prefix = unique_repo.split("/")[-1]
    a, b = f"{prefix}-fa", f"{prefix}-fb"
    _skill(project_dir, a)
    _skill(project_dir, b)
    _write(project_dir / "README.md", README)
    _manifest(
        project_dir,
        registry,
        "\n[description]\n"
        'readme = "README.md"\n'
        f"\n[skills.{a}]\nversion = \"0.1.0\"\n"
        f"\n[skills.{b}]\nversion = \"0.1.0\"\n",
    )

    runner = grim_at(project_dir)
    out = runner.json("publish")
    descs = {d["repository"]: d for d in out["descriptions"]["items"]}
    assert set(descs) == {f"{registry}/skills/{a}", f"{registry}/skills/{b}"}
    digests = {d["digest"] for d in descs.values()}
    assert len(digests) == 1, f"identical companion content ⇒ one digest, got {digests}"

    for repo in (a, b):
        doc = runner.json("fetch", f"{registry}/skills/{repo}:__grimoire")
        assert doc["content"] == README


def test_republish_unchanged_is_identical_digest(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Deterministic packing ⇒ an unchanged republish is a CAS no-op: identical
    companion manifest digest across runs."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-cas"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "0.1.0"\n')

    runner = grim_at(project_dir)
    first = runner.json("publish")["descriptions"]["items"][0]["digest"]
    second = runner.json("publish")["descriptions"]["items"][0]["digest"]
    assert first == second, "unchanged republish must yield the same companion digest"


def test_dry_run_previews_companion_and_pushes_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """--dry-run lists the planned companion (digest null) but pushes nothing:
    the reserved tag stays absent on the registry."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-dry"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "0.1.0"\n')

    runner = grim_at(project_dir)
    out = runner.json("publish", "--dry-run")
    descs = out["descriptions"]["items"]
    assert len(descs) == 1
    assert descs[0]["repository"] == f"{registry}/skills/{name}"
    assert descs[0]["digest"] is None, "dry-run pushes nothing, so no digest"
    assert set(descs[0]["files"]) == {"README.md"}

    # Nothing was pushed — the companion tag must not resolve.
    miss = runner.plain("fetch", f"{registry}/skills/{name}:__grimoire", check=False)
    assert miss.returncode != 0
    assert "not found" in miss.stderr.lower()


def test_empty_resolution_publishes_no_companion(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """No [description] table and no conventional files ⇒ no companion, and the
    publish still succeeds (an absent companion is not an error)."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-empty"
    _skill(project_dir, name)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "0.1.0"\n')

    runner = grim_at(project_dir)
    result = runner.run("publish", format="json", check=False)
    assert result.returncode == 0, result.stderr
    import json

    assert json.loads(result.stdout)["descriptions"]["items"] == [], "no companion resolved"

    miss = runner.plain("fetch", f"{registry}/skills/{name}:__grimoire", check=False)
    assert miss.returncode != 0
    assert "not found" in miss.stderr.lower()


def test_companion_tag_hidden_from_describe_tags(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The `__grimoire` companion tag must not leak into describe `tags[]`."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-hidden"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "1.0.0"\n')

    runner = grim_at(project_dir)
    runner.json("publish")

    d = runner.json("describe", f"{registry}/skills/{name}:1.0.0")
    assert "__grimoire" not in d["tags"], f"internal tag leaked into describe tags[]: {d['tags']}"
    assert "1.0.0" in d["tags"]


# ---------------------------------------------------------------------------
# B1 — path containment: a companion source must resolve inside the manifest dir
# ---------------------------------------------------------------------------


def test_readme_escaping_manifest_dir_is_data_error_nothing_pushed(
    grim_at, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """An explicit `[description]` readme that escapes the manifest directory
    (`../outside.md`, resolving to a real file OUTSIDE the tree) is a data error
    (65) surfaced before any push — the containment guard runs in the plan
    phase, so the artifact repository must stay absent on the registry."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-escape"
    _skill(project_dir, name)
    # A README that exists OUTSIDE the manifest dir (the parent of project_dir).
    (tmp_path / "outside.md").write_text(README)
    _manifest(
        project_dir,
        registry,
        '\n[description]\nreadme = "../outside.md"\n' f'\n[skills.{name}]\nversion = "1.0.0"\n',
    )

    runner = grim_at(project_dir)
    result = runner.run("publish", format="json", check=False)
    assert result.returncode == 65, (
        f"an escaping readme must be a data error (65), got {result.returncode}; stderr: {result.stderr}"
    )
    # Containment fails before the entry push loop → nothing reached the registry.
    _assert_tag_absent(f"skills/{name}", "1.0.0")


def test_readme_with_leading_dot_slash_publishes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Regression for issue #36: a `[description]` readme written with a
    leading `./` (idiomatic in hand-written manifests, join-neutral) must not
    be rejected as out of bounds — the publish succeeds and the companion is
    pushed."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-dotslash"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(
        project_dir,
        registry,
        '\n[description]\nreadme = "./README.md"\n' f'\n[skills.{name}]\nversion = "1.0.0"\n',
    )

    runner = grim_at(project_dir)
    out = runner.json("publish")
    descs = out["descriptions"]["items"]
    assert len(descs) == 1, f"one companion expected, got {descs}"
    assert descs[0]["repository"] == f"{registry}/skills/{name}"
    assert descs[0]["files"] == ["README.md"]


def test_include_glob_escaping_manifest_dir_is_data_error(
    grim_at, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """An `include` glob that walks out of the manifest tree (`../**/*.env`)
    must be a containment data error (65), never a silent pack of an
    out-of-tree file that happened to match."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-glob-escape"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)  # a valid in-tree source
    # A secret OUTSIDE the manifest dir, reachable via `../**/*.env`.
    (tmp_path / "secret.env").write_text("TOKEN=shh\n")
    _manifest(
        project_dir,
        registry,
        '\n[description]\nreadme = "README.md"\ninclude = ["../**/*.env"]\n'
        f'\n[skills.{name}]\nversion = "1.0.0"\n',
    )

    runner = grim_at(project_dir)
    result = runner.run("publish", format="json", check=False)
    assert result.returncode == 65, (
        f"an escaping include glob must be a data error (65), got {result.returncode}; stderr: {result.stderr}"
    )
    _assert_tag_absent(f"skills/{name}", "1.0.0")


# ---------------------------------------------------------------------------
# W2 — pre-pack companions: a bad companion aborts with zero registry mutations
# ---------------------------------------------------------------------------


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX unreadable-file mode")
def test_unreadable_companion_aborts_with_zero_registry_mutations(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A companion whose source passes plan-time validation (`is_file()`) but
    cannot be READ at pack time must abort the publish with ZERO registry
    mutations: every companion is read + packed BEFORE the first entry push, so
    no half-published batch (a live artifact + a failed companion) is left."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-badpack"
    _skill(project_dir, name)
    readme = project_dir / "README.md"
    readme.write_text(README)
    # Readable as a file (stat/`is_file()` still succeed at plan time) but the
    # pack-time read fails — the exact window the pre-pack step must close.
    os.chmod(readme, 0o000)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "1.0.0"\n')

    runner = grim_at(project_dir)
    try:
        result = runner.run("publish", format="json", check=False)
    finally:
        os.chmod(readme, 0o644)  # restore so tmp cleanup can remove the tree

    assert result.returncode != 0, (
        f"an unreadable companion must fail the publish, got {result.returncode}; stderr: {result.stderr}"
    )
    # Zero registry mutations: the entry artifact must NOT have been pushed.
    _assert_tag_absent(f"skills/{name}", "1.0.0")


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX unreadable-file mode")
def test_unreadable_companion_aborts_dry_run(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The companion pre-pack runs even under `--dry-run` (validation parity):
    a companion that passes plan-time `is_file()` but cannot be READ at pack
    time fails the dry-run non-zero, before any push. This pins the "a dry-run
    packs too" contract — a preview that skipped packing would let a broken
    companion slip through to a later real publish undetected. Sibling of
    test_unreadable_companion_aborts_with_zero_registry_mutations."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-badpack-dryrun"
    _skill(project_dir, name)
    readme = project_dir / "README.md"
    readme.write_text(README)
    # Readable as a file at plan time (`is_file()` succeeds), unreadable at pack
    # time — the exact window the dry-run pre-pack step must also close.
    os.chmod(readme, 0o000)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "1.0.0"\n')

    runner = grim_at(project_dir)
    try:
        result = runner.run("publish", "--dry-run", format="json", check=False)
    finally:
        os.chmod(readme, 0o644)  # restore so tmp cleanup can remove the tree

    assert result.returncode != 0, (
        f"an unreadable companion must fail even a --dry-run publish (pack "
        f"parity), got {result.returncode}; stderr: {result.stderr}"
    )
    # A dry-run never pushes, but the failure must leave nothing behind either.
    _assert_tag_absent(f"skills/{name}", "1.0.0")


# ---------------------------------------------------------------------------
# S6 — the top-level `publish = false` kill switch
# ---------------------------------------------------------------------------


def test_publish_false_kill_switch_yields_no_companion(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A top-level `[description] publish = false` disables the auto-companion
    for the whole manifest: even with a conventional README present, no
    companion is published (`descriptions.items == []`) and publish succeeds."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-killswitch"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)  # present, but disabled below
    _manifest(
        project_dir,
        registry,
        "\n[description]\npublish = false\n" f'\n[skills.{name}]\nversion = "0.1.0"\n',
    )

    runner = grim_at(project_dir)
    out = runner.json("publish")
    assert out["descriptions"]["items"] == [], (
        f"publish = false disables the companion, got {out['descriptions']['items']}"
    )
    # No companion tag on the registry either.
    _assert_tag_absent(f"skills/{name}", "__grimoire")
