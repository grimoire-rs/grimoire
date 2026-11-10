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

import json
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
    p.write_text(body, newline="\n", encoding="utf-8")


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
    (project_dir / "publish.toml").write_text(
        f'registry = "{registry}"\n{body}', newline="\n", encoding="utf-8"
    )


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
    assert "no description is published for" in miss.stderr


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
    assert "no description is published for" in miss.stderr


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
    assert "no description is published for" in miss.stderr


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
    (tmp_path / "outside.md").write_text(README, newline="\n", encoding="utf-8")
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
    (tmp_path / "secret.env").write_text("TOKEN=shh\n", newline="\n", encoding="utf-8")
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
    readme.write_text(README, newline="\n", encoding="utf-8")
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
    readme.write_text(README, newline="\n", encoding="utf-8")
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


# ---------------------------------------------------------------------------
# S7 — manifest-level `[support]` channels
# ---------------------------------------------------------------------------

ISSUES = "https://github.com/acme/skills/issues"
CHAT = "https://chat.example.invalid/acme"
CONTACT = "ai-platform@example.invalid"
SECURITY = "https://acme.example.invalid/security"


def test_manifest_support_fans_out_to_every_companion(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`[support]` is manifest-level, so it reaches EVERY companion the run
    pushes — including an entry that overrides `[description]` with its own
    table. That override replaces the file set wholesale; support is not part
    of the file set and must survive it."""
    prefix = unique_repo.split("/")[-1]
    a, b = f"{prefix}-sup-a", f"{prefix}-sup-b"
    _skill(project_dir, a)
    _skill(project_dir, b)
    _write(project_dir / "README.md", README)
    _write(project_dir / "own.md", "# Own readme\n")
    _manifest(
        project_dir,
        registry,
        "\n[support]\n"
        f'issues = "{ISSUES}"\n'
        f'chat = "{CHAT}"\n'
        f'contact = "{CONTACT}"\n'
        f'security = "{SECURITY}"\n'
        "\n[description]\n"
        'readme = "README.md"\n'
        f'\n[skills.{a}]\nversion = "0.1.0"\n'
        # b brings its own file set — support must NOT travel with it.
        f'\n[skills.{b}]\nversion = "0.1.0"\n'
        f'[skills.{b}.description]\nreadme = "own.md"\n',
    )

    runner = grim_at(project_dir)
    out = runner.json("publish")
    assert len(out["descriptions"]["items"]) == 2

    expected = {"issues": ISSUES, "chat": CHAT, "contact": CONTACT, "security": SECURITY}
    for name in (a, b):
        d = runner.json("describe", f"{registry}/skills/{name}:0.1.0")
        assert d["support"] == expected, f"{name} lost the manifest-wide support block: {d['support']}"

    # The per-entry table still replaced the FILES.
    assert runner.json("fetch", f"{registry}/skills/{b}:__grimoire")["content"] == "# Own readme\n"


def test_support_absent_yields_four_nulls(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """No `[support]` table ⇒ `describe` still reports all four keys, null.
    The consumer contract is always-present-null, never an absent object."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-nosup"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(project_dir, registry, f'\n[skills.{name}]\nversion = "0.1.0"\n')

    runner = grim_at(project_dir)
    runner.json("publish")
    d = runner.json("describe", f"{registry}/skills/{name}:0.1.0")
    assert d["support"] == {"issues": None, "chat": None, "contact": None, "security": None}


def test_partial_support_publishes_only_declared_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A `[support]` table declaring one key publishes one annotation — the
    other three read back null, not an empty string."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-partsup"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(
        project_dir,
        registry,
        f'\n[support]\nissues = "{ISSUES}"\n' f'\n[skills.{name}]\nversion = "0.1.0"\n',
    )

    runner = grim_at(project_dir)
    runner.json("publish")
    d = runner.json("describe", f"{registry}/skills/{name}:0.1.0")
    assert d["support"] == {"issues": ISSUES, "chat": None, "contact": None, "security": None}


# ---------------------------------------------------------------------------
# Package READMEs: each package's own README becomes its companion README
# ---------------------------------------------------------------------------


def _readme_of(kind: str, name: str) -> str:
    return f"# {kind} {name}\n\nThe package's own README.\n"


def _package_repo(project_dir: Path, prefix: str) -> dict[str, str]:
    """The issue's layout: a skill, a rule and an agent, each with its own
    README next to its source, plus a shared README, changelog and logo at the
    manifest root. Returns the entry names by kind."""
    names = {"skill": f"{prefix}-review", "rule": f"{prefix}-style", "agent": f"{prefix}-writer"}
    _skill(project_dir, names["skill"])
    _write(project_dir / "skills" / names["skill"] / "README.md", _readme_of("skill", names["skill"]))
    _write(project_dir / "rules" / f"{names['rule']}.md", "---\npaths: ['**/*.md']\n---\n# rule\n")
    _write(project_dir / "rules" / names["rule"] / "README.md", _readme_of("rule", names["rule"]))
    _write(
        project_dir / "agents" / f"{names['agent']}.md",
        f"---\nname: {names['agent']}\ndescription: A test agent for the desc suite.\n---\nYou help.\n",
    )
    _write(project_dir / "agents" / names["agent"] / "README.md", _readme_of("agent", names["agent"]))
    _write(project_dir / "README.md", README)
    _write(project_dir / "CHANGELOG.md", CHANGELOG)
    (project_dir / "logo.png").write_bytes(LOGO)
    return names


def _package_entries(names: dict[str, str]) -> str:
    return (
        f'\n[skills.{names["skill"]}]\nversion = "0.1.0"\n'
        f'\n[rules.{names["rule"]}]\nversion = "0.1.0"\n'
        f'\n[agents.{names["agent"]}]\nversion = "0.1.0"\n'
    )


def _repos(registry: str, names: dict[str, str]) -> dict[str, str]:
    return {
        "skill": f"{registry}/skills/{names['skill']}",
        "rule": f"{registry}/rules/{names['rule']}",
        "agent": f"{registry}/agents/{names['agent']}",
    }


def _companion_paths(runner, repo: str) -> set[str]:
    return {f["path"] for f in runner.json("fetch", f"{repo}:__grimoire")["files"]}


def test_package_readme_is_the_default_companion_readme(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """With no [description] table, each package's own README replaces the
    README slot of its companion while the changelog and logo still come from
    the manifest root — for a skill (directory), a rule and an agent (the path
    without `.md`)."""
    prefix = unique_repo.split("/")[-1]
    names = _package_repo(project_dir, prefix)
    _manifest(project_dir, registry, _package_entries(names))

    runner = grim_at(project_dir)
    out = runner.json("publish")
    for item in out["descriptions"]["items"]:
        assert set(item["files"]) == {"README.md", "CHANGELOG.md", "logo.png"}, item

    for kind, repo in _repos(registry, names).items():
        doc = runner.json("fetch", f"{repo}:__grimoire")
        assert doc["content"] == _readme_of(kind, names[kind]), f"{kind} companion README"
        assert _companion_paths(runner, repo) == {"README.md", "CHANGELOG.md", "logo.png"}
        changelog = runner.plain("fetch", f"{repo}:__grimoire", "--path", "CHANGELOG.md")
        assert changelog.stdout == CHANGELOG, f"{kind} changelog comes from the root"


def test_entry_readme_with_path_components_is_a_data_error_nothing_pushed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`entry_readme` names one file, not a path: `../x` exits 65 before any
    push, so the artifact repository stays absent."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-badname"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    _manifest(
        project_dir,
        registry,
        '\n[description]\nentry_readme = "../x"\n' f'\n[skills.{name}]\nversion = "1.0.0"\n',
    )

    result = grim_at(project_dir).run("publish", format="json", check=False)
    assert result.returncode == 65, f"got {result.returncode}; stderr: {result.stderr}"
    assert "entry_readme" in result.stderr
    _assert_tag_absent(f"skills/{name}", "1.0.0")


def test_partial_table_warns_and_entries_keep_their_own_readme(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The issue's partial table (`logo` only) next to a root README warns once,
    naming the manifest and the fix. Exit and JSON are unchanged: the entry
    without a README of its own ships the logo alone, the entry with one (here
    through an explicit `path`) ships it beside the logo."""
    prefix = unique_repo.split("/")[-1]
    alpha, beta = f"{prefix}-alpha", f"{prefix}-beta"
    _skill(project_dir, alpha)
    _write(
        project_dir / "plugins" / "p1" / "skills" / beta / "SKILL.md",
        f"---\nname: {beta}\ndescription: A test skill for the desc suite.\n---\n# {beta}\n",
    )
    beta_readme = _readme_of("skill", beta)
    _write(project_dir / "plugins" / "p1" / "skills" / beta / "README.md", beta_readme)
    _write(project_dir / "README.md", README)
    (project_dir / "logo.png").write_bytes(LOGO)
    _manifest(
        project_dir,
        registry,
        '\n[description]\nlogo = "logo.png"\n'
        f'\n[skills.{alpha}]\nversion = "0.1.0"\n'
        f'\n[skills.{beta}]\nversion = "0.1.0"\npath = "plugins/p1/skills/{beta}"\n',
    )

    runner = grim_at(project_dir)
    result = runner.run("publish", format="json")
    assert result.stderr.count("sets no readme") == 1, result.stderr
    assert "publish.toml" in result.stderr and 'readme = "README.md"' in result.stderr

    # A logo-only companion has no README index to fetch, so read the report.
    files = {i["repository"]: set(i["files"]) for i in json.loads(result.stdout)["descriptions"]["items"]}
    assert files[f"{registry}/skills/{alpha}"] == {"logo.png"}
    assert files[f"{registry}/skills/{beta}"] == {"README.md", "logo.png"}
    doc = runner.json("fetch", f"{registry}/skills/{beta}:__grimoire")
    assert doc["content"] == beta_readme


@pytest.mark.parametrize(
    "table",
    [
        'readme = "README.md"\nlogo = "logo.png"',
        'entry_readme = "README.md"',
        'publish = false',
    ],
    ids=["readme-named", "entry-readme-only", "kill-switch"],
)
def test_no_partial_table_warning_when_nothing_is_left_out(
    grim_at, project_dir: Path, registry: str, unique_repo: str, table: str
) -> None:
    """The warning stays quiet when the table names the README, sets only
    `entry_readme` (the root probe stays on), or switches the companion off."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-quiet"
    _skill(project_dir, name)
    _write(project_dir / "README.md", README)
    (project_dir / "logo.png").write_bytes(LOGO)
    _manifest(
        project_dir,
        registry,
        f"\n[description]\n{table}\n" f'\n[skills.{name}]\nversion = "0.1.0"\n',
    )

    result = grim_at(project_dir).run("publish", format="json")
    assert "sets no readme" not in result.stderr, result.stderr


def test_entry_readme_only_table_keeps_the_root_probe_on(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A table that sets only `entry_readme` is no partial table: no "resolves
    to no files" error, the root README/CHANGELOG are still probed for an entry
    without a README of its own, and a custom file name is honoured."""
    prefix = unique_repo.split("/")[-1]
    plain, custom = f"{prefix}-plain", f"{prefix}-custom"
    _skill(project_dir, plain)
    _skill(project_dir, custom)
    _write(project_dir / "skills" / custom / "ABOUT.md", "# about custom\n")
    _write(project_dir / "skills" / custom / "README.md", "# not this one\n")
    _write(project_dir / "README.md", README)
    _write(project_dir / "CHANGELOG.md", CHANGELOG)
    _manifest(
        project_dir,
        registry,
        '\n[description]\nentry_readme = "ABOUT.md"\n'
        f'\n[skills.{plain}]\nversion = "0.1.0"\n'
        f'\n[skills.{custom}]\nversion = "0.1.0"\n',
    )

    runner = grim_at(project_dir)
    runner.json("publish")
    plain_doc = runner.json("fetch", f"{registry}/skills/{plain}:__grimoire")
    assert plain_doc["content"] == README
    assert _companion_paths(runner, f"{registry}/skills/{plain}") == {"README.md", "CHANGELOG.md"}
    assert runner.json("fetch", f"{registry}/skills/{custom}:__grimoire")["content"] == "# about custom\n"


def test_entry_readme_false_and_per_entry_switches_win_over_package_readme(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`entry_readme = false` turns the lookup off; a per-entry table still
    replaces the companion wholesale and `description = false` still publishes
    none, even when the package has a README of its own."""
    prefix = unique_repo.split("/")[-1]
    off, table, none = f"{prefix}-off", f"{prefix}-table", f"{prefix}-none"
    for name in (off, table, none):
        _skill(project_dir, name)
        _write(project_dir / "skills" / name / "README.md", f"# own {name}\n")
    _write(project_dir / "README.md", README)
    _write(project_dir / "elsewhere.md", "# per-entry table\n")
    # Lookup off for the whole manifest first.
    _manifest(
        project_dir,
        registry,
        "\n[description]\nentry_readme = false\n" f'\n[skills.{off}]\nversion = "0.1.0"\n',
    )
    runner = grim_at(project_dir)
    runner.json("publish")
    assert runner.json("fetch", f"{registry}/skills/{off}:__grimoire")["content"] == README

    # Default lookup on; the two per-entry switches still win.
    _manifest(
        project_dir,
        registry,
        f'\n[skills.{table}]\nversion = "0.1.0"\n'
        f'[skills.{table}.description]\nreadme = "elsewhere.md"\n'
        f'\n[skills.{none}]\nversion = "0.1.0"\ndescription = false\n',
    )
    runner.json("publish")
    assert runner.json("fetch", f"{registry}/skills/{table}:__grimoire")["content"] == "# per-entry table\n"
    _assert_tag_absent(f"skills/{none}", "__grimoire")


def test_kill_switch_also_turns_package_readmes_off(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`publish = false` publishes no companion at all, package READMEs
    included."""
    prefix = unique_repo.split("/")[-1]
    name = f"{prefix}-killed"
    _skill(project_dir, name)
    _write(project_dir / "skills" / name / "README.md", "# own\n")
    _manifest(
        project_dir,
        registry,
        "\n[description]\npublish = false\n" f'\n[skills.{name}]\nversion = "0.1.0"\n',
    )

    out = grim_at(project_dir).json("publish")
    assert out["descriptions"]["items"] == []
    _assert_tag_absent(f"skills/{name}", "__grimoire")


def test_republish_at_unchanged_version_repoints_companions_to_package_readmes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A repository already published with the shared README republishes at the
    same versions after the lookup switches on: every artifact is skipped, no
    refusal, and each companion now carries its package README."""
    prefix = unique_repo.split("/")[-1]
    names = _package_repo(project_dir, prefix)
    # The old behaviour: the lookup off, so every companion carries the root README.
    _manifest(project_dir, registry, "\n[description]\nentry_readme = false\n" + _package_entries(names))
    runner = grim_at(project_dir)
    first = runner.json("publish")
    old_digests = {i["repository"]: i["digest"] for i in first["descriptions"]["items"]}
    for repo in _repos(registry, names).values():
        assert runner.json("fetch", f"{repo}:__grimoire")["content"] == README

    _manifest(project_dir, registry, _package_entries(names))
    result = runner.run("publish", format="json")
    second = json.loads(result.stdout)
    assert {r["status"] for r in second["items"]} == {"skipped"}, second["items"]
    for kind, repo in _repos(registry, names).items():
        assert runner.json("fetch", f"{repo}:__grimoire")["content"] == _readme_of(kind, names[kind])
    new_digests = {i["repository"]: i["digest"] for i in second["descriptions"]["items"]}
    assert set(new_digests) == set(old_digests)
    assert all(new_digests[r] != old_digests[r] for r in old_digests), "companions must be re-pointed"


def test_using_the_package_readme_as_companion_changes_no_artifact_digest(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The artifact bytes do not depend on whether its README is also used as
    the companion: the same sources published with the lookup off and then,
    with `--force` so nothing is skipped, with it on, give the skill, rule and
    agent one digest each, while the companion README does change."""
    prefix = unique_repo.split("/")[-1]
    names = _package_repo(project_dir, prefix)
    _manifest(project_dir, registry, "\n[description]\nentry_readme = false\n" + _package_entries(names))
    runner = grim_at(project_dir)
    off = runner.json("publish")["items"]
    off_companions = {
        kind: runner.json("fetch", f"{repo}:__grimoire")["content"] for kind, repo in _repos(registry, names).items()
    }

    _manifest(project_dir, registry, _package_entries(names))
    on = runner.json("publish", "--force")["items"]

    assert {r["status"] for r in off} == {"pushed"}, off
    assert {r["status"] for r in on} == {"pushed"}, "--force must push, or the digests below compare nothing"
    assert {r["ref"]: r["digest"] for r in on} == {r["ref"]: r["digest"] for r in off}
    for kind, repo in _repos(registry, names).items():
        used = runner.json("fetch", f"{repo}:__grimoire")["content"]
        assert used == _readme_of(kind, names[kind]) != off_companions[kind], kind


def _publish_with_unusable_readme(runner, project_dir: Path, registry: str, name: str):
    _manifest(project_dir, registry, f'\n[rules.{name}]\nversion = "0.1.0"\n')
    result = runner.run("publish", format="json")
    assert result.returncode == 0, result.stderr
    assert "its own README was not published" in result.stderr, result.stderr
    assert "the top-level README stays" in result.stderr, result.stderr
    assert "set `entry_readme = false`" in result.stderr, result.stderr
    assert runner.json("fetch", f"{registry}/rules/{name}:__grimoire")["content"] == README
    return result


def test_a_package_readme_that_is_not_a_regular_file_is_skipped_with_a_warning(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A directory where the package README should be: the publish still exits
    0, the warning says why and what to do, and the top-level README stays."""
    name = f"{unique_repo.split('/')[-1]}-style"
    _write(project_dir / "rules" / f"{name}.md", "---\npaths: ['**/*.md']\n---\n# rule\n")
    _write(project_dir / "rules" / name / "README.md" / "inner.md", "# not a file\n")
    _write(project_dir / "README.md", README)

    result = _publish_with_unusable_readme(grim_at(project_dir), project_dir, registry, name)

    assert "is not a regular file" in result.stderr, result.stderr


@pytest.mark.skipif(sys.platform == "win32", reason="creating a symlink needs privileges on Windows")
def test_a_package_readme_symlinked_out_of_the_tree_is_skipped_with_a_warning(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A package README that links outside the manifest directory is never
    read: the warning names the containment cause once and the top-level
    README stays."""
    name = f"{unique_repo.split('/')[-1]}-style"
    _write(project_dir / "rules" / f"{name}.md", "---\npaths: ['**/*.md']\n---\n# rule\n")
    outside = project_dir.parent / f"{name}-secret.md"
    _write(outside, "# outside the tree\n")
    (project_dir / "rules" / name).mkdir(parents=True)
    os.symlink(outside, project_dir / "rules" / name / "README.md")
    _write(project_dir / "README.md", README)

    result = _publish_with_unusable_readme(grim_at(project_dir), project_dir, registry, name)

    assert "not safely contained" in result.stderr, result.stderr
    assert result.stderr.count(f"rules/{name}/README.md") == 1, result.stderr
