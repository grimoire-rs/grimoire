# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim add` default-registry acceptance tests.

The default registry is a pure CLI-input convenience: short references are
expanded against it, but the resolved fully-qualified name (registry host
included) is what gets persisted in both ``grimoire.toml`` and
``grimoire.lock``.  Two resolution sources are tested:

1. ``GRIM_DEFAULT_REGISTRY`` environment variable.
2. ``[options].default_registry`` in the project config.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_HOST


def test_add_env_default_registry_persists_fq_name(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Env-sourced default registry: config+lock carry the fully-qualified name."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)
    # Inject the default registry via env; use a SHORT reference (no host).
    runner.env["GRIM_DEFAULT_REGISTRY"] = REGISTRY_HOST

    short_ref = f"{unique_repo}/code-review:stable"
    out = runner.json("add", short_ref)
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "added"

    # Both config and lock must persist the FULLY-QUALIFIED name (host present).
    cfg_text = (project_dir / "grimoire.toml").read_text()
    assert f"{REGISTRY_HOST}/" in cfg_text, (
        f"grimoire.toml must contain the registry host '{REGISTRY_HOST}/', "
        f"got:\n{cfg_text}"
    )

    lock_text = (project_dir / "grimoire.lock").read_text()
    assert f"{REGISTRY_HOST}/" in lock_text, (
        f"grimoire.lock must contain the registry host '{REGISTRY_HOST}/', "
        f"got:\n{lock_text}"
    )


def test_add_env_default_registry_beats_config_default(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Registry precedence: ``GRIM_DEFAULT_REGISTRY`` wins over the config
    ``[options].default_registry``.

    The config declares a bogus host; the env names the real registry. The
    short reference must expand against the env value (so resolution succeeds
    and the persisted FQ name carries the real host), proving env beats config
    in the reordered precedence chain.
    """
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    wrong_host = "wrong-registry.invalid:5000"
    cfg_path = project_dir / "grimoire.toml"
    cfg_path.write_text(
        f'[options]\ndefault_registry = "{wrong_host}"\n\n[skills]\n\n[rules]\n'
    )

    runner = grim_at(project_dir)
    # The env names the REAL registry; it must win over the config default.
    runner.env["GRIM_DEFAULT_REGISTRY"] = REGISTRY_HOST

    short_ref = f"{unique_repo}/code-review:stable"
    out = runner.json("add", short_ref)
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "added"

    # The resolved skill binding must expand against the env (real) host,
    # proving env beats config. The bogus `default_registry` line still
    # round-trips in `[options]` (add preserves options), so assert on the
    # skill ENTRY line, not the whole file.
    cfg_text = (project_dir / "grimoire.toml").read_text()
    skill_line = next(
        (line for line in cfg_text.splitlines() if line.startswith("code-review")),
        "",
    )
    assert f"{REGISTRY_HOST}/" in skill_line, (
        f"the skill binding must use the env registry host '{REGISTRY_HOST}/', "
        f"got skill line: {skill_line!r}\nfull config:\n{cfg_text}"
    )
    assert wrong_host not in skill_line, (
        f"the bogus config registry '{wrong_host}' must not win on the skill "
        f"binding, got skill line: {skill_line!r}"
    )

    # The lock must record the env (real) host, and never the bogus one.
    lock_text = (project_dir / "grimoire.lock").read_text()
    assert f"{REGISTRY_HOST}/" in lock_text, (
        f"grimoire.lock must use the env registry host '{REGISTRY_HOST}/', got:\n{lock_text}"
    )
    assert wrong_host not in lock_text, (
        f"the bogus config registry '{wrong_host}' must not appear in the lock, got:\n{lock_text}"
    )


def test_add_config_default_registry_persists_fq_name(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Config-sourced default registry: config+lock carry the fully-qualified name.

    ``GRIM_DEFAULT_REGISTRY`` is NOT set; only the ``[options].default_registry``
    entry in ``grimoire.toml`` provides the default.
    """
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    # Write a grimoire.toml with [options].default_registry directly; the
    # write_config helper does not emit [options], so we write it manually.
    cfg_path = project_dir / "grimoire.toml"
    cfg_path.write_text(
        f'[options]\ndefault_registry = "{REGISTRY_HOST}"\n\n[skills]\n\n[rules]\n'
    )

    runner = grim_at(project_dir)
    # Deliberately do NOT set GRIM_DEFAULT_REGISTRY so the config option
    # is the only source.
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    short_ref = f"{unique_repo}/code-review:stable"
    out = runner.json("add", short_ref)
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "added"

    # Config after `grim add` re-serialises the declared set (registry host
    # must be in the skill entry).
    cfg_text = (project_dir / "grimoire.toml").read_text()
    assert f"{REGISTRY_HOST}/" in cfg_text, (
        f"grimoire.toml must contain the registry host '{REGISTRY_HOST}/', "
        f"got:\n{cfg_text}"
    )

    lock_text = (project_dir / "grimoire.lock").read_text()
    assert f"{REGISTRY_HOST}/" in lock_text, (
        f"grimoire.lock must contain the registry host '{REGISTRY_HOST}/', "
        f"got:\n{lock_text}"
    )


def test_add_short_id_honors_global_registries_array(
    grim_at, project_dir: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Regression (ADR G2/B1): a ``[[registries]]`` default declared only in
    the GLOBAL config must resolve a project-scope short id — the project
    config declares neither ``[[registries]]`` nor ``[options].default_registry``,
    so the only source of a default is the global config's array. Must not
    silently fall back to the built-in ``ghcr.io/grimoire-rs``."""
    make_artifact(
        f"{unique_repo}/global-tool",
        "skill",
        {"global-tool/SKILL.md": "---\nname: global-tool\ndescription: d\n---\n# G\n"},
        tag="1",
    )
    (grim_home / "grimoire.toml").write_text(f'[[registries]]\noci = "{REGISTRY_HOST}"\ndefault = true\n')
    write_config(project_dir)

    runner = grim_at(project_dir)
    runner.env.pop("GRIM_DEFAULT_REGISTRY", None)

    short_ref = f"{unique_repo}/global-tool:1"
    out = runner.json("add", short_ref)
    assert out["status"] == "added", (
        f"add must resolve via the global [[registries]] default, not fall back "
        f"to the built-in registry: {out!r}"
    )

    cfg_text = (project_dir / "grimoire.toml").read_text()
    assert f"{REGISTRY_HOST}/{unique_repo}/global-tool" in cfg_text, (
        f"the skill binding must use the global registry host '{REGISTRY_HOST}/', "
        f"got:\n{cfg_text}"
    )


def test_add_short_id_in_index_only_project_uses_default_chain(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Regression: an index-only ``[[registries]]`` set has no OCI primary,
    so a short reference must expand against the documented short-id chain
    (here ``GRIM_DEFAULT_REGISTRY``) — never persist a registry-less
    ``/name`` reference into ``grimoire.toml``."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    (project_dir / "grimoire.toml").write_text(
        '[[registries]]\n'
        'alias = "hub"\n'
        'index = "http://127.0.0.1:1/absent"\n'
        'default = true\n'
        '\n[skills]\n\n[rules]\n'
    )
    runner = grim_at(project_dir)
    runner.env["GRIM_DEFAULT_REGISTRY"] = REGISTRY_HOST

    short_ref = f"{unique_repo}/code-review:stable"
    out = runner.json("add", short_ref)
    assert out["status"] == "added"

    cfg_text = (project_dir / "grimoire.toml").read_text()
    assert f"{REGISTRY_HOST}/{unique_repo}/code-review" in cfg_text, (
        f"the binding must be fully qualified against the short-id default "
        f"chain, got:\n{cfg_text}"
    )
    assert '"/' not in cfg_text, (
        f"a registry-less reference must never be persisted, got:\n{cfg_text}"
    )
