# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Self-test of the settings test infrastructure.

`make_settings` pushes through `push_manifest`, never through grim, so a broken
builder or writer cannot mask a broken reader. `write_config` gains `settings=`
and `experimental_settings=`, and `settings_sandbox()` redirects every client
root. The last test proves a declared package is inert while the install side
is a stub.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from src.helpers import (
    SETTINGS_LAYER_MEDIA_TYPE,
    make_artifact,
    make_settings,
    settings_sandbox,
    write_config,
)
from src.registry import fetch_blob, fetch_manifest

DOCUMENT = {
    "description": "Team defaults",
    "claude": {"model": "sonnet", "permissions": {"deny": ["Bash(rm:*)"]}},
}


def test_make_settings_pushes_the_kinds_wire_shape(registry: str, unique_repo: str) -> None:
    pushed = make_settings(f"{unique_repo}/defaults", "1", DOCUMENT)

    manifest = fetch_manifest(f"{unique_repo}/defaults", "1")
    assert pushed.kind == "settings"
    assert manifest["annotations"] == {"com.grimoire.kind": "settings"}
    assert "artifactType" not in manifest
    assert [layer["mediaType"] for layer in manifest["layers"]] == [SETTINGS_LAYER_MEDIA_TYPE]
    assert SETTINGS_LAYER_MEDIA_TYPE == "application/vnd.grimoire.settings.v1+json"

    blob = fetch_blob(f"{unique_repo}/defaults", manifest["layers"][0]["digest"])
    assert json.loads(blob) == DOCUMENT
    assert blob.endswith(b"}\n")
    # Sorted keys: `claude` precedes `description` regardless of insertion order.
    assert blob.index(b'"claude"') < blob.index(b'"description"')


def test_make_settings_appends_a_requires_layer(registry: str, unique_repo: str) -> None:
    dep = make_artifact(f"{unique_repo}/dep", "rule", {"dep.md": "# dep\n"}, tag="1")
    make_settings(
        f"{unique_repo}/with-req", "1", DOCUMENT, requires=[("rule", "dep", dep.fq)]
    )

    manifest = fetch_manifest(f"{unique_repo}/with-req", "1")
    assert len(manifest["layers"]) == 2
    assert manifest["layers"][0]["mediaType"] == SETTINGS_LAYER_MEDIA_TYPE
    members = json.loads(fetch_blob(f"{unique_repo}/with-req", manifest["layers"][1]["digest"]))
    assert members == {"members": [{"kind": "rule", "name": "dep", "id": dep.fq}]}


def test_write_config_settings_section_and_flag(tmp_path: Path) -> None:
    plain = write_config(tmp_path, skills={"a": "r/a:1"}).read_text(encoding="utf-8")
    assert "[settings]" not in plain
    assert "experimental" not in plain

    declared = write_config(
        tmp_path, skills={"a": "r/a:1"}, settings={"defaults": "r/d:1"}, experimental_settings=True
    ).read_text(encoding="utf-8")
    assert '[settings]\ndefaults = "r/d:1"' in declared
    assert "[options.experimental]\nsettings = true" in declared
    # The skill-only prefix is unchanged by the new arguments.
    assert declared.startswith(plain.split("\n[options")[0].rstrip("\n"))

    off = write_config(tmp_path, experimental_settings=False).read_text(encoding="utf-8")
    assert "settings = false" in off


def test_settings_sandbox_seeds_every_settings_root(tmp_path: Path) -> None:
    home = tmp_path / "home"
    env, roots = settings_sandbox(tmp_path / "sandbox", home)

    assert set(roots) == {
        "amp",
        "antigravity",
        "claude",
        "codex",
        "copilot",
        "cursor",
        "droid",
        "gemini",
        "kilo",
        "kiro",
        "opencode",
        "qoder",
        "warp",
    }
    assert set(env) == {
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "COPILOT_HOME",
        "KIRO_HOME",
        "OPENCODE_CONFIG_DIR",
        "QODER_CONFIG_DIR",
    }
    for root in roots.values():
        assert root.is_dir()
        assert tmp_path in root.parents
    for value in env.values():
        assert Path(value).is_relative_to(tmp_path / "sandbox")
    assert roots["cursor"] == home / ".cursor"
    assert roots["amp"] == home / ".config" / "amp"
    assert roots["antigravity"] == home / ".gemini" / "antigravity-cli"
    assert roots["droid"] == home / ".factory"
    assert roots["kilo"] == home / ".config" / "kilo"
    assert roots["gemini"] == home / ".gemini"
    assert not (home / ".gemini" / "config").exists(), "CLI root suffices"
    warp = (".warp",) if sys.platform == "darwin" else (".config", "warp-terminal")
    assert roots["warp"] == home.joinpath(*warp)
    assert (home / ".warp").is_dir(), "Warp's detection marker"


def _tree(root: Path) -> list[str]:
    return sorted(p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file())


def test_settings_row_passes_the_stubs(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """A declared settings package installs nothing and exits 0, flag on or off.

    Positive control: a skill declared beside it does install, so the empty
    sandbox comes from the stub and not from an install that never ran.
    """
    package = make_settings(f"{unique_repo}/defaults", "1", DOCUMENT)
    skill = make_artifact(
        f"{unique_repo}/probe", "skill", {"probe/SKILL.md": "---\nname: probe\ndescription: probe\n---\n"}, tag="1"
    )
    runner = grim_at(project_dir)
    env, roots = settings_sandbox(tmp_path / "sandbox", runner.home)
    runner.env.update(env)
    (project_dir / ".claude").mkdir(exist_ok=True)

    write_config(
        project_dir,
        skills={"probe": skill.fq},
        settings={"defaults": package.fq},
        experimental_settings=True,
    )
    runner.run("lock")
    result = runner.run("install")
    assert result.returncode == 0, result.stderr

    installed = _tree(project_dir / ".claude")
    assert any(p.endswith("probe/SKILL.md") for p in installed), installed
    assert not any("settings" in p for p in installed), installed
    for client, root in roots.items():
        assert _tree(root) == [], (client, _tree(root))

    # The same declaration with the flag unset is just as inert.
    write_config(project_dir, settings={"defaults": package.fq})
    runner.run("lock")
    assert runner.run("install").returncode == 0
    assert not (project_dir / ".claude" / "settings.local.json").exists()
    for client, root in roots.items():
        assert _tree(root) == [], (client, _tree(root))
