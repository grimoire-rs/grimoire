# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim config registry` acceptance tests.

Additional coverage (review-fix round 1):
- Security regression: injection payloads in URL / default_registry rejected or
  produce no extra registries in the written config.
- D2: unset registry.<alias> removes the whole entry; set registry.<alias>.default
  true; unset registry.<alias>.oci exits 64.

Specification-phase suite: every test expresses expected behavior from
``adr_grim_config_command.md`` and ``plan_grim_config.md``.  All tests
FAIL against the Phase-3 stubs (``run`` body is ``unimplemented!()``).

Behaviors covered:
- ``registry add`` / ``list`` / ``show`` / ``use`` / ``rm`` lifecycle.
- At-most-one-default invariant: ``registry use <alias>`` clears all prior
  defaults, leaving exactly one.
- ``registry add`` with a duplicate alias exits 64 (UsageError).
- ``registry rm`` / ``show`` / ``use`` with a missing alias exits 64.
- Dotted ``config get registry.<alias>.oci`` returns the URL.
- Dotted ``config set registry.<alias>.oci`` on a MISSING alias exits 64
  (creation is ``registry add`` only — keeps validation in one path).
- End-to-end: ``config registry add`` at project scope then ``grim add``
  with an alias-qualified ref resolves and installs the artifact.
- End-to-end (global): ``--global registry add corp --oci <host>`` writes
  to ``$GRIM_HOME/grimoire.toml``.
"""
from __future__ import annotations

import os
import tomllib  # stdlib (Python 3.11+)
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _minimal_global_config(grim_home: Path) -> None:
    """Write a minimal valid ``grimoire.toml`` in ``$GRIM_HOME``."""
    (grim_home / "grimoire.toml").write_text("[skills]\n\n[rules]\n")


# ---------------------------------------------------------------------------
# Lifecycle tests
# ---------------------------------------------------------------------------


def test_registry_add_then_list_shows_entry(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry add`` adds an entry visible in ``registry list``.

    After adding ``acme`` with URL ``ghcr.io/acme``, ``registry list``
    must return a JSON array that contains an entry with alias ``acme``
    and the configured URL.

    Traces to ADR: ``grim config registry add <alias> --oci <ref>``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    result = runner.json("config", "registry", "list")["items"]

    assert isinstance(result, list), (
        f"registry list --format json must return a JSON array; got: {type(result)}"
    )
    aliases = [r.get("alias") for r in result if isinstance(r, dict)]
    assert "acme" in aliases, (
        f"registry list must include 'acme' after add; aliases: {aliases}"
    )
    urls = [r.get("oci") for r in result if isinstance(r, dict)]
    assert "ghcr.io/acme" in urls, (
        f"registry list must include the configured URL; urls: {urls}"
    )


def test_registry_add_legacy_url_flag_and_key_still_work(
    grim_at: object,
    project_dir: Path,
) -> None:
    """0.6.x back-compat: ``--url`` is a hidden alias for ``--oci`` and the
    dotted field ``registry.<alias>.url`` maps to ``oci``.

    End-to-end alias proof: add via the legacy flag, read back via both the
    new and the legacy dotted key.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "legacy", "--url", "ghcr.io/legacy")

    new_key = runner.plain("config", "get", "registry.legacy.oci")
    assert new_key.stdout.strip() == "ghcr.io/legacy", (
        f"registry.legacy.oci must return the value added via --url; got: {new_key.stdout!r}"
    )
    old_key = runner.plain("config", "get", "registry.legacy.url")
    assert old_key.stdout.strip() == "ghcr.io/legacy", (
        f"legacy dotted key registry.legacy.url must keep working; got: {old_key.stdout!r}"
    )


def test_registry_add_then_show_returns_fields(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry show <alias>`` returns all fields for a specific registry.

    The JSON object must contain ``alias``, ``oci``, and ``default``.

    Traces to ADR: ``grim config registry show <alias>`` → one-row table
    (Alias | Type | Source | Default); JSON ``{"alias":"…","oci":"…","default":bool}``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    result = runner.json("config", "registry", "show", "acme")

    assert result.get("alias") == "acme", (
        f"show must return alias='acme'; got: {result!r}"
    )
    assert result.get("oci") == "ghcr.io/acme", (
        f"show must return the configured URL; got: {result!r}"
    )
    assert "default" in result, (
        f"show must include the 'default' field; got: {result!r}"
    )
    # Always-present-null: the unused locator key is present and null.
    assert "index" in result, (
        f"show must include the 'index' key even for an oci entry; got: {result!r}"
    )
    assert result["index"] is None, (
        f"'index' must be explicit null for an oci entry; got: {result!r}"
    )


def test_registry_use_transfers_default_at_most_one(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry use <alias>`` makes exactly one registry the default.

    After ``registry add acme --default`` and ``registry add corp``,
    ``registry use corp`` must set ``corp`` as the default and clear
    ``acme``'s default flag.  At most one entry may carry ``default=true``.

    Traces to ADR: "``registry use`` sets target ``default=true`` and
    clears it on all others (enforces at-most-one before write)".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme", "--default",
    )
    runner.run(
        "config", "registry", "add", "corp",
        "--oci", "registry.corp/team",
    )

    runner.run("config", "registry", "use", "corp")

    result = runner.json("config", "registry", "list")["items"]
    assert isinstance(result, list), "registry list must return a JSON array"

    defaults = [r for r in result if r.get("default") is True]
    assert len(defaults) == 1, (
        f"exactly one registry must be the default after 'use'; "
        f"got {len(defaults)} defaults: {defaults}"
    )
    assert defaults[0].get("alias") == "corp", (
        f"'corp' must be the new default; got: {defaults[0]!r}"
    )


# ---------------------------------------------------------------------------
# Error-path tests (UsageError 64)
# ---------------------------------------------------------------------------


def test_registry_add_duplicate_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry add`` with an already-existing alias exits 64 (UsageError).

    To change a registry URL, use dotted ``set registry.<alias>.oci`` or
    ``rm`` + ``add`` — there is no overwrite/upsert verb.

    Traces to ADR: "``registry add`` of an existing alias → UsageError 64".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    result = runner.run(
        "config", "registry", "add", "acme", "--oci", "ghcr.io/acme-v2",
        check=False,
    )
    assert result.returncode == 64, (
        f"duplicate registry add must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_registry_rm_removes_entry_then_show_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry rm`` removes the entry; subsequent ``show`` exits 64.

    After ``rm``, the alias is gone: ``show`` must return UsageError 64,
    not a stale result.

    Traces to ADR: ``grim config registry rm <alias>``; "alias not found
    → UsageError 64".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")
    runner.run("config", "registry", "rm", "acme")

    result = runner.run("config", "registry", "show", "acme", check=False)
    assert result.returncode == 64, (
        f"show after rm must exit 64 (alias no longer exists); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_registry_show_missing_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry show`` with an unknown alias exits 64 (UsageError).

    Traces to ADR: "Registry alias not found on get/show/rm/use … →
    UsageError 64".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "registry", "show", "nonexistent", check=False
    )
    assert result.returncode == 64, (
        f"show of missing alias must exit 64; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_registry_rm_missing_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry rm`` with an unknown alias exits 64 (UsageError).

    Traces to ADR: "Registry alias not found on get/show/rm/use … →
    UsageError 64".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "registry", "rm", "nonexistent", check=False
    )
    assert result.returncode == 64, (
        f"rm of missing alias must exit 64; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_registry_use_missing_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry use`` with an unknown alias exits 64 (UsageError).

    Traces to ADR: "Registry alias not found on get/show/rm/use … →
    UsageError 64".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "registry", "use", "nonexistent", check=False
    )
    assert result.returncode == 64, (
        f"use of missing alias must exit 64; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# Dotted-key access to registry fields
# ---------------------------------------------------------------------------


def test_dotted_get_registry_alias_url_returns_url(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config get registry.<alias>.oci`` returns the URL via dotted-key.

    After ``registry add acme --oci ghcr.io/acme``, the dotted key
    ``registry.acme.oci`` must return ``ghcr.io/acme`` and exit 0.

    Traces to ADR: key-namespace table row ``registry.<alias>.oci``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    result = runner.plain("config", "get", "registry.acme.oci")
    assert result.returncode == 0, (
        f"dotted get of registry URL must exit 0; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "ghcr.io/acme" in result.stdout, (
        f"dotted get must return the URL; got: {result.stdout!r}"
    )


def test_dotted_set_registry_url_on_missing_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set registry.<alias>.oci`` on an absent alias exits 64.

    Registry entries are created ONLY via ``registry add`` to keep the
    url-required validation invariant in one path.  A dotted ``set`` on a
    non-existent alias must not silently create a half-built entry.

    Traces to ADR: "Dotted ``set registry.<alias>.<field>`` requires the
    entry to already exist … create only via ``registry add``".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "registry.phantom.oci", "ghcr.io/phantom",
        check=False,
    )
    assert result.returncode == 64, (
        f"dotted set on missing alias must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# End-to-end: config registry add → grim add resolves alias
# ---------------------------------------------------------------------------


def test_registry_add_then_grim_add_resolves_alias(
    grim_at: object,
    project_dir: Path,
    registry: str,
    unique_repo: str,
) -> None:
    """A registry added via ``grim config`` is used to resolve alias refs.

    After ``config registry add corp --oci <host>/<ns> --default``, a
    ``grim add corp/<repo>:tag`` must expand the alias to the full URL
    and succeed.

    This end-to-end path tests that:
    1. ``grim config registry add`` correctly writes the ``[[registries]]``
       entry into ``grimoire.toml``.
    2. The existing alias-expansion logic in ``grim add`` works with entries
       written by the config command (not only hand-authored configs).

    Traces to ADR: end-to-end test scenario in Testing Strategy section.
    """
    ns = unique_repo
    make_artifact(
        f"{ns}/corp-skill",
        "skill",
        {
            "corp-skill/SKILL.md": (
                "---\nname: corp-skill\ndescription: config e2e\n---\n# corp\n"
            )
        },
        tag="v1",
    )

    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    # Register the test registry under the "corp" alias.
    runner.run(
        "config", "registry", "add", "corp",
        "--oci", f"{REGISTRY_HOST}/{ns}", "--default",
    )

    # Add via the alias-qualified reference; grim must expand corp → the URL.
    out = runner.json("add", "corp/corp-skill:v1")

    assert out.get("status") == "added", (
        f"grim add via config-registered alias must succeed; got: {out!r}"
    )
    assert out.get("kind") == "skill", (
        f"kind must be 'skill'; got: {out!r}"
    )


# ---------------------------------------------------------------------------
# End-to-end (global scope)
# ---------------------------------------------------------------------------


def test_global_registry_add_writes_grim_home_config(
    grim_binary: Path,
    grim_home: Path,
) -> None:
    """``--global registry add`` writes the entry to ``$GRIM_HOME/grimoire.toml``.

    The written config must contain the alias name and the configured URL
    so that subsequent global-scope commands (lock, install) can use it.

    Traces to ADR: ``--global`` flag selects ``$GRIM_HOME/grimoire.toml``;
    "``--global registry add corp --oci <REGISTRY_HOST>`` then a short-id
    resolves against it".
    """
    _minimal_global_config(grim_home)
    runner = GrimRunner(grim_binary, grim_home)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    runner.run(
        "config", "--global", "registry", "add", "corp",
        "--oci", f"{REGISTRY_HOST}/grim-test/e2e", "--default",
    )

    cfg_text = (grim_home / "grimoire.toml").read_text()
    assert "corp" in cfg_text, (
        f"$GRIM_HOME/grimoire.toml must contain the alias 'corp'; "
        f"got:\n{cfg_text}"
    )
    assert REGISTRY_HOST in cfg_text, (
        f"$GRIM_HOME/grimoire.toml must contain the registry host; "
        f"got:\n{cfg_text}"
    )


# ---------------------------------------------------------------------------
# Security regression (S1 + S3): injection payloads must be rejected or
# produce no extra registries in the written config.
# ---------------------------------------------------------------------------


def _count_registries(cfg_path: Path) -> int:
    """Return the number of ``[[registries]]`` entries in the parsed config."""
    with cfg_path.open("rb") as f:
        data = tomllib.load(f)
    return len(data.get("registries", []))


def test_injection_via_registry_add_url_is_rejected_or_harmless(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A ``[[registries]]`` injection payload in a registry URL must not
    produce extra entries in the written ``grimoire.toml``.

    Either the command rejects the value (exit 65 for control chars) OR the
    written file round-trips to the same registry count.  In either case,
    ``registry list`` must not reveal any additional (injected) registry.

    Traces to S1 + S3: TOML-escape all written string values;
    reject control characters in URLs before writing.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    # Payload that would create a second [[registries]] block if not escaped.
    injection = 'legit.io"\n[[registries]]\nalias="evil"\nurl="attacker.io"'

    result = runner.run(
        "config", "registry", "add", "probe",
        "--oci", injection,
        check=False,
    )

    cfg_path = project_dir / "grimoire.toml"
    if result.returncode == 0:
        # Command accepted the value; the file must still parse cleanly and
        # contain exactly one registry (the one we added).
        try:
            count = _count_registries(cfg_path)
        except tomllib.TOMLDecodeError as exc:
            raise AssertionError(
                f"grimoire.toml is invalid TOML after injection attempt:\n"
                f"{cfg_path.read_text()}\nError: {exc}"
            ) from exc
        assert count == 1, (
            f"injection must not create extra [[registries]] entries; "
            f"got {count} entries.\nConfig:\n{cfg_path.read_text()}"
        )
    else:
        # Command rejected the value (exit 65 = DataError expected for control chars).
        assert result.returncode == 65, (
            f"injection URL with control chars must exit 65 (DataError); "
            f"got {result.returncode}; stderr: {result.stderr.strip()}"
        )


def test_injection_via_set_default_registry_is_rejected(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A ``default_registry`` injection payload must not corrupt ``grimoire.toml``.

    Traces to S1 + S3: ``options.default_registry`` is TOML-escaped on write;
    control characters in the value are rejected before writing.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    injection = 'legit.io"\ndefault_registry = "pwned'

    result = runner.run(
        "config", "set", "options.default_registry", injection,
        check=False,
    )

    cfg_path = project_dir / "grimoire.toml"
    if result.returncode == 0:
        try:
            with cfg_path.open("rb") as f:
                parsed = tomllib.load(f)
        except tomllib.TOMLDecodeError as exc:
            raise AssertionError(
                f"grimoire.toml is invalid TOML after injection: {exc}\n"
                f"Content:\n{cfg_path.read_text()}"
            ) from exc
        options = parsed.get("options", {})
        assert options.get("default_registry") != "pwned", (
            "injection must not set default_registry to 'pwned'"
        )
    else:
        assert result.returncode == 65, (
            f"injection value with newline must exit 65 (DataError); "
            f"got {result.returncode}"
        )


# ---------------------------------------------------------------------------
# D2: dotted unset registry.<alias> removes whole entry
# ---------------------------------------------------------------------------


def test_unset_registry_alias_removes_whole_entry(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config unset registry.<alias>`` removes the entire registry entry.

    After ``registry add acme`` and ``unset registry.acme``, the
    ``registry list`` output must not contain acme.

    Traces to D2: ``unset registry.<alias>`` removes the whole entry.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    runner.run("config", "unset", "registry.acme")

    result = runner.json("config", "registry", "list")["items"]
    aliases = [r.get("alias") for r in result if isinstance(r, dict)]
    assert "acme" not in aliases, (
        f"unset registry.acme must remove the entry; remaining aliases: {aliases}"
    )


# ---------------------------------------------------------------------------
# FIX B regression: injection via grim add tag must not create [[registries]]
# ---------------------------------------------------------------------------


def test_injection_via_grim_add_tag_is_rejected(
    grim_at: object,
    project_dir: Path,
) -> None:
    """An injection payload in a ``grim add`` tag must not produce extra
    ``[[registries]]`` entries in ``grimoire.toml``.

    Before FIX B the four artifact-table VALUE writes in ``write_config``
    used raw ``"{name} = \\"{id}\\""`` formatting.  A tag containing a
    literal ``"`` + newline could inject additional TOML sections, e.g.::

        tag"
        [[registries]]
        alias="evil"
        url="attacker.io"
        # <rest of tag>

    The fix wraps every value through ``toml::Value::String(id.to_string())``
    so the output is always properly TOML-escaped.  Normal identifiers are
    byte-identical after escaping, so existing round-trip tests still pass.

    Traces to FIX B: all four artifact-table writes in write_config are
    TOML-escaped.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    # Injection payload in the tag position — would create a second [[registries]]
    # block if the value were written unescaped.
    injection_ref = 'ghcr.io/x/y:tag"\n[[registries]]\nalias="evil"\nurl="attacker.io"\n# '

    result = runner.run(
        "add", injection_ref, "--kind", "skill",
        check=False,
    )

    cfg_path = project_dir / "grimoire.toml"
    if cfg_path.exists():
        try:
            with cfg_path.open("rb") as f:
                data = tomllib.load(f)
        except Exception as exc:
            raise AssertionError(
                f"grimoire.toml is invalid TOML after injection attempt — "
                f"injection succeeded:\n{cfg_path.read_text()}\nError: {exc}"
            ) from exc
        registry_count = len(data.get("registries", []))
        assert registry_count == 0, (
            f"injection must not create [[registries]] entries; "
            f"got {registry_count}.\nConfig:\n{cfg_path.read_text()}"
        )
    else:
        # No config file written — command must have exited non-zero.
        assert result.returncode != 0, (
            f"grim add with injection payload must either reject (non-zero) "
            f"or write a clean config with 0 registries; "
            f"got exit {result.returncode} with no config file"
        )


# ---------------------------------------------------------------------------
# D2: set registry.<alias>.default true
# ---------------------------------------------------------------------------


def test_set_registry_alias_default_true(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set registry.<alias>.default true`` marks the registry as default.

    Traces to D2: ``set registry.<alias>.default true``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")
    runner.run("config", "registry", "add", "corp", "--oci", "ghcr.io/corp")

    runner.run("config", "set", "registry.acme.default", "true")

    result = runner.json("config", "registry", "list")["items"]
    defaults = [r for r in result if r.get("default") is True]
    assert len(defaults) == 1, (
        f"exactly one registry must be the default; got: {defaults!r}"
    )
    assert defaults[0].get("alias") == "acme", (
        f"acme must be the default after set; got: {defaults[0]!r}"
    )


# ---------------------------------------------------------------------------
# D2: unset registry.<alias>.oci exits 64
# ---------------------------------------------------------------------------


def test_unset_registry_alias_url_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config unset registry.<alias>.oci`` exits 64 (UsageError).

    The URL is required for a registry entry; it cannot be unset without
    removing the whole entry via ``registry rm`` or ``unset registry.<alias>``.

    Traces to D2: ``unset registry.<alias>.oci`` → exit 64.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    result = runner.run(
        "config", "unset", "registry.acme.oci", check=False
    )
    assert result.returncode == 64, (
        f"unset registry.acme.oci must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# FIX 1: malformed alias exits 64, not 78
# ---------------------------------------------------------------------------


def test_registry_add_slash_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry add`` with a slash in the alias exits 64 (UsageError), not 78.

    Before FIX 1, ``registry add "a/b" --oci x`` pushed the entry and let
    ``validate_registries`` reject it → ``RegistryInvalid`` → exit 78
    (ConfigError).  A bad CLI argument must exit 64 (UsageError).

    Traces to FIX 1: pre-validate alias at command boundary before building
    the entry.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "registry", "add", "a/b", "--oci", "ghcr.io/test",
        check=False,
    )
    assert result.returncode == 64, (
        f"alias with '/' must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# B3: registry fields — static per-field metadata, no config required
# ---------------------------------------------------------------------------


def test_registry_fields_works_without_config(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config registry fields`` lists the 6 addressable per-registry field
    names and their metadata, and works in a directory with no
    ``grimoire.toml`` at all — it is static metadata, not a config read.

    Traces to B3: ``run_registry_fields`` takes no ctx, no scope resolve,
    no lock; must work outside any project.
    """
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    # Deliberately no write_config(project_dir) — the directory has no
    # grimoire.toml, and grim_home has no global config either.

    result = runner.json("config", "registry", "fields")
    items = result["items"]

    assert len(items) == 6, (
        f"registry fields must list exactly the 6 per-registry fields; got: {items!r}"
    )
    keys = [i.get("key") for i in items]
    # `RegistryField::ALL` is documented append-only with frozen positions,
    # and the `default` index assertion below depends on that, so a new
    # field never lands mid-list.
    assert keys == [
        "oci",
        "index",
        "default",
        "include",
        "exclude",
        "insecure",
    ], (
        f"fields must be oci, index, default, include, exclude, insecure "
        f"in that order; got: {keys!r}"
    )
    # The browse filters were appended after `default` partly to keep this
    # index stable.
    default_field = items[2]
    assert default_field.get("type") == "boolean", (
        f"'default' field's type must be 'boolean'; got: {default_field!r}"
    )
    # Both browse filters are string lists — the schema and the JSON shape
    # ARE arrays of strings, even though the CLI `set` write path stores
    # exactly one pattern (a comma is glob alternation, never a separator).
    by_key = {i["key"]: i for i in items}
    for field in ("include", "exclude"):
        assert by_key[field].get("type") == "string-list", (
            f"'{field}' must be typed as a string list; got: {by_key[field]!r}"
        )
    assert by_key["insecure"].get("type") == "boolean", (
        f"'insecure' field's type must be 'boolean'; got: {by_key['insecure']!r}"
    )


# ---------------------------------------------------------------------------
# FIX 2: dotted aliases stay addressable via dotted key
# ---------------------------------------------------------------------------


def test_dotted_alias_roundtrips_via_get(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry add a.b`` then ``config get registry.a.b.oci`` returns the URL.

    Before FIX 2, ``parse_key`` split at the FIRST dot, so
    ``registry.a.b.oci`` → alias=``a``, field=``b.url`` → unknown field
    error.  After fixing parse_key to split at the RIGHTMOST dot, aliases
    containing dots are fully addressable.

    Traces to FIX 2: parse_key splits at rightmost dot for registry keys.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "a.b", "--oci", "ghcr.io/dotted")

    result = runner.plain("config", "get", "registry.a.b.oci")
    assert result.returncode == 0, (
        f"config get registry.a.b.oci must exit 0; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "ghcr.io/dotted" in result.stdout, (
        f"get must return the URL; got: {result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# Per-registry browse filters (`include` / `exclude`)
# ---------------------------------------------------------------------------


def test_registry_add_accumulates_repeated_filter_flags(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry add --include x --include y`` keeps both patterns (S-008).

    ``registry add`` is the only CLI path that writes a multi-pattern list,
    and it is the shareable one-liner the feature exists for.  Repeating a
    flag accumulates; the flags do not replace each other, and neither one is
    ever comma-split — a comma is glob alternation syntax, so a value
    containing one must survive as a single pattern.

    ``--format json`` is the authoritative shape: it returns the true array,
    unlike the comma-joined display value ``config get`` renders.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        "--include", "acme/platform/**",
        "--include", "acme/{tools,labs}/**",
        "--exclude", "acme/platform/legacy/**",
    )

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["acme/platform/**", "acme/{tools,labs}/**"], (
        f"repeated --include flags must accumulate in order, and brace "
        f"alternation must survive intact; got: {shown!r}"
    )
    assert shown["exclude"] == ["acme/platform/legacy/**"], (
        f"--exclude must land on the exclude side, not the include side; got: {shown!r}"
    )


def test_registry_add_never_comma_splits_a_filter_flag(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A comma inside one ``--include`` value is data, not a separator (C-013).

    ``--registry``'s house style is repeatable-or-comma-separated; the filter
    flags deliberately diverge, because splitting on a comma would make
    ``acme/{platform,tools}/**`` unwritable — and would split it into two
    fragments that fail glob compilation.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        "--include", "a,b",
    )

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["a,b"], (
        f"a comma must never split a filter flag into two patterns; got: {shown!r}"
    )


def test_set_registry_include_replaces_the_list_with_exactly_one_pattern(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set registry.<alias>.include`` stores one pattern, unsplit (S-007).

    ``set`` replaces the whole list with exactly one pattern.  This diverges
    from the ``StringList`` house comma-split (``options.clients``,
    ``options.tui.tree_separators``) on purpose: the value here is a glob, and
    a comma is alternation syntax.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        "--include", "stale/**",
        "--include", "also-stale/**",
    )
    runner.run("config", "set", "registry.acme.include", "acme/{platform,tools}/**")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["acme/{platform,tools}/**"], (
        f"set must replace the whole list with exactly one unsplit pattern; got: {shown!r}"
    )


def test_get_registry_include_on_an_empty_list_exits_1_with_no_output(
    grim_at: object,
    project_dir: Path,
) -> None:
    """An empty filter list reads as unset: exit 1, empty stdout (S-009, S-010).

    Empty and absent are the same state for these lists, so ``get`` must use
    the existing unset semantics rather than printing an empty string — a
    migration script distinguishes "unset" from "set to nothing" by the exit
    code, and ``""`` on stdout would look like a value.

    The same entry's ``registry show --format json`` must nonetheless carry
    both keys as ``[]``: an always-present empty array, never an absent key
    (S-010).
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    for field in ("include", "exclude"):
        result = runner.plain("config", "get", f"registry.acme.{field}", check=False)
        assert result.returncode == 1, (
            f"get on an empty {field} list must exit 1 (unset); "
            f"got {result.returncode}; stderr: {result.stderr.strip()}"
        )
        assert result.stdout == "", (
            f"an unset key must print nothing at all; got: {result.stdout!r}"
        )

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == [] and shown["exclude"] == [], (
        f"both filter keys must be present as empty arrays on an unfiltered "
        f"entry; got: {shown!r}"
    )


def test_set_registry_include_with_a_bad_pattern_writes_nothing(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A rejected pattern exits 65 and leaves ``grimoire.toml`` byte-unchanged (S-016).

    Three rejected shapes, one contract: an uncompilable glob, an empty value,
    and a whitespace-only value (which compiles to a valid glob matching
    nothing, so it would silently empty the browse set).  Each must fail at
    the CLI write boundary with ``DataError`` — 65, not the load-time 78 — and
    the file must be identical afterwards, not merely semantically equivalent.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        "--include", "acme/platform/**",
    )
    cfg_path = project_dir / "grimoire.toml"
    before = cfg_path.read_bytes()

    for bad in ("acme{unclosed", "", "   "):
        result = runner.run(
            "config", "set", "registry.acme.include", bad, check=False
        )
        assert result.returncode == 65, (
            f"a rejected browse pattern {bad!r} must exit 65 (DataError); "
            f"got {result.returncode}; stderr: {result.stderr.strip()}"
        )
        assert cfg_path.read_bytes() == before, (
            f"nothing may be written when {bad!r} is rejected; config is now:\n"
            f"{cfg_path.read_text()}"
        )

    # The surviving value is the one that was there before the failed writes.
    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["acme/platform/**"], (
        f"the pre-existing pattern must be untouched; got: {shown!r}"
    )


@pytest.mark.skipif(
    os.name == "nt",
    reason="66,560 bytes of --include flags exceeds the ~32,767-char Windows command line",
)
def test_registry_add_over_the_aggregate_pattern_budget_exits_65(
    grim_at: object,
    project_dir: Path,
) -> None:
    """The whole-list budget fails at the CLI boundary too — 65, not 78 (C-006).

    ``MAX_PATTERN_LIST_BYTES`` (64 KiB summed across one entry's list) is the
    one rejection ``registry add``'s per-pattern loop cannot see, because no
    single pattern is over any per-pattern cap.  Before this it surfaced only
    from ``commit_config`` → ``validate_registries``: exit **78**, naming the
    ``grimoire.toml`` path as the offender for a value that arrived through
    ``--include`` flags, on a file the user never edited.

    Every other ``registry add`` rejection is 65 (S-016), and the plan's error
    taxonomy is frozen for ``case $?`` consumers — one command returning 65 or
    78 depending on *which* cap it trips is the inconsistency this closes.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    cfg_path = project_dir / "grimoire.toml"
    before = cfg_path.read_bytes()

    # 65 x 1024 = 66560 bytes, over the 65536-byte list budget — while every
    # pattern is legal alone (1024 is the inclusive per-pattern cap), which is
    # exactly why the per-pattern loop cannot catch it.
    flags: list[str] = []
    for _ in range(65):
        flags += ["--include", "a" * 1024]

    result = runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        *flags,
        check=False,
    )
    assert result.returncode == 65, (
        f"an over-budget pattern LIST must exit 65 at the CLI write boundary, "
        f"like every other pattern rejected there; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "registry.acme.include" in result.stderr, (
        f"the message must name the CLI key the value arrived through, not the "
        f"config file; stderr: {result.stderr.strip()}"
    )
    assert cfg_path.read_bytes() == before, (
        f"nothing may be written when the list is rejected; config is now:\n"
        f"{cfg_path.read_text()}"
    )


def test_unset_registry_include_clears_the_list_and_keeps_the_entry(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``unset`` empties a filter list without removing the registry (C-012).

    Unlike ``oci``/``index`` — where clearing the last locator would leave the
    entry sourceless, so ``unset`` exits 64 — a filter has an empty state that
    means "unfiltered", which is a legitimate place to land.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        "--include", "acme/platform/**",
    )
    runner.run("config", "unset", "registry.acme.include")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == [], f"unset must clear the list; got: {shown!r}"
    assert shown["oci"] == "ghcr.io/acme", (
        f"the entry itself must survive an unset of one filter; got: {shown!r}"
    )


def test_load_registry_hostile_locator_exits_78_without_raw_escape(
    grim_at: object,
    project_dir: Path,
) -> None:
    """Authored ``[[registries]]`` content is never echoed raw at exit 78.

    Every ``RegistryInvalid`` arm quotes authored TOML back to stderr, so a
    cloned repo whose ``grimoire.toml`` carries ESC + ``[2J`` in a locator, or
    ``U+202E`` in an alias, would inject a terminal control sequence into the
    terminal of anyone running a config-loading command. Both bytes are
    authored via TOML backslash escapes because the parser rejects the raw
    forms — the escapes are the genuine vector.
    """
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    cfg = project_dir / "grimoire.toml"

    # ESC reaches the locator through the both-sources-set arm.
    cfg.write_text(
        '[[registries]]\noci = "\\u001b[2Jghcr.io/x"\nindex = "https://y"\n'
    )
    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"an invalid authored registry entry must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "\x1b" not in result.stderr, (
        f"error must not echo the raw ESC byte to the terminal; "
        f"got: {result.stderr!r}"
    )
    assert r"\u{1b}[2Jghcr.io/x" in result.stderr, (
        f"error must still name the offending locator, escaped; "
        f"got: {result.stderr!r}"
    )

    # U+202E is not a control character, so it survives the alias control-char
    # guard and reaches the duplicate-alias arm intact.
    cfg.write_text(
        '[[registries]]\nalias = "a\\u202Eb"\noci = "ghcr.io/x"\n\n'
        '[[registries]]\nalias = "a\\u202Eb"\noci = "ghcr.io/y"\n'
    )
    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"a duplicate authored alias must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "\u202e" not in result.stderr, (
        f"error must not echo the raw override to the terminal; "
        f"got: {result.stderr!r}"
    )
    assert r"a\u{202e}b" in result.stderr, (
        f"error must still name the offending alias, escaped; "
        f"got: {result.stderr!r}"
    )


def test_registry_add_duplicate_hostile_alias_exits_64_without_raw_override(
    grim_at: object,
    project_dir: Path,
) -> None:
    """The duplicate-alias message escapes a bidi override in the alias.

    ``U+202E`` is neither whitespace nor a control character, so no alias
    guard rejects it — a script that feeds an attacker-influenced name to
    ``registry add`` twice would otherwise reorder the caller's terminal
    line. This is the set-time twin of the load-time ``duplicate alias``
    message.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    alias = "a\u202eb"
    runner.run("config", "registry", "add", alias, "--oci", "ghcr.io/acme")

    result = runner.run(
        "config", "registry", "add", alias, "--oci", "ghcr.io/acme-v2",
        check=False,
    )
    assert result.returncode == 64, (
        f"duplicate registry add must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "\u202e" not in result.stderr, (
        f"error must not echo the raw override to the terminal; "
        f"got: {result.stderr!r}"
    )
    assert r"a\u{202e}b" in result.stderr, (
        f"error must still name the duplicated alias, escaped; "
        f"got: {result.stderr!r}"
    )


# ---------------------------------------------------------------------------
# `registry set` — in-place edit
# ---------------------------------------------------------------------------


def test_registry_set_accumulates_repeated_filter_flags(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry set --include x --include y`` grows the list after creation.

    The gap this verb closes: ``config set registry.<alias>.include`` writes
    exactly ONE pattern and replaces the whole list, so before ``set`` there
    was no way to author a multi-pattern filter on an entry that already
    existed.  Repeating a flag accumulates, and a comma inside one value is
    glob alternation that must survive as a single pattern — the same
    contract ``registry add`` holds.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")
    runner.run(
        "config", "registry", "set", "acme",
        "--include", "acme/platform/**",
        "--include", "acme/{tools,labs}/**",
        "--exclude", "acme/platform/legacy/**",
    )

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["acme/platform/**", "acme/{tools,labs}/**"], (
        f"repeated --include flags must accumulate in order, and brace "
        f"alternation must survive intact; got: {shown!r}"
    )
    assert shown["exclude"] == ["acme/platform/legacy/**"], (
        f"--exclude must land on the exclude side, not the include side; got: {shown!r}"
    )


def test_registry_set_leaves_fields_it_was_not_given(
    grim_at: object,
    project_dir: Path,
) -> None:
    """Patch semantics: an omitted flag leaves that field exactly as it was.

    A filter-only edit must not disturb the locator, the default flag, or the
    other filter side.  This is what makes the verb safe to call from a UI
    that only knows about the field the user touched.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme", "--default",
        "--include", "stale/**",
        "--exclude", "legacy/**",
    )
    runner.run("config", "registry", "set", "acme", "--include", "platform/**")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["platform/**"], (
        f"the named list is replaced wholesale; got: {shown!r}"
    )
    assert shown["exclude"] == ["legacy/**"], (
        f"the other filter side must be untouched; got: {shown!r}"
    )
    assert shown["oci"] == "ghcr.io/acme", (
        f"an omitted locator flag must leave the locator alone; got: {shown!r}"
    )
    assert shown["default"] is True, (
        f"an omitted --default must not clear the flag; got: {shown!r}"
    )


def test_registry_set_keeps_the_entry_in_place_so_the_default_cannot_move(
    grim_at: object,
    project_dir: Path,
) -> None:
    """Editing the first of several entries must not move the default.

    The regression this verb exists to prevent.  The previous remedy for a
    multi-pattern edit was ``registry rm`` + re-``add``, but ``add`` appends,
    so re-creating the FIRST entry moved it last — and with no entry
    declaring ``default``, ``resolve_registries`` falls back to "first entry
    wins".  A user editing a browse filter would have silently changed which
    registry short identifiers expand against.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    for alias in ("first", "second", "third"):
        runner.run("config", "registry", "add", alias, "--oci", f"ghcr.io/{alias}")

    before = runner.json("context")["default_registry"]
    runner.run("config", "registry", "set", "first", "--include", "platform/**")

    order = [r["alias"] for r in runner.json("config", "registry", "list")["items"]]
    assert order == ["first", "second", "third"], (
        f"an in-place edit must not reorder [[registries]]; got: {order!r}"
    )
    assert runner.json("context")["default_registry"] == before, (
        f"the implicit default must not move when an entry is edited; "
        f"was {before!r}"
    )


def test_registry_set_swaps_the_locator_kind(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``--index`` on an OCI entry swaps the kind, clearing ``oci``.

    ``config set registry.<alias>.index`` refuses this and tells the user to
    unset ``oci`` first.  Here the locator flag is the whole declaration, so
    there is nothing ambiguous to refuse.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")
    runner.run(
        "config", "registry", "set", "acme",
        "--index", "https://index.acme.internal",
    )

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["index"] == "https://index.acme.internal", (
        f"the new locator must be written; got: {shown!r}"
    )
    assert shown["oci"] is None, (
        f"swapping the kind must clear the other side; got: {shown!r}"
    )


def test_registry_set_default_leaves_exactly_one_default(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``--default`` holds the at-most-one invariant, like ``registry use``."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme", "--default",
    )
    runner.run("config", "registry", "add", "corp", "--oci", "registry.corp/team")

    runner.run("config", "registry", "set", "corp", "--default")

    items = runner.json("config", "registry", "list")["items"]
    defaults = [r for r in items if r.get("default") is True]
    assert len(defaults) == 1, (
        f"exactly one registry may carry the flag; got {len(defaults)}: {defaults}"
    )
    assert defaults[0].get("alias") == "corp", (
        f"'corp' must be the new default; got: {defaults[0]!r}"
    )


def test_registry_set_missing_alias_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry set`` on an alias that does not exist exits 64.

    ``set`` edits, it never creates — ``registry add`` stays the one path
    that declares an entry, so validation lives in one place.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "registry", "set", "ghost", "--include", "a/**",
        check=False,
    )
    assert result.returncode == 64, (
        f"set on a missing alias must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_registry_set_naming_no_field_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A ``set`` that names no field is refused rather than rewriting the file.

    It would take the lock, rewrite ``grimoire.toml`` and report a change that
    never happened.  The message routes the adjacent intent — clearing a
    filter — to ``config unset``, which is the one edit patch semantics
    deliberately cannot express.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")
    result = runner.run("config", "registry", "set", "acme", check=False)

    assert result.returncode == 64, (
        f"a set naming no field must exit 64 (UsageError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "grim config unset" in result.stderr, (
        f"the message must route clearing a filter to unset; "
        f"got: {result.stderr!r}"
    )
    # S-015 / C-015: the guard widened to admit a clear-only edit, so its
    # message must enumerate the clear flags too.  A message listing five
    # flags for a six-flag guard tells the user the clear route does not
    # exist.  The ``config unset`` sentence above stays — owner decision 9
    # keeps both clearing routes valid.
    for flag in (
        "--oci/--index",
        "--include",
        "--exclude",
        "--clear-include",
        "--clear-exclude",
        "--default",
    ):
        assert flag in result.stderr, (
            f"the guard's message must enumerate {flag}, or it contradicts "
            f"the guard it backs; got: {result.stderr!r}"
        )


def test_registry_set_with_a_bad_pattern_writes_nothing(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A rejected pattern exits 65 and leaves the entry exactly as it was.

    Validation runs before the lock, so the same exit-65 gate ``registry add``
    and ``config set`` share applies here — and a partially applied edit is
    never observable.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme", "--include", "good/**",
    )
    before = (project_dir / "grimoire.toml").read_text()

    result = runner.run(
        "config", "registry", "set", "acme", "--include", "acme{unclosed",
        check=False,
    )
    assert result.returncode == 65, (
        f"an invalid pattern must exit 65 (DataError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert (project_dir / "grimoire.toml").read_text() == before, (
        "a rejected pattern must leave grimoire.toml byte-identical"
    )


def test_registry_set_then_unset_clears_the_list_and_keeps_the_entry(
    grim_at: object,
    project_dir: Path,
) -> None:
    """Clearing a filter is ``config unset``, and it leaves the entry intact.

    ``set`` reads a flag given zero times as "leave it alone", so it cannot
    express an empty list.  The pair is the full editing surface: ``set`` to
    write patterns, ``unset`` to clear them.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")
    runner.run(
        "config", "registry", "set", "acme",
        "--include", "a/**", "--include", "b/**",
        "--exclude", "legacy/**",
    )
    runner.run("config", "unset", "registry.acme.include")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == [], f"unset must clear the list; got: {shown!r}"
    assert shown["exclude"] == ["legacy/**"], (
        f"unset must not disturb the other side; got: {shown!r}"
    )
    assert shown["oci"] == "ghcr.io/acme", f"the entry survives; got: {shown!r}"


def test_registry_set_reports_registry_set_action(
    grim_at: object,
    project_dir: Path,
) -> None:
    """The write confirmation carries the new ``registry-set`` action.

    ``value`` is single-valued, so a filter-only edit reports ``null`` rather
    than echoing a locator the call never touched.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    doc = runner.json("config", "registry", "set", "acme", "--include", "a/**")
    assert doc["action"] == "registry-set", f"got: {doc!r}"
    assert doc["key"] == "registry.acme", f"got: {doc!r}"
    assert doc["value"] is None, (
        f"a filter-only edit has no single value to report; got: {doc!r}"
    )
    assert doc["dry_run"] is False, f"got: {doc!r}"

    doc = runner.json("config", "registry", "set", "acme", "--oci", "ghcr.io/other")
    assert doc["value"] == "ghcr.io/other", (
        f"a locator edit reports the new locator; got: {doc!r}"
    )


# ---------------------------------------------------------------------------
# `registry set --clear-include` / `--clear-exclude` (design C-013 … C-021)
# ---------------------------------------------------------------------------


def _acme_with_both_filters(runner: GrimRunner) -> None:
    """Seed ``acme`` with BOTH filter lists, a locator and no default.

    Both sides are populated on purpose: the two clear flags travel as two
    adjacent booleans all the way down, so a fixture that filtered only one
    side could not tell them apart.
    """
    runner.run(
        "config", "registry", "add", "acme",
        "--oci", "ghcr.io/acme",
        "--include", "a/**", "--include", "b/**",
        "--exclude", "legacy/**",
    )


def test_registry_set_clear_include_keeps_the_other_side(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-012 — clearing one list leaves the other, the locator and the flag.

    ``--clear-include`` is the additive route to the one edit patch semantics
    cannot express: a flag given zero times means "leave it alone", so there
    was no way to say "make this list empty" without ``config unset``.  Both
    halves are asserted — the targeted list empties AND the other side
    survives — because asserting only "something cleared" cannot distinguish
    the two flags from each other.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    runner.run("config", "registry", "set", "acme", "--clear-include")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == [], f"--clear-include must empty include; got: {shown!r}"
    assert shown["exclude"] == ["legacy/**"], (
        f"the exclude list must survive --clear-include; got: {shown!r}"
    )
    assert shown["oci"] == "ghcr.io/acme", f"the locator is untouched; got: {shown!r}"
    assert shown["default"] is False, f"the default flag is untouched; got: {shown!r}"


def test_registry_set_clear_exclude_keeps_the_include_side(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-012, mirrored — ``--clear-exclude`` empties only the exclude list."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    runner.run("config", "registry", "set", "acme", "--clear-exclude")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["exclude"] == [], f"--clear-exclude must empty exclude; got: {shown!r}"
    assert shown["include"] == ["a/**", "b/**"], (
        f"the include list must survive --clear-exclude; got: {shown!r}"
    )


def test_registry_set_clear_and_include_together_exit_64_from_clap(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-013 — the flag pair is refused by clap, before any alias lookup.

    The alias is deliberately one that does not exist: ``conflicts_with``
    fires during parsing, so the user must get clap's conflict error rather
    than ``no registry 'ghost'``.  A handler-level check would report the
    alias first and hide the real mistake.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "registry", "set", "ghost",
        "--clear-include", "--include", "a/**",
        check=False,
    )
    assert result.returncode == 64, (
        f"--clear-include with --include must exit 64; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "no registry" not in result.stderr, (
        f"the conflict must fire before alias resolution; got: {result.stderr!r}"
    )
    assert "--clear-include" in result.stderr, (
        f"clap's error must name the conflicting flag; got: {result.stderr!r}"
    )


def test_registry_set_with_only_a_clear_flag_exits_0(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-014 — a clear IS a change, so the "nothing to change" guard lets it by.

    The guard exists so a ``set`` naming no field cannot take the lock and
    report a write that never happened.  A clear-only invocation names a field
    and must not be caught by it.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    result = runner.run(
        "config", "registry", "set", "acme", "--clear-include", check=False
    )
    assert result.returncode == 0, (
        f"a clear alone must be enough to act; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_registry_set_clear_include_twice_is_idempotent(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-016 — a clear of an already-empty list still exits 0 and changes nothing.

    Idempotence is what makes the flag safe to emit unconditionally from a UI
    that does not know the current list.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    runner.run("config", "registry", "set", "acme", "--clear-include")
    after_first = (project_dir / "grimoire.toml").read_text()

    result = runner.run(
        "config", "registry", "set", "acme", "--clear-include", check=False
    )
    assert result.returncode == 0, (
        f"a repeated clear must still exit 0; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert (project_dir / "grimoire.toml").read_text() == after_first, (
        "a repeated clear must leave grimoire.toml byte-identical"
    )


def test_registry_set_clear_writes_no_include_key_at_all(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-017 / C-018 — a cleared list round-trips as an ABSENT key.

    Not ``include = []``: the written entry is byte-identical to one that
    never carried the key, which is what makes a clear and a never-filtered
    entry indistinguishable to every downstream reader.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    runner.run("config", "registry", "set", "acme", "--clear-include")
    written = (project_dir / "grimoire.toml").read_text()

    assert "include" not in written, (
        f"a cleared list must leave no include key; got:\n{written}"
    )
    entry = tomllib.loads(written)["registries"][0]
    assert "include" not in entry, (
        f"and the parsed table must not carry the key either; got: {entry!r}"
    )
    assert entry["exclude"] == ["legacy/**"], (
        f"the other side must still be written; got: {entry!r}"
    )


def test_registry_set_default_only_keeps_the_include_list(
    grim_at: object,
    project_dir: Path,
) -> None:
    """S-018 / C-020 row 1 — patch semantics hold in the other direction.

    The scenario the surviving mutant destroys silently: with
    ``if !include.is_empty() {`` mutated to ``{``, the include arm runs on
    EVERY edit with an empty slice, so a ``--default``-only call wipes a
    committed browse filter at exit 0 with no diagnostic.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    runner.run("config", "registry", "set", "acme", "--default")

    shown = runner.json("config", "registry", "show", "acme")
    assert shown["include"] == ["a/**", "b/**"], (
        f"a --default-only edit must not touch include; got: {shown!r}"
    )
    assert shown["exclude"] == ["legacy/**"], (
        f"a --default-only edit must not touch exclude; got: {shown!r}"
    )
    assert shown["default"] is True, f"the flag it named did move; got: {shown!r}"


def test_registry_set_reports_one_fields_element_per_touched_field(
    grim_at: object,
    project_dir: Path,
) -> None:
    """C-021 assertions 3 + 4 over the real binary — the VS Code interface.

    ``fields`` carries one element per field the call touched, in
    ``RegistryField::ALL``'s frozen order (``oci, index, default, include,
    exclude``) — deliberately NOT the enum's declaration order (``oci, index,
    include, exclude, default``), which an implementation iterating the enum
    would produce.  A cleared field carries an explicit ``"cleared"`` action
    and NO ``value`` key: ``value``'s own ``null`` already means "not
    applicable to this verb", and a third nested meaning would overload it.

    Then reads the entry back with ``registry show`` and holds every claim in
    ``fields`` against it — the only assertion in the suite that ties the
    report to the file it describes.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    doc = runner.json(
        "config", "registry", "set", "acme",
        "--oci", "ghcr.io/moved",
        "--include", "x/**",
        "--clear-exclude",
        "--default",
    )
    assert doc["fields"] == [
        {"field": "oci", "action": "set", "value": "ghcr.io/moved"},
        {"field": "default", "action": "set", "value": True},
        {"field": "include", "action": "set", "value": ["x/**"]},
        {"field": "exclude", "action": "cleared"},
    ], f"got: {doc!r}"
    assert [f["field"] for f in doc["fields"]] != [
        "oci", "include", "exclude", "default",
    ], f"declaration order is the trap — iterate ALL, never the enum; got: {doc!r}"
    cleared = doc["fields"][-1]
    assert "value" not in cleared, (
        f"a cleared element must carry no value key at all — not null; "
        f"got: {cleared!r}"
    )

    # `fields` is re-derived from the flags rather than emitted at the write
    # sites, so nothing in the Rust suite ties it to what landed on disk —
    # the report tests never re-read the config and the config tests never
    # look at the report, so a divergence in either direction passes both.
    # Read the entry back and hold `fields`' own claims against it; a cleared
    # list claims the empty list.
    shown = runner.json("config", "registry", "show", "acme")
    claimed = {f["field"]: f.get("value", []) for f in doc["fields"]}
    for field, value in claimed.items():
        assert shown[field] == value, (
            f"`fields` claimed {field}={value!r} but the entry reads "
            f"{shown[field]!r}; got: {shown!r}"
        )
    assert shown["index"] is None, (
        f"an untouched field emits no element and must not have moved; got: {shown!r}"
    )


def test_registry_set_clear_only_reports_just_that_field(
    grim_at: object,
    project_dir: Path,
) -> None:
    """C-021 assertion 3 — the exact element a clear-only edit emits."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    _acme_with_both_filters(runner)

    doc = runner.json("config", "registry", "set", "acme", "--clear-exclude")
    assert doc["fields"] == [{"field": "exclude", "action": "cleared"}], (
        f"an untouched field emits nothing; got: {doc!r}"
    )


def test_config_set_carries_an_empty_fields_array(
    grim_at: object,
    project_dir: Path,
) -> None:
    """C-021 assertion 2 — ``fields`` is always present, ``[]`` on a released verb.

    Additive-safety under ``docs/src/stability.md`` depends on the key being
    present on EVERY write report, never absent: a consumer distinguishing
    "not applicable" from "older grim" reads the key, not its absence.
    ``subsystem-cli-api.md`` bans ``skip_serializing_if`` in ``src/api/`` for
    the same reason.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    doc = runner.json("config", "set", "registry.acme.include", "a/**")
    assert doc["fields"] == [], (
        f"a released verb must emit an empty fields array, never omit it; "
        f"got: {doc!r}"
    )


# ---------------------------------------------------------------------------
# `insecure` — plain-HTTP transport as a per-registry field
# ---------------------------------------------------------------------------


def test_registry_add_insecure_round_trips_through_show_and_list(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry add --insecure`` writes the field and reports it back.

    The extension's settings panel reads ``show``/``list``, so the flag has
    to be visible on both, and the written TOML has to carry it — an entry
    that only *reports* the opt-in would silently keep using HTTPS.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run(
        "config", "registry", "add", "local",
        "--oci", "localhost:5050/grimoire",
        "--insecure",
    )

    assert runner.json("config", "registry", "show", "local")["insecure"] is True
    listed = runner.json("config", "registry", "list")["items"]
    assert [r["insecure"] for r in listed] == [True], f"got: {listed!r}"

    doc = tomllib.loads((project_dir / "grimoire.toml").read_text())
    assert doc["registries"][0]["insecure"] is True, f"got: {doc!r}"


def test_registry_entry_without_the_flag_reports_insecure_false(
    grim_at: object,
    project_dir: Path,
) -> None:
    """The key is always present, ``false`` by default, and unset entries
    write no ``insecure`` line at all (additive-safety: an entry authored
    before the field existed stays byte-identical)."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    assert runner.json("config", "registry", "show", "acme")["insecure"] is False
    assert "insecure" not in (project_dir / "grimoire.toml").read_text()


def test_registry_set_insecure_and_no_insecure_toggle_the_field(
    grim_at: object,
    project_dir: Path,
) -> None:
    """Unlike ``--default``, the opt-in is revocable in place — a transport
    downgrade a user turned on has to be turnable off through the same verb
    the settings panel writes with."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "registry", "add", "local", "--oci", "localhost:5050/grimoire")

    doc = runner.json("config", "registry", "set", "local", "--insecure")
    assert doc["fields"] == [{"field": "insecure", "action": "set", "value": True}], (
        f"got: {doc!r}"
    )
    assert runner.json("config", "registry", "show", "local")["insecure"] is True

    doc = runner.json("config", "registry", "set", "local", "--no-insecure")
    assert doc["fields"] == [{"field": "insecure", "action": "set", "value": False}], (
        f"--no-insecure is a set to false, never a cleared; got: {doc!r}"
    )
    assert runner.json("config", "registry", "show", "local")["insecure"] is False


def test_registry_set_without_the_flags_leaves_insecure_alone(
    grim_at: object,
    project_dir: Path,
) -> None:
    """An unrelated edit must not silently restore HTTPS."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run(
        "config", "registry", "add", "local",
        "--oci", "localhost:5050/grimoire",
        "--insecure",
    )

    doc = runner.json("config", "registry", "set", "local", "--include", "acme/**")
    assert [f["field"] for f in doc["fields"]] == ["include"], f"got: {doc!r}"
    assert runner.json("config", "registry", "show", "local")["insecure"] is True


def test_dotted_insecure_key_round_trips_through_get_set_unset(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``registry.<alias>.insecure`` behaves like ``default``: a bool with no
    unset state, so ``get`` always answers and ``unset`` clears to false."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "registry", "add", "local", "--oci", "localhost:5050/grimoire")

    def read() -> str:
        return runner.plain("config", "get", "registry.local.insecure").stdout.strip()

    assert read() == "false"
    runner.run("config", "set", "registry.local.insecure", "true")
    assert read() == "true"
    runner.run("config", "unset", "registry.local.insecure")
    assert read() == "false"


def test_insecure_on_an_index_entry_is_rejected_everywhere(
    grim_at: object,
    project_dir: Path,
) -> None:
    """An index locator already carries its own ``http(s)://`` scheme, so the
    flag could only ever be a silent no-op there.

    Exit 65 at set time (``registry add`` / ``registry set`` / ``config
    set``), 78 at load time for a hand-written file — the same split the
    ``oci``/``index`` mutual exclusion already uses.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    r = runner.run(
        "config", "registry", "add", "hub",
        "--index", "https://index.grimoire.rs",
        "--insecure",
        check=False,
    )
    assert r.returncode == 65, f"rc={r.returncode} stderr={r.stderr!r}"
    assert "insecure" not in (project_dir / "grimoire.toml").read_text()

    runner.run("config", "registry", "add", "hub", "--index", "https://index.grimoire.rs")
    for args in (
        ("config", "registry", "set", "hub", "--insecure"),
        ("config", "set", "registry.hub.insecure", "true"),
    ):
        r = runner.run(*args, check=False)
        assert r.returncode == 65, f"{args!r}: rc={r.returncode} stderr={r.stderr!r}"

    # Turning it back off stays open, so a mis-authored entry is repairable.
    runner.run("config", "registry", "set", "hub", "--no-insecure")

    # Hand-written file: the same pairing is a config error at load.
    (project_dir / "grimoire.toml").write_text(
        '[skills]\n\n[rules]\n\n[[registries]]\n'
        'index = "https://index.grimoire.rs"\ninsecure = true\n'
    )
    r = runner.run("config", "registry", "list", check=False)
    assert r.returncode == 78, f"rc={r.returncode} stderr={r.stderr!r}"


def test_insecure_alone_reaches_a_plain_http_registry(
    grim_at: object,
    project_dir: Path,
    registry: str,
    unique_repo: str,
) -> None:
    """The load-bearing end-to-end proof: with ``GRIM_INSECURE_REGISTRIES``
    removed from the environment, ``insecure = true`` alone is what makes
    grim contact the registry over plain HTTP.

    Skipped when the session registry is one of grim's implicit loopback
    forms — there the env var is not what the request depends on either, so
    the test could not distinguish a working config field from a broken one.
    Requires a non-default port, which the conftest picks whenever
    ``localhost:5000`` is unusable.
    """
    builtin = {"localhost", "localhost:5000", "127.0.0.1", "127.0.0.1:5000"}
    if registry in builtin:
        pytest.skip(
            f"{registry} is an implicit plain-HTTP host; run with "
            f"GRIM_TEST_REGISTRY_HOST=<host:non-5000-port> to exercise this"
        )

    ns = unique_repo
    make_artifact(
        f"{ns}/http-skill",
        "skill",
        {"http-skill/SKILL.md": "---\nname: http-skill\ndescription: transport e2e\n---\n# http\n"},
        tag="v1",
    )

    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)
    # The whole point: nothing but the config field may authorize plain HTTP.
    runner.env.pop("GRIM_INSECURE_REGISTRIES", None)

    runner.run(
        "config", "registry", "add", "corp",
        "--oci", f"{REGISTRY_HOST}/{ns}", "--default",
    )

    # Without the opt-in grim must attempt HTTPS against a plain-HTTP port.
    failed = runner.run("add", "corp/http-skill:v1", check=False)
    assert failed.returncode != 0, (
        f"an https attempt against a plain-http registry must fail; got: {failed!r}"
    )

    runner.run("config", "registry", "set", "corp", "--insecure")
    out = runner.json("add", "corp/http-skill:v1")
    assert out.get("status") == "added", (
        f"insecure = true alone must carry the request; got: {out!r}"
    )

    # `--registry` narrows the browse set; it must not narrow which hosts may
    # be reached over plain HTTP.  While the transport allowlist was derived
    # from the browse-set resolver, any `--registry` value silently disarmed
    # the opt-in and every network command failed against a plain-HTTP port.
    runner.run("remove", "skill", "http-skill")
    pinned = runner.json("add", f"{ns}/http-skill:v1", "--registry", REGISTRY_HOST)
    assert pinned.get("status") == "added", (
        f"--registry must not disarm a config-declared plain-HTTP host; got: {pinned!r}"
    )
