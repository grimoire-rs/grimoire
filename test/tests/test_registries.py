# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Multi-registry acceptance tests (`[[registries]]` config table).

These tests exercise three distinct behaviors introduced by the multi-registry
feature (ADR: adr_multi_registry_mcp.md):

1. ``grim search`` (no --registry flag) browses ALL declared ``[[registries]]``
   entries in the project config.
2. ``grim add alias/repo:tag`` expands a qualified alias reference against the
   configured registry URL and persists the fully-qualified name.
3. A project using only the legacy ``[options].default_registry`` (no
   ``[[registries]]``) still works correctly on a cold cache — backward
   compatibility guard.

Registry simulation strategy
-----------------------------
The acceptance suite runs against a single shared ``localhost:5000`` registry.
To simulate two independent registries we use two DISTINCT NAMESPACE prefixes
on the same host and declare them as two ``[[registries]]`` entries:

    [[registries]]
    alias = "ns1"
    oci = "localhost:5000/<namespace1>"

    [[registries]]
    alias = "ns2"
    oci = "localhost:5000/<namespace2>"

This mirrors real multi-registry usage (namespaced orgs on ghcr.io, etc.) and
is the recommended pattern in ``test_search_namespaced.py``.
"""
from __future__ import annotations

import json
import subprocess
import uuid
from pathlib import Path

import pytest

from src.assertions import assert_dir_exists
from src.helpers import make_artifact
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _two_namespace_config(project_dir: Path, ns1: str, ns2: str) -> None:
    """Write a grimoire.toml with two ``[[registries]]`` entries (two namespaces)."""
    text = (
        f'[[registries]]\n'
        f'alias = "reg1"\n'
        f'oci = "{REGISTRY_HOST}/{ns1}"\n'
        f'default = true\n'
        f'\n'
        f'[[registries]]\n'
        f'alias = "reg2"\n'
        f'oci = "{REGISTRY_HOST}/{ns2}"\n'
        f'\n'
        f'[skills]\n'
        f'\n'
        f'[rules]\n'
    )
    (project_dir / "grimoire.toml").write_text(text)


# ---------------------------------------------------------------------------
# Test 1 — multi-registry default browse
# ---------------------------------------------------------------------------


def test_search_multi_registry_browses_all_declared(
    grim_at, project_dir: Path, registry: str
) -> None:
    """``grim search`` (no --registry flag) must browse ALL declared
    ``[[registries]]`` entries in grimoire.toml and surface packages from each.

    Implementation note: the shared localhost:5000 registry distinguishes
    registries by namespace prefix. We declare two namespaces as two
    ``[[registries]]`` entries, publish one artifact in each, then assert both
    appear in the search results.
    """
    # Use distinct unique segments for each namespace to avoid cross-test
    # collisions on the shared session-scoped registry.
    ns1 = f"grim-test/{uuid.uuid4().hex[:12]}"
    ns2 = f"grim-test/{uuid.uuid4().hex[:12]}"

    # Publish one artifact under each namespace.
    make_artifact(
        f"{ns1}/skill-in-ns1",
        "skill",
        {"skill-in-ns1/SKILL.md": "---\nname: skill-in-ns1\ndescription: from ns1\n---\n# S1\n"},
        tag="latest",
        annotations={
            "org.opencontainers.image.description": "Skill from namespace 1",
        },
    )
    make_artifact(
        f"{ns2}/rule-in-ns2",
        "rule",
        {"rule-in-ns2.md": "---\npaths: ['**/*.rs']\n---\n# R2\n"},
        tag="latest",
        annotations={
            "org.opencontainers.image.description": "Rule from namespace 2",
        },
    )

    _two_namespace_config(project_dir, ns1, ns2)
    runner = grim_at(project_dir)

    # Run grim search WITHOUT --registry so it uses the declared [[registries]].
    # --refresh forces a catalog rebuild from both registries.
    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, (
        f"multi-registry search must exit 0, got {result.returncode}; "
        f"stderr: {result.stderr}"
    )
    rows = json.loads(result.stdout)["items"]
    assert isinstance(rows, list), f"search must return a JSON array, got {rows!r}"

    repos = [r.get("repo", "") for r in rows]

    assert any("skill-in-ns1" in repo for repo in repos), (
        f"search must surface the artifact from namespace 1 (reg1), "
        f"but got repos: {repos}"
    )
    assert any("rule-in-ns2" in repo for repo in repos), (
        f"search must surface the artifact from namespace 2 (reg2), "
        f"but got repos: {repos}"
    )


@pytest.mark.parametrize("style", ["comma", "repeat"])
def test_search_multi_registry_flag_browses_all(
    grim_at, project_dir: Path, registry: str, style: str
) -> None:
    """``--registry`` accepts several registries — comma-separated
    (``--registry a,b``) or repeated (``--registry a --registry b``) — and
    browses all of them at once, overriding any configured ``[[registries]]``.

    Two namespaces simulate two registries (same pattern as the config test).
    The project config declares only the FIRST namespace, so if the flag were
    single-valued the second namespace's artifact would be missing — the test
    proves the flag spans both.
    """
    ns1 = f"grim-test/{uuid.uuid4().hex[:12]}"
    ns2 = f"grim-test/{uuid.uuid4().hex[:12]}"

    make_artifact(
        f"{ns1}/flag-skill-ns1",
        "skill",
        {"flag-skill-ns1/SKILL.md": "---\nname: flag-skill-ns1\ndescription: from ns1\n---\n# S1\n"},
        tag="latest",
        annotations={"org.opencontainers.image.description": "Flag skill ns1"},
    )
    make_artifact(
        f"{ns2}/flag-rule-ns2",
        "rule",
        {"flag-rule-ns2.md": "---\npaths: ['**/*.rs']\n---\n# R2\n"},
        tag="latest",
        annotations={"org.opencontainers.image.description": "Flag rule ns2"},
    )

    # Config declares only ns1; the flag must override and span both.
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\noci = "{REGISTRY_HOST}/{ns1}"\ndefault = true\n\n[skills]\n\n[rules]\n'
    )
    runner = grim_at(project_dir)

    reg1 = f"{REGISTRY_HOST}/{ns1}"
    reg2 = f"{REGISTRY_HOST}/{ns2}"
    if style == "comma":
        flag_args = ["--registry", f"{reg1},{reg2}"]
    else:
        flag_args = ["--registry", reg1, "--registry", reg2]

    result = runner.run("--format", "json", "search", *flag_args, "--refresh", check=False)
    assert result.returncode == 0, (
        f"multi-registry --registry ({style}) must exit 0, got {result.returncode}; "
        f"stderr: {result.stderr}"
    )
    rows = json.loads(result.stdout)["items"]
    repos = [r.get("repo", "") for r in rows]

    assert any("flag-skill-ns1" in repo for repo in repos), (
        f"--registry ({style}) must browse the first registry, got repos: {repos}"
    )
    assert any("flag-rule-ns2" in repo for repo in repos), (
        f"--registry ({style}) must browse the second registry too, got repos: {repos}"
    )


# ---------------------------------------------------------------------------
# Test 2 — qualified alias reference resolves via [[registries]] alias
# ---------------------------------------------------------------------------


def test_add_qualified_alias_reference_resolves(
    grim_at, project_dir: Path, registry: str
) -> None:
    """``grim add alias/repo:tag`` resolves the alias to its configured URL.

    With a ``[[registries]]`` entry ``alias="reg1", url="localhost:5000/<ns>"``,
    the short form ``reg1/<repo>:<tag>`` must expand to
    ``localhost:5000/<ns>/<repo>:<tag>`` and that fully-qualified name must
    appear in both grimoire.toml and grimoire.lock.
    """
    ns1 = f"grim-test/{uuid.uuid4().hex[:12]}"

    art = make_artifact(
        f"{ns1}/my-tool",
        "skill",
        {"my-tool/SKILL.md": "---\nname: my-tool\ndescription: d\n---\n# T\n"},
        tag="v1",
    )

    # Config declares one registry with alias "reg1".
    cfg_text = (
        f'[[registries]]\n'
        f'alias = "reg1"\n'
        f'oci = "{REGISTRY_HOST}/{ns1}"\n'
        f'default = true\n'
        f'\n'
        f'[skills]\n'
        f'\n'
        f'[rules]\n'
    )
    (project_dir / "grimoire.toml").write_text(cfg_text)
    runner = grim_at(project_dir)

    # Use the qualified alias/repo:tag form — the leading segment "reg1" is
    # the alias; grim must substitute the configured URL.
    qualified_ref = f"reg1/my-tool:v1"
    out = runner.json("add", qualified_ref)

    assert out["status"] == "added", f"add must report 'added', got {out!r}"
    assert out["kind"] == "skill", f"kind must be 'skill', got {out!r}"

    # The alias must be EXPANDED to the full path, not persisted: the
    # qualified form `reg1/my-tool` must appear nowhere in the config (the
    # `alias = "reg1"` declaration line is unaffected — it has no `/my-tool`),
    # and the full expanded path must be present.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "reg1/my-tool" not in cfg, (
        f"the alias-qualified form 'reg1/my-tool' must be expanded, not stored; got:\n{cfg}"
    )
    assert f"{REGISTRY_HOST}/{ns1}/my-tool" in cfg, (
        f"grimoire.toml must carry the expanded path "
        f"'{REGISTRY_HOST}/{ns1}/my-tool'; got:\n{cfg}"
    )

    lock = (project_dir / "grimoire.lock").read_text()
    assert REGISTRY_HOST in lock, (
        f"grimoire.lock must contain the registry host '{REGISTRY_HOST}'; "
        f"got:\n{lock}"
    )
    assert f"{REGISTRY_HOST}/{ns1}/my-tool" in lock, (
        f"lock must carry the full expanded path '{REGISTRY_HOST}/{ns1}/my-tool'; "
        f"got:\n{lock}"
    )


# ---------------------------------------------------------------------------
# Test 3 — legacy single default_registry cold-cache backward compat
# ---------------------------------------------------------------------------


def test_search_single_default_registry_cold_cache(
    grim_at, project_dir: Path, registry: str
) -> None:
    """A project using only ``[options].default_registry`` (no ``[[registries]]``)
    must still work on a cold cache.

    This guards the legacy path: when no ``[[registries]]`` is declared,
    grim falls back to the single-registry resolve chain (project config
    ``[options].default_registry`` > GRIM_DEFAULT_REGISTRY > built-in
    fallback). The test uses a fresh per-test GRIM_HOME (cold cache) and
    asserts exit 0 + valid JSON array — the same behavioral contract as
    existing search tests.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    make_artifact(
        f"{ns}/legacy-skill",
        "skill",
        {"legacy-skill/SKILL.md": "---\nname: legacy-skill\ndescription: legacy\n---\n# L\n"},
        tag="latest",
        annotations={
            "org.opencontainers.image.description": "Legacy single-registry skill",
        },
    )

    # Legacy config: no [[registries]], only [options].default_registry.
    cfg_text = (
        f'[options]\n'
        f'default_registry = "{REGISTRY_HOST}/{ns}"\n'
        f'\n'
        f'[skills]\n'
        f'\n'
        f'[rules]\n'
    )
    (project_dir / "grimoire.toml").write_text(cfg_text)

    runner = grim_at(project_dir)
    # grim_home is from tmp_path so this is always a cold cache.
    # Do NOT set GRIM_DEFAULT_REGISTRY — use only the config default.
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    # --refresh forces a real registry walk even from a cold cache.
    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, (
        f"legacy single-registry search must exit 0 on cold cache, "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    arr = json.loads(result.stdout)["items"]
    assert isinstance(arr, list), f"search must return a JSON array, got {arr!r}"

    # The scoped namespace was used as the default_registry, so the skill
    # published there must appear in results.
    repos = [r.get("repo", "") for r in arr]
    assert any("legacy-skill" in repo for repo in repos), (
        f"cold-cache search with legacy default_registry must find 'legacy-skill', "
        f"got repos: {repos}"
    )


# ---------------------------------------------------------------------------
# Test 4 — partial failure: one unreachable registry degrades gracefully
# ---------------------------------------------------------------------------


def test_search_partial_registry_failure_degrades_to_reachable(
    grim_at, project_dir: Path, registry: str
) -> None:
    """One unreachable ``[[registries]]`` entry must not fail the whole browse.

    ``grim search`` fans out one task per declared registry and catches a
    per-registry failure (degrading it to an empty group) rather than
    propagating it. With two registries declared — one reachable, one pointing
    at a dead port — the command must:

    - exit 0 (the per-registry failure never becomes the process exit code)
    - still surface the reachable registry's artifact

    The unreachable entry uses ``localhost:9999`` (nothing listening), which
    refuses the connection immediately, so the test stays fast and hermetic —
    that namespace is never published to the shared registry.
    """
    ns_good = f"grim-test/{uuid.uuid4().hex[:12]}"

    cfg_text = (
        f'[[registries]]\n'
        f'alias = "good"\n'
        f'oci = "{REGISTRY_HOST}/{ns_good}"\n'
        f'default = true\n'
        f'\n'
        f'[[registries]]\n'
        f'alias = "bad"\n'
        f'oci = "localhost:9999/grim-test/unreachable"\n'
        f'\n'
        f'[skills]\n'
        f'\n'
        f'[rules]\n'
    )
    (project_dir / "grimoire.toml").write_text(cfg_text)
    runner = grim_at(project_dir)

    make_artifact(
        f"{ns_good}/reachable-skill",
        "skill",
        {"reachable-skill/SKILL.md": "---\nname: reachable-skill\ndescription: works\n---\n# OK\n"},
        tag="latest",
        annotations={"org.opencontainers.image.description": "Reachable artifact"},
    )

    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, (
        f"search with one unreachable registry must still exit 0, "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    rows = json.loads(result.stdout)["items"]
    assert isinstance(rows, list), f"search must return a JSON array, got {rows!r}"

    repos = [r.get("repo", "") for r in rows]
    assert any("reachable-skill" in repo for repo in repos), (
        f"search must surface the reachable registry's artifact despite the "
        f"unreachable one, got repos: {repos}"
    )


# ---------------------------------------------------------------------------
# Test 5 — no dedup: the same repo in two registries surfaces twice
# ---------------------------------------------------------------------------


def test_search_same_repo_in_two_registries_is_not_deduped(
    grim_at, project_dir: Path, registry: str
) -> None:
    """The same repo name in two registries surfaces as two distinct rows.

    The catalog is registry-grouped and flattened by fully-qualified
    ``registry/repository`` reference with no dedup or precedence step, so a
    repository published under the SAME bare name in two declared registries
    must appear TWICE — once per registry — never collapsed to one winner.
    This pins the "browse all, disambiguate by registry" contract: a future
    accidental dedup-by-bare-name would silently hide a registry's copy.
    """
    ns1 = f"grim-test/{uuid.uuid4().hex[:12]}"
    ns2 = f"grim-test/{uuid.uuid4().hex[:12]}"
    shared = "shared-tool"

    make_artifact(
        f"{ns1}/{shared}",
        "skill",
        {f"{shared}/SKILL.md": f"---\nname: {shared}\ndescription: from reg1\n---\n# R1\n"},
        tag="latest",
        annotations={"org.opencontainers.image.description": "Shared tool, registry 1"},
    )
    make_artifact(
        f"{ns2}/{shared}",
        "skill",
        {f"{shared}/SKILL.md": f"---\nname: {shared}\ndescription: from reg2\n---\n# R2\n"},
        tag="latest",
        annotations={"org.opencontainers.image.description": "Shared tool, registry 2"},
    )

    _two_namespace_config(project_dir, ns1, ns2)
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "search", "--refresh", check=False)
    assert result.returncode == 0, (
        f"multi-registry search must exit 0, got {result.returncode}; stderr: {result.stderr}"
    )
    rows = json.loads(result.stdout)["items"]
    assert isinstance(rows, list), f"search must return a JSON array, got {rows!r}"

    shared_repos = [r.get("repo", "") for r in rows if shared in r.get("repo", "")]
    assert len(shared_repos) == 2, (
        f"the same repo in two registries must surface twice (no dedup), "
        f"got: {shared_repos}"
    )
    assert any(f"{REGISTRY_HOST}/{ns1}" in repo for repo in shared_repos), (
        f"registry 1's copy of '{shared}' must appear, got: {shared_repos}"
    )
    assert any(f"{REGISTRY_HOST}/{ns2}" in repo for repo in shared_repos), (
        f"registry 2's copy of '{shared}' must appear, got: {shared_repos}"
    )
    assert shared_repos[0] != shared_repos[1], (
        f"the two copies must be distinct fully-qualified refs, got: {shared_repos}"
    )


# ---------------------------------------------------------------------------
# Test 6 — legacy [options].default_registry still resolves (back-compat lock)
# ---------------------------------------------------------------------------


def test_legacy_default_registry_still_resolves(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A hand-written ``[options].default_registry`` config must still resolve
    short references — backward compatibility guard for the deprecation.

    After P2 migrates init to ``[[registries]]``, a pre-existing config using
    the legacy field must continue to work unchanged (deprecate-and-read, not
    remove).
    """
    make_artifact(
        f"{unique_repo}/legacy-tool",
        "skill",
        {"legacy-tool/SKILL.md": "---\nname: legacy-tool\ndescription: d\n---\n# L\n"},
        tag="1",
    )

    # Hand-write the legacy config shape.
    (project_dir / "grimoire.toml").write_text(
        f'[options]\ndefault_registry = "{REGISTRY_HOST}"\n\n[skills]\n\n[rules]\n'
    )

    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    short_ref = f"{unique_repo}/legacy-tool:1"
    out = runner.json("add", short_ref)
    assert out["kind"] == "skill"
    assert out["status"] == "added", f"add with legacy config must succeed: {out!r}"

    cfg = (project_dir / "grimoire.toml").read_text()
    assert f"{REGISTRY_HOST}/{unique_repo}/legacy-tool" in cfg, (
        f"legacy-resolved skill binding must use the legacy registry host; got:\n{cfg}"
    )
    # The legacy field must survive re-serialization (no destructive migration).
    assert f'default_registry = "{REGISTRY_HOST}"' in cfg, (
        f"write_config must preserve the legacy default_registry; got:\n{cfg}"
    )


# ---------------------------------------------------------------------------
# Test 7 — both fields present: array wins, legacy not used for short refs
# ---------------------------------------------------------------------------


def test_both_fields_array_wins(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """When both ``[options].default_registry`` and ``[[registries]]`` are
    present, ``[[registries]]`` is authoritative for short-ref expansion.

    A legacy ``default_registry`` pointing at the real registry and a
    ``[[registries]]`` entry pointing at a non-existent host: if the array
    wins, add fails (non-existent host); if the legacy wins, add succeeds.
    We assert that the array wins (add fails with a network error, not success
    with the legacy host).
    """
    make_artifact(
        f"{unique_repo}/both-tool",
        "skill",
        {"both-tool/SKILL.md": "---\nname: both-tool\ndescription: d\n---\n# B\n"},
        tag="1",
    )

    # Array points at a dead host; legacy points at the real registry.
    # If the resolver erroneously uses the legacy path, add would succeed.
    dead_host = "localhost:9999"
    (project_dir / "grimoire.toml").write_text(
        f'[options]\n'
        f'default_registry = "{REGISTRY_HOST}"\n'
        f'\n'
        f'[[registries]]\n'
        f'oci = "{dead_host}"\n'
        f'default = true\n'
        f'\n'
        f'[skills]\n'
        f'\n'
        f'[rules]\n'
    )

    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    # add must attempt the dead host (array wins), fail, and exit non-zero.
    result = runner.run("--format", "json", "add", f"{unique_repo}/both-tool:1", check=False)
    assert result.returncode != 0, (
        f"add must fail when [[registries]] points at an unreachable host; "
        f"if it succeeded, legacy default_registry was used instead of the array. "
        f"returncode={result.returncode}, stdout={result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# Test 8 — two default = true entries are rejected with exit 78
# ---------------------------------------------------------------------------


def test_two_defaults_rejected(
    grim_at, project_dir: Path
) -> None:
    """A ``grimoire.toml`` with two ``[[registries]]`` entries both carrying
    ``default = true`` must be rejected at parse time with exit 78 (EX_CONFIG)
    and the error must mention "default".
    """
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\n'
        'oci = "ghcr.io/acme"\n'
        'default = true\n'
        '\n'
        '[[registries]]\n'
        'oci = "registry.corp/team"\n'
        'default = true\n'
        '\n'
        '[skills]\n'
        '\n'
        '[rules]\n'
    )

    runner = grim_at(project_dir)
    result = runner.run("status", check=False)
    assert result.returncode == 78, (
        f"two default = true entries must exit 78 (EX_CONFIG), "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert "default" in result.stderr.lower(), (
        f"error message must mention 'default'; got: {result.stderr!r}"
    )


# ---------------------------------------------------------------------------
# Test 9 — a malformed GLOBAL config is fatal for a project-scope run too
# ---------------------------------------------------------------------------


# A `[[registries]]` entry whose `include` glob does not compile —
# `validate_registries` rejects it (exit 78), the same class of fault Test 8
# pins for the project config.
_MALFORMED_GLOBAL_CONFIG = (
    '[[registries]]\n'
    'alias = "acme"\n'
    'oci = "ghcr.io/acme"\n'
    'include = ["acme{unclosed"]\n'
)


def _valid_project_config(project_dir: Path) -> None:
    (project_dir / "grimoire.toml").write_text(
        f'[[registries]]\n'
        f'oci = "{REGISTRY_HOST}"\n'
        f'default = true\n'
        f'\n'
        f'[skills]\n'
        f'\n'
        f'[rules]\n'
    )


@pytest.mark.parametrize(
    "command",
    [
        pytest.param(("context",), id="context"),
        pytest.param(("--offline", "search", "foo"), id="search"),
    ],
)
def test_malformed_global_config_is_fatal_at_project_scope(
    grim_at, project_dir: Path, grim_home: Path, command: tuple[str, ...]
) -> None:
    """A broken ``$GRIM_HOME/grimoire.toml`` must exit 78, not be swallowed.

    The global config is a lower-priority registry tier for every
    project-scope command. Loading it best-effort meant a malformed global
    config silently dropped every global registry and the command exited 0 —
    the user's registries just vanished. Same contract as the project config
    (Test 8): a config grim cannot parse is exit 78 (EX_CONFIG), and the
    diagnostic names the offending file.
    """
    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL_CONFIG)
    _valid_project_config(project_dir)

    runner = grim_at(project_dir)
    result = runner.run(*command, check=False)
    assert result.returncode == 78, (
        f"a malformed global config must exit 78 (EX_CONFIG) on a project-scope "
        f"`grim {' '.join(command)}`, got {result.returncode}; stderr: {result.stderr}"
    )
    assert str(grim_home / "grimoire.toml") in result.stderr, (
        f"the diagnostic must name the global config path; got: {result.stderr!r}"
    )


def test_absent_global_config_still_exits_zero(
    grim_at, project_dir: Path, grim_home: Path
) -> None:
    """An absent global config stays an empty declaration, never an error.

    The boundary of Test 9: `GlobalConfig::load` maps NotFound to an empty
    config, so the fresh-install case must keep exiting 0.
    """
    assert not (grim_home / "grimoire.toml").exists()
    _valid_project_config(project_dir)

    runner = grim_at(project_dir)
    result = runner.run("context", check=False)
    assert result.returncode == 0, (
        f"an absent global config must not fail a project-scope run; "
        f"got {result.returncode}; stderr: {result.stderr}"
    )


def test_search_registry_flag_never_reads_the_global_config(
    grim_at, project_dir: Path, grim_home: Path
) -> None:
    """`grim search --registry <r>` collapses the browse set before any config
    is consulted, so a malformed global config cannot fail it.

    Preserving that escape hatch is part of the fix: the flag path never
    reads the global config, so it must keep exiting 0 — otherwise a user
    locked out by a broken global config would have no way to search past it.
    """
    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL_CONFIG)
    _valid_project_config(project_dir)

    runner = grim_at(project_dir)
    result = runner.run(
        "--offline", "search", "--registry", f"{REGISTRY_HOST}/nothing-here", "foo", check=False
    )
    assert result.returncode == 0, (
        f"--registry must bypass the global config entirely; "
        f"got {result.returncode}; stderr: {result.stderr}"
    )


# ---------------------------------------------------------------------------
# Test 10 — exit 78 reaches every command that resolves a registry set
# ---------------------------------------------------------------------------
#
# Test 9 pins `context` and `search`. The exit-0→78 move lands on the whole
# `registries_for_scope` seam, and the rest of it was correct only by
# inspection — nothing failed if a command dropped back to swallowing the
# error. These fill that in.
#
# Three shapes, because "malformed" is what every doc says and only the first
# of them is a parse failure. The other two are files that every TOML linter,
# editor and formatter accepts and that grim still refuses at
# `validate_registries` — the case a user is least likely to suspect, and the
# one no doc mentions.

_UNPARSEABLE_GLOBAL_CONFIG = "this is not toml at all\n[[[\n"

# Valid TOML; `include` is not a compilable glob (same shape as
# `_MALFORMED_GLOBAL_CONFIG` above, kept separate so the table reads whole).
_UNCOMPILABLE_FILTER_GLOBAL_CONFIG = (
    '[[registries]]\n'
    'alias = "acme"\n'
    'oci = "ghcr.io/acme"\n'
    'include = ["acme{unclosed"]\n'
)

# Valid TOML, compilable patterns, two entries claiming the default.
_DUPLICATE_DEFAULT_GLOBAL_CONFIG = (
    '[[registries]]\n'
    'alias = "a"\n'
    'oci = "ghcr.io/a"\n'
    'default = true\n'
    '\n'
    '[[registries]]\n'
    'alias = "b"\n'
    'oci = "ghcr.io/b"\n'
    'default = true\n'
)

_BROKEN_GLOBAL_CONFIGS = [
    pytest.param(_UNPARSEABLE_GLOBAL_CONFIG, id="unparseable-toml"),
    pytest.param(_UNCOMPILABLE_FILTER_GLOBAL_CONFIG, id="uncompilable-filter"),
    pytest.param(_DUPLICATE_DEFAULT_GLOBAL_CONFIG, id="duplicate-default"),
]


def _closed_stdin_run(runner: GrimRunner, *args: str) -> subprocess.CompletedProcess[str]:
    """``runner.run`` with stdin closed.

    `grim login` prompts for a password when stdin is a TTY. Under
    `pytest -s` a regression that stopped exiting 78 would therefore hang
    the suite rather than fail it, and a hung CI job is a much worse
    signal than a red one.
    """
    return subprocess.run(
        [str(runner.binary), *args],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        env=runner.env,
        cwd=str(runner.cwd) if runner.cwd else None,
        check=False,
    )


@pytest.mark.parametrize("broken", _BROKEN_GLOBAL_CONFIGS)
@pytest.mark.parametrize(
    "command",
    [
        # `add` reads the set to expand a possible alias; measured 65 (kind
        # inference) with a healthy global config, so a 78 here is the
        # config and nothing else.
        pytest.param(("add", "localhost:5000/nope/thing:latest"), id="add"),
        # The path deliberately does not exist: `release` resolves the
        # registry set before it validates the package directory (measured
        # 65, "skill directory", with a healthy global config), so the test
        # needs no fixture on disk.
        pytest.param(("release", "no-such-package-dir", "ghcr.io/acme/demo:1.0.0"), id="release"),
        # Without `-u` this is 64 on a healthy global config — never a
        # prompt, since `_closed_stdin_run` gives it no TTY.
        pytest.param(("login", "ghcr.io", "--no-verify"), id="login"),
    ],
)
def test_malformed_global_config_is_fatal_for_registry_resolving_commands(
    grim_at, project_dir: Path, grim_home: Path, tmp_path: Path,
    command: tuple[str, ...], broken: str,
) -> None:
    """`add` / `release` / `login` inherit test 9's contract, in all three shapes.

    `login` is the one that must stay hard: writing a credential to a host
    resolved from a config grim could not read is the dangerous direction,
    so it fails closed. Its inverse, `logout`, must not — that is the test
    below.

    **`grim tui` is not covered and cannot be**: `src/command/tui.rs` returns
    `ExitCode::Success` the moment stdout is not a TTY, so every acceptance
    run exits 0 before any config is read. Verified by hand at exit 78 with
    a real terminal; the gap is structural, not an oversight.
    """
    (grim_home / "grimoire.toml").write_text(broken)
    _valid_project_config(project_dir)

    runner = grim_at(project_dir)
    runner.env["DOCKER_CONFIG"] = str(tmp_path / "docker-unused")
    result = _closed_stdin_run(runner, *command)

    assert result.returncode == 78, (
        f"a broken global config must exit 78 (EX_CONFIG) on "
        f"`grim {' '.join(command)}`, got {result.returncode}; "
        f"stderr: {result.stderr}"
    )
    assert str(grim_home / "grimoire.toml") in result.stderr, (
        f"the diagnostic must name the global config path; got: {result.stderr!r}"
    )


@pytest.mark.parametrize("broken", _BROKEN_GLOBAL_CONFIGS)
def test_logout_removes_the_credential_despite_a_broken_global_config(
    grim_at, project_dir: Path, grim_home: Path, tmp_path: Path, broken: str
) -> None:
    """`grim logout <host>` must stay exit 0 and still erase the credential.

    The deliberate exception to the test above, and the direction the exit-78
    move must not travel: `logout` is what a user runs when a token has
    leaked, an explicit positional host needs no registry set to resolve
    (`ghcr.io` is not an alias), and grim cannot repair the global config
    that would block it — `grim config registry rm --global` exits 78 on the
    same file. `logout` is already idempotent, so degrading to an empty
    registry set costs nothing.
    """
    docker_config = tmp_path / "docker"
    docker_config.mkdir()
    (docker_config / "config.json").write_text(
        '{"auths": {"ghcr.io": {"auth": "dXNlcjpwYXNz"}}}'
    )
    (grim_home / "grimoire.toml").write_text(broken)
    _valid_project_config(project_dir)

    runner = grim_at(project_dir)
    runner.env["DOCKER_CONFIG"] = str(docker_config)
    result = _closed_stdin_run(runner, "logout", "ghcr.io")

    assert result.returncode == 0, (
        f"`grim logout <host>` must not be blocked by an unrelated global "
        f"config, got {result.returncode}; stderr: {result.stderr}"
    )
    auths = json.loads((docker_config / "config.json").read_text()).get("auths", {})
    assert "ghcr.io" not in auths, (
        f"the credential must actually be gone, not merely reported removed; "
        f"got: {auths}"
    )


# ---------------------------------------------------------------------------
# Per-registry browse filters (`include` / `exclude`)
# ---------------------------------------------------------------------------
#
# Fixture shape. Every filter test publishes a small repository tree under one
# UUID namespace and declares that namespace as the source URL, so the match
# candidate is the path BELOW it (plan C-005):
#
#     source oci  = localhost:5000/grim-test/<uuid>
#     row         = localhost:5000/grim-test/<uuid>/platform/foo
#     candidate   = platform/foo
#
# Namespacing the source also bounds the catalog walk to this test's own
# repositories, which is what makes the `0 of N` count in the C-019 warning
# deterministic on a registry shared with every other test in the session.


def _publish_skill(repo: str, name: str) -> None:
    """Push a minimal, non-deprecated skill at ``repo`` (catalog fodder)."""
    make_artifact(
        repo,
        "skill",
        {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: {name}\n---\n# {name}\n"},
        tag="latest",
        annotations={"org.opencontainers.image.description": f"description of {name}"},
    )


def _publish_filter_tree(ns: str) -> None:
    """Publish the four-repository tree the include/exclude cases match against."""
    _publish_skill(f"{ns}/platform/foo", "foo")
    _publish_skill(f"{ns}/platform/foo/deep", "deep")
    _publish_skill(f"{ns}/platform/bar", "bar")
    _publish_skill(f"{ns}/internal/thing", "thing")


def _toml_list(patterns: tuple[str, ...]) -> str:
    return "[" + ", ".join(f'"{p}"' for p in patterns) + "]"


def _ns_rel(ns: str, *patterns: str) -> tuple[str, ...]:
    """Anchor ns-relative patterns onto the repository path grim matches.

    A pattern is tested against the row's repository path — the reference
    with the registry HOST removed and nothing else removed — so a filter on
    a source rooted at ``<host>/<ns>`` still has to name ``<ns>`` itself. The
    entry's own locator is not an input, which is why this is spelled at the
    call site rather than derived from it.
    """
    return tuple(f"{ns}/{p}" for p in patterns)


def _filtered_config(
    project_dir: Path,
    ns: str,
    include: tuple[str, ...] = (),
    exclude: tuple[str, ...] = (),
    alias: str = "acme",
) -> None:
    """Write a one-entry `[[registries]]` config carrying a browse filter."""
    lines = [
        "[[registries]]",
        f'alias = "{alias}"',
        f'oci = "{REGISTRY_HOST}/{ns}"',
    ]
    if include:
        lines.append(f"include = {_toml_list(include)}")
    if exclude:
        lines.append(f"exclude = {_toml_list(exclude)}")
    lines += ["default = true", "", "[skills]", "", "[rules]", ""]
    (project_dir / "grimoire.toml").write_text("\n".join(lines))


def _visible_candidates(runner, ns: str, *extra: str) -> set[str]:
    """The browse-visible rows of ``ns``, as source-relative candidates."""
    rows = runner.json("search", "--refresh", *extra)["items"]
    prefix = f"{REGISTRY_HOST}/{ns}/"
    return {r["repo"].removeprefix(prefix) for r in rows if r["repo"].startswith(prefix)}


@pytest.mark.parametrize(
    ("include", "exclude", "expected"),
    [
        pytest.param(
            ("platform",),
            (),
            {"platform/foo", "platform/foo/deep", "platform/bar"},
            id="S-001-include-subtree",
        ),
        pytest.param(
            ("platform/foo",),
            ("platform/foo/**",),
            {"platform/foo"},
            id="S-002-exclude-wins-on-overlap",
        ),
        pytest.param(
            (),
            ("internal/**",),
            {"platform/foo", "platform/foo/deep", "platform/bar"},
            id="S-003-exclude-only",
        ),
    ],
)
def test_browse_filter_narrows_search_to_the_declared_patterns(
    grim_at,
    project_dir: Path,
    registry: str,
    include: tuple[str, ...],
    exclude: tuple[str, ...],
    expected: set[str],
) -> None:
    """`grim search` shows exactly the rows the filter admits (S-001…S-003).

    All three cases run against the same four-repository tree so the
    expectations are directly comparable:

    - **S-001** a wildcard-free `platform` expands to `platform{,/**}` and
      admits the whole subtree, hiding `internal/thing`.
    - **S-002** `include = ["platform/foo"]` admits the package and its
      descendants; `exclude = ["platform/foo/**"]` then removes only the
      descendants — exclude wins on overlap, leaving exactly one package.
    - **S-003** an exclude-only filter means "everything except", not
      "nothing": an empty include list skips the include check entirely.

    Each case asserts the full visible set, not just the presence of one row,
    so a filter that silently admits everything fails as loudly as one that
    admits nothing.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_filter_tree(ns)
    _filtered_config(
        project_dir,
        ns,
        include=_ns_rel(ns, *include),
        exclude=_ns_rel(ns, *exclude),
    )

    runner = grim_at(project_dir)
    assert _visible_candidates(runner, ns) == expected


def test_zero_match_filter_warns_on_stderr_and_still_exits_zero(
    grim_at, project_dir: Path, registry: str
) -> None:
    """A filter that admits nothing is legal, loud, and green (S-004, S-017).

    Editing a source's `oci` url re-points every relative pattern in that
    entry (the candidate is source-relative, plan C-005), so the failure mode
    this guards is a browse that silently goes empty. `load_catalog` emits the
    C-019 line once per affected source; nothing else in the tree proves it
    reaches a real process's stderr — the unit layer tests `zero_match_warning`
    as a pure function and the capturing subscriber only sees the seam.

    The second half is the other side of the same contract: `grim status
    --check` loads the catalog under `CatalogScope::Complete`, so the same
    config must produce **no** warning there. It reads the very cache the
    search run just wrote, which also pins C-008's read-time-only rule.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/platform/foo", "foo")
    _publish_skill(f"{ns}/internal/thing", "thing")
    # Nothing under this namespace is called `nope`, so the include list
    # admits 0 of the 2 repositories the source lists.
    _filtered_config(project_dir, ns, include=_ns_rel(ns, "nope/**"))

    runner = grim_at(project_dir)
    result = runner.run("--format", "json", "search", "--refresh", check=False)

    assert result.returncode == 0, (
        f"a filter matching nothing must stay exit 0; got {result.returncode}; "
        f"stderr: {result.stderr}"
    )
    assert json.loads(result.stdout)["items"] == [], (
        f"the filter admits nothing, so the browse must be empty; got: {result.stdout!r}"
    )
    assert "registry 'acme': filter admitted 0 of 2 repositories" in result.stderr, (
        f"the C-019 diagnostic must reach stderr verbatim, naming the source "
        f"and both counts; got: {result.stderr!r}"
    )

    checked = runner.run("status", "--check", check=False)
    assert checked.returncode == 0, (
        f"`status --check` on a filtered config must exit 0; "
        f"got {checked.returncode}; stderr: {checked.stderr}"
    )
    assert "filter admitted" not in checked.stderr, (
        f"`status --check` browses under Complete scope and must never emit the "
        f"browse-filter diagnostic; got: {checked.stderr!r}"
    )


def test_status_check_ignores_the_browse_filter(
    grim_at, project_dir: Path, registry: str
) -> None:
    """An excluded artifact stays hidden in search and complete in status (S-005).

    `grim status --check` exists to answer "is anything I declared deprecated
    or replaced?". It loads the catalog under `CatalogScope::Complete`
    precisely so a browse filter cannot hide a declared artifact's notice —
    flipping that one token to `Browse` turns this row's `deprecated` null
    while every unit test stays green.

    One config drives both halves: the same `exclude` that hides the artifact
    from `grim search` must change nothing about `grim status --check`. The
    `keeper` row is the control — without it, an unreachable registry would
    satisfy the "hidden from search" half vacuously.

    Also covers **S-006**: the reference is excluded, yet `grim add` resolves,
    declares, locks and installs it — the filter never reaches resolution.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    make_artifact(
        f"{ns}/old-skill",
        "skill",
        {"old-skill/SKILL.md": "---\nname: old-skill\ndescription: old\n---\n# old\n"},
        tag="latest",
        annotations={"com.grimoire.deprecated": "use new-skill instead"},
    )
    _publish_skill(f"{ns}/keeper", "keeper")
    _filtered_config(project_dir, ns, exclude=_ns_rel(ns, "old-skill"))

    runner = grim_at(project_dir)

    # S-006: an excluded reference still resolves, declares, locks, installs.
    added = runner.json("add", f"{REGISTRY_HOST}/{ns}/old-skill:latest")
    assert added["status"] == "added", f"an excluded ref must still install; got {added!r}"
    assert f"{REGISTRY_HOST}/{ns}/old-skill" in (project_dir / "grimoire.lock").read_text(), (
        "the excluded artifact must be pinned in the lock"
    )
    assert_dir_exists(project_dir / ".claude" / "skills" / "old-skill")
    # `grim add` re-serializes grimoire.toml; the filter must survive that
    # round-trip, or the rest of this test would pass for the wrong reason.
    assert f'exclude = ["{ns}/old-skill"]' in (project_dir / "grimoire.toml").read_text(), (
        "write_config must preserve the authored browse filter"
    )

    # Browse: the excluded artifact is gone, the sibling is not.
    # `--show-deprecated` removes the unrelated deprecation-hiding rule from
    # the picture, so the exclude pattern is the only thing under test.
    visible = _visible_candidates(runner, ns, "--show-deprecated")
    assert visible == {"keeper"}, (
        f"the exclude pattern must hide only 'old-skill'; got: {visible}"
    )

    # Complete: the declared artifact's deprecation notice is still reported.
    doc = runner.json("status", "--check")
    assert doc["checked"] is True, f"`--check` must have run live; got: {doc!r}"
    row = next(i for i in doc["items"] if i["name"] == "old-skill")
    assert row["deprecated"] == "use new-skill instead", (
        f"`status --check` must ignore the browse filter and populate "
        f"'deprecated' for a declared artifact; got: {row!r}"
    )


def test_registry_flag_bypasses_the_configured_filter(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`--registry` collapses the browse set and applies no filter (S-014).

    The forced branch constructs its entries from the flag alone, so it never
    sees a `[[registries]]` entry's patterns. The config here hides
    everything; the flag must still list both repositories.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/platform/foo", "foo")
    _publish_skill(f"{ns}/internal/thing", "thing")
    _filtered_config(project_dir, ns, include=_ns_rel(ns, "nope/**"))

    runner = grim_at(project_dir)
    rows = runner.json("search", "--registry", f"{REGISTRY_HOST}/{ns}", "--refresh")["items"]
    repos = [r["repo"] for r in rows]

    assert any(r.endswith("/platform/foo") for r in repos), (
        f"--registry must browse unfiltered; got repos: {repos}"
    )
    assert any(r.endswith("/internal/thing") for r in repos), (
        f"--registry must browse unfiltered; got repos: {repos}"
    )


MALFORMED_PROJECT_FILTER = (
    '[[registries]]\n'
    'alias = "acme"\n'
    'oci = "ghcr.io/acme"\n'
    'include = ["acme/["]\n'
    'default = true\n'
)
"""The review's Block reproduction: a project config whose only fault is an
uncompilable browse pattern. Shared by the four tests below so the fix and
the two asymmetries it preserves are all judged against one fixture."""


def test_malformed_project_filter_never_browses_the_fallback_set_b1(
    grim_at, project_dir: Path, grim_home: Path, registry: str
) -> None:
    """A config grim cannot validate must fail, not browse somebody else (B1).

    The defect was never "wrong exit code" — `grim search` swallowed the
    parse failure, fell through to `registries_global_fallback`, and listed
    **a different registry set** at exit 0, hiding the very file the user
    has to fix while `context`/`config get`/`add` all exited 78 on it.

    The control runs first and is what gives the negative assertion teeth:
    with no project config at all, that same fallback branch is taken, it is
    still supposed to work, and it lists the row. Only then is the broken
    config written — so "the row is absent" means "the fallback was not
    taken", not "the fixture had nothing to show". Deleting the fix makes
    the second half report exit 0 and print the control's row.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/platform/foo", "foo")
    (grim_home / "grimoire.toml").write_text(
        f'[[registries]]\nalias = "fallback"\noci = "{REGISTRY_HOST}/{ns}"\ndefault = true\n'
    )
    runner = grim_at(project_dir)

    control = runner.run("search", "--refresh", check=False)
    assert control.returncode == 0, (
        f"the fallback branch must keep working when no project config exists; "
        f"got {control.returncode}; stderr: {control.stderr}"
    )
    assert f"{ns}/platform/foo" in control.stdout, (
        f"control: the fallback set must list the global registry's row, or the "
        f"assertion below passes vacuously; got: {control.stdout!r}"
    )

    (project_dir / "grimoire.toml").write_text(MALFORMED_PROJECT_FILTER)
    result = runner.run("search", "--refresh", check=False)

    assert result.returncode == 78, (
        f"a malformed project config must exit 78 (EX_CONFIG), not browse on; "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert str(project_dir / "grimoire.toml") in result.stderr, (
        f"the diagnostic must name the file to fix; got: {result.stderr!r}"
    )
    assert "acme/[" in result.stderr, (
        f"the diagnostic must quote the offending pattern; got: {result.stderr!r}"
    )
    assert f"{ns}/platform/foo" not in result.stdout, (
        f"the run must not fall back to the global registry set the control "
        f"just proved is reachable; got: {result.stdout!r}"
    )
    assert "index.grimoire.rs" not in result.stdout, (
        f"nor to the built-in package index; got: {result.stdout!r}"
    )


def test_registry_flag_still_browses_past_a_malformed_project_filter_s014(
    grim_at, project_dir: Path, registry: str
) -> None:
    """`--registry` collapses the browse set before any config is read (S-014).

    A recorded, deliberate asymmetry: the flag path returns from
    `resolve_scope` before the project config is touched, so a user locked
    out by a config they cannot parse still has a way to search. Without
    this test the acceptance layer cannot tell the B1 fix from an over-fix
    that propagates every config error from every branch.

    Asserting the row — not just exit 0 — is what stops it passing on an
    empty browse that never reached the registry.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/platform/foo", "foo")
    (project_dir / "grimoire.toml").write_text(MALFORMED_PROJECT_FILTER)

    runner = grim_at(project_dir)
    result = runner.run("search", "--registry", f"{REGISTRY_HOST}/{ns}", "--refresh", check=False)

    assert result.returncode == 0, (
        f"--registry must bypass the project config entirely; "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert f"{ns}/platform/foo" in result.stdout, (
        f"--registry must actually browse the forced set, not exit 0 empty; "
        f"got: {result.stdout!r}"
    )


def test_status_without_check_survives_a_malformed_global_config_s013(
    grim_at, project_dir: Path, grim_home: Path, registry: str
) -> None:
    """Plain `grim status` never resolves a browse set, so it stays 0 (S-013).

    The other half of `test_search_registry_flag_never_reads_the_global_config`:
    `registries_for_scope` is reached only from the `--check` branch, so a
    global config grim cannot parse is invisible to a local status report and
    fatal to a live one. Hoisting that call out of the branch — a plausible
    "simplification" — would make every offline status start failing on a
    broken global config, and only this pair fails loudly when it does.

    The `--check` half is the control: it proves the fixture really is
    malformed and really is read on the path that needs it.
    """
    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL_CONFIG)
    _valid_project_config(project_dir)

    runner = grim_at(project_dir)
    local = runner.run("status", check=False)
    assert local.returncode == 0, (
        f"`status` without `--check` reads no registry set and must not fail on a "
        f"broken global config; got {local.returncode}; stderr: {local.stderr}"
    )

    checked = runner.run("status", "--check", check=False)
    assert checked.returncode == 78, (
        f"control: `status --check` does resolve the browse set, so the same "
        f"config must be fatal there; got {checked.returncode}; stderr: {checked.stderr}"
    )
    assert str(grim_home / "grimoire.toml") in checked.stderr, (
        f"the diagnostic must name the global config path; got: {checked.stderr!r}"
    )


@pytest.mark.parametrize(
    "command",
    [
        pytest.param(("context",), id="context"),
        pytest.param(("config", "list"), id="config-list"),
    ],
)
def test_malformed_global_config_is_fatal_at_global_scope_w17(
    grim, grim_home: Path, command: tuple[str, ...]
) -> None:
    """`--global` resolves the global config as its own scope — also 78 (W17).

    Every other exit-78 test on this branch runs at *project* scope, where
    the global config is a lower registry tier folded in by
    `global_config_registries`. `--global` takes a different route entirely:
    `scope_resolution::resolve_in` loads it as the active scope, and that
    branch returns an empty tier from the folding seam by design. The two
    paths agreeing is a claim, and this is the test of it.

    The absent-config control is the boundary: a fresh install has no global
    config, and that must stay exit 0 rather than becoming a hard error.
    """
    runner = grim
    absent = runner.run("--global", *command, check=False)
    assert absent.returncode == 0, (
        f"control: an absent global config must not fail a global-scope "
        f"`grim {' '.join(command)}`; got {absent.returncode}; stderr: {absent.stderr}"
    )

    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL_CONFIG)
    result = runner.run("--global", *command, check=False)
    assert result.returncode == 78, (
        f"a malformed global config must exit 78 (EX_CONFIG) at global scope too; "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert str(grim_home / "grimoire.toml") in result.stderr, (
        f"the diagnostic must name the global config path; got: {result.stderr!r}"
    )
    assert "acme{unclosed" in result.stderr, (
        f"the diagnostic must quote the offending pattern; got: {result.stderr!r}"
    )


def test_zero_match_warning_never_pairs_with_the_catalog_gate_hint_h4(
    grim_at, project_dir: Path, registry: str
) -> None:
    """One empty browse, one explanation — never two contradicting ones (H4).

    A filter that admits nothing empties the *rendered* rows, and the
    `_catalog`-gate hint keys on exactly that emptiness. Before the fix both
    lines printed on the same run: "your filter admitted 0 of 2" immediately
    followed by "this registry probably gates `_catalog`" plus a doc link to
    an unrelated subject — the second one false, and the one a user follows.
    The unit layer pins the gate predicate; only a real process proves the
    two strings never reach one stderr together.

    The second half is the control, and it is the whole point: the same
    binary, an unfiltered source that is genuinely empty, and the hint
    *does* fire. Without it "the hint is absent" would also pass on a build
    that had deleted the hint outright.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/platform/foo", "foo")
    _publish_skill(f"{ns}/internal/thing", "thing")
    _filtered_config(project_dir, ns, include=_ns_rel(ns, "nope/**"))

    runner = grim_at(project_dir)
    filtered = runner.run("search", "--refresh", check=False)

    assert filtered.returncode == 0, (
        f"a filter matching nothing stays exit 0; got {filtered.returncode}; "
        f"stderr: {filtered.stderr}"
    )
    assert "filter admitted 0 of" in filtered.stderr, (
        f"the filter must own the empty result; got: {filtered.stderr!r}"
    )
    assert "gate the `_catalog` browse endpoint" not in filtered.stderr, (
        f"the registry-gate hint contradicts the line above it and must not "
        f"appear on the same run; got: {filtered.stderr!r}"
    )
    assert "grimoire.rs/configuration.html#registry-compatibility" not in filtered.stderr, (
        f"nor its doc link, which points at an unrelated subject; "
        f"got: {filtered.stderr!r}"
    )

    empty_ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _filtered_config(project_dir, empty_ns, alias="unfiltered")
    unfiltered = runner.run("search", "--refresh", check=False)
    assert "gate the `_catalog` browse endpoint" in unfiltered.stderr, (
        f"control: an unfiltered source that came back empty is exactly what "
        f"the hint is for, and it must still fire; got: {unfiltered.stderr!r}"
    )


def test_exclude_that_removes_nothing_stays_silent(
    grim_at, project_dir: Path, registry: str
) -> None:
    """A no-op exclude must NOT warn — the trigger was tried and removed.

    W12 briefly shipped an `admitted N of N` trigger here, aimed at the
    authoring mistake this fixture builds: the match candidate is
    source-relative (plan C-005), the source is `<host>/grim-test/<uuid>`,
    every candidate is `platform/...`, so `grim-test/**` repeats a segment
    already spelled in the `oci` and excludes zero of them.

    **Do not add it back.** `N of N` is also the permanent steady state of a
    *correct* exclude with nothing to match yet — `exclude = ["archive/**"]`
    against a source that has no `archive/*` repository is right, will work
    the day one is published, and would warn on every browse until then.
    Counts cannot tell the two apart, and no state ever clears the false
    one. Worse, the remedy clause the message carries tells a user whose
    patterns *are* source-relative that they are not, so the trigger burns
    the credibility of `admitted 0 of N`, which shares that sentence and
    does attach to a real symptom: you asked to see a set and saw nothing.

    The rows assertion is what still earns this test its place — it pins
    that a no-op exclude is genuinely a no-op, which is the half that was
    never in doubt and is the half worth guarding.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/platform/foo", "foo")
    _publish_skill(f"{ns}/platform/bar", "bar")
    # Written without the namespace the repository path actually carries, so
    # it looks like it should match every row and matches none.
    _filtered_config(project_dir, ns, exclude=("platform/**",))

    runner = grim_at(project_dir)
    result = runner.run("search", "--refresh", check=False)

    assert result.returncode == 0, (
        f"a no-op exclude is not a failure; got {result.returncode}; "
        f"stderr: {result.stderr}"
    )
    assert "filter admitted" not in result.stderr, (
        f"a no-op exclude is indistinguishable from a correct exclude with nothing "
        f"to match yet, so it must not warn; got: {result.stderr!r}"
    )
    assert _visible_candidates(runner, ns) == {"platform/foo", "platform/bar"}, (
        "control: every row survives — the exclude really did remove nothing"
    )


def test_config_list_all_carries_the_browse_filter_rows_s012(
    grim_at, project_dir: Path
) -> None:
    """`config list` surfaces `include`/`exclude` like any other field (S-012).

    The two filter fields were appended to `RegistryField::ALL`, which is
    what `list --all` iterates — a config surface with unit coverage only.
    A set pattern is a row without `--all`; the unset sibling needs it. If
    the fields ever drop out of that iteration a user loses the only way to
    discover them from the CLI, and nothing else here notices.
    """
    (project_dir / "grimoire.toml").write_text(
        "[[registries]]\n"
        'alias = "acme"\n'
        f'oci = "{REGISTRY_HOST}"\n'
        'include = ["platform/**"]\n'
        "default = true\n"
    )
    runner = grim_at(project_dir)

    all_rows = {i["key"]: i for i in runner.json("config", "list", "--all")["items"]}
    assert "registry.acme.include" in all_rows, (
        f"`list --all` must offer the include key; got: {sorted(all_rows)}"
    )
    assert "registry.acme.exclude" in all_rows, (
        f"`list --all` must offer the unset exclude key too; got: {sorted(all_rows)}"
    )
    assert all_rows["registry.acme.include"]["value"] == "platform/**", (
        f"the include row must echo the authored pattern; "
        f"got: {all_rows['registry.acme.include']!r}"
    )
    assert all_rows["registry.acme.exclude"]["set"] is False, (
        f"the unfiltered exclude row must report set False; "
        f"got: {all_rows['registry.acme.exclude']!r}"
    )

    # Control: `--all` is what adds the unset row — without it only the
    # authored one is listed, so the assertion above is about the flag.
    set_only = {i["key"] for i in runner.json("config", "list")["items"]}
    assert "registry.acme.include" in set_only, set_only
    assert "registry.acme.exclude" not in set_only, set_only


def test_excluded_reference_still_locks_and_installs_s015(
    grim_at, project_dir: Path, registry: str
) -> None:
    """A browse filter is not access control — the headline claim (C-018).

    `test_status_check_ignores_the_browse_filter` proves it for `grim add`,
    which resolves, locks and installs in one call. This is the other route:
    a hand-written declaration walked through `grim lock` and `grim install`
    as separate commands, which is what a committed `grimoire.toml` does on
    a colleague's checkout. Neither command may consult the filter — the
    property is currently guaranteed by a call graph, so a refactor that
    threaded the filter one level deeper into resolution would break no
    other test.

    The `search` half at the end is the control: it proves the exclude is
    live for this fixture. Without it every assertion here would pass
    against a config carrying no filter at all.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_skill(f"{ns}/hidden/secret-skill", "secret-skill")
    _publish_skill(f"{ns}/keeper", "keeper")
    (project_dir / "grimoire.toml").write_text(
        "[[registries]]\n"
        'alias = "acme"\n'
        f'oci = "{REGISTRY_HOST}/{ns}"\n'
        # Anchored on the repository path, which carries the namespace — the
        # entry's own locator is not what a pattern is matched against.
        f'exclude = ["{ns}/hidden/**"]\n'
        "default = true\n"
        "\n[skills]\n"
        f'secret-skill = "{REGISTRY_HOST}/{ns}/hidden/secret-skill:latest"\n'
        "\n[rules]\n"
    )
    runner = grim_at(project_dir)

    locked = runner.run("lock", check=False)
    assert locked.returncode == 0, (
        f"an excluded reference must still resolve at lock time; "
        f"got {locked.returncode}; stderr: {locked.stderr}"
    )
    assert f"{REGISTRY_HOST}/{ns}/hidden/secret-skill@sha256:" in (
        project_dir / "grimoire.lock"
    ).read_text(), "the excluded artifact must be pinned by digest in the lock"

    installed = runner.run("install", check=False)
    assert installed.returncode == 0, (
        f"an excluded reference must still install; got {installed.returncode}; "
        f"stderr: {installed.stderr}"
    )
    assert_dir_exists(project_dir / ".claude" / "skills" / "secret-skill")

    assert _visible_candidates(runner, ns) == {"keeper"}, (
        "control: the exclude must genuinely hide the package from browsing, "
        "or nothing above was actually tested"
    )


def test_malformed_filter_pattern_in_project_config_exits_78(
    grim_at, project_dir: Path
) -> None:
    """An uncompilable pattern in `grimoire.toml` is exit 78, quoting it (S-015).

    Same contract as the malformed *global* config above, at project scope:
    a config grim cannot validate is `EX_CONFIG`, and the message names the
    offending pattern so the user can find it without bisecting the file.
    """
    (project_dir / "grimoire.toml").write_text(
        "[[registries]]\n"
        'alias = "acme"\n'
        f'oci = "{REGISTRY_HOST}"\n'
        'include = ["acme{unclosed"]\n'
        "default = true\n"
    )

    runner = grim_at(project_dir)
    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"an uncompilable browse pattern must exit 78 (EX_CONFIG); "
        f"got {result.returncode}; stderr: {result.stderr}"
    )
    assert "acme{unclosed" in result.stderr, (
        f"the diagnostic must quote the offending pattern; got: {result.stderr!r}"
    )


def test_one_file_may_declare_a_locator_twice_as_two_views(
    grim_at, project_dir: Path, registry: str
) -> None:
    """Two entries over ONE locator are two filtered views, and both browse.

    The dedup that used to collapse them predates per-entry browse filters:
    it dropped the second entry whole — alias, filter and all — so the rows
    only its `include` admitted were unreachable, and the loss was reported
    to stderr alone. Splitting a source into a wide view and a narrow one is
    exactly what the filters are for, so a repetition inside one file is
    honoured; only a global entry a project entry repeats is shadowed.
    """
    ns = f"grim-test/{uuid.uuid4().hex[:12]}"
    _publish_filter_tree(ns)

    (project_dir / "grimoire.toml").write_text(
        "[[registries]]\n"
        'alias = "platform"\n'
        f'oci = "{REGISTRY_HOST}/{ns}"\n'
        f'include = {_toml_list(_ns_rel(ns, "platform"))}\n'
        "default = true\n"
        "\n"
        "[[registries]]\n"
        'alias = "internal"\n'
        f'oci = "{REGISTRY_HOST}/{ns}"\n'
        f'include = {_toml_list(_ns_rel(ns, "internal"))}\n'
        "\n[skills]\n\n[rules]\n"
    )
    runner = grim_at(project_dir)

    context = runner.json("context")
    aliases = [r["alias"] for r in context["registries"]]
    assert aliases == ["platform", "internal"], (
        f"both entries must resolve, in declaration order; got {aliases}"
    )

    # The union of the two views: neither entry alone admits all four rows,
    # so this fails if either view was dropped or handed the other's filter.
    assert _visible_candidates(runner, ns) == {
        "platform/foo",
        "platform/foo/deep",
        "platform/bar",
        "internal/thing",
    }, "each view must contribute the rows its own include admits"
