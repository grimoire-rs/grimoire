"""How grim 0.15.1 meets a project that uses the settings kind.

The settings kind is written by a newer grim; an older grim on the same machine
must refuse cleanly or ignore what it cannot see, never corrupt.  The binary
under test is the *old* one, given by ``GRIM_OLD_BIN`` (a grim 0.15.1 build);
the module skips when it is unset.  Every fixture is hand-written or pushed by
``make_settings``, so no new grim is involved and a broken writer cannot mask a
broken reader.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import (
    make_artifact,
    make_bundle,
    make_dependent,
    make_hook,
    make_settings,
    write_config,
)
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner

OLD_BIN = os.environ.get("GRIM_OLD_BIN")

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

SKILL = {"s/SKILL.md": "---\nname: s\ndescription: a skill\n---\n# s\n"}
SETTINGS_DOC = {"description": "team defaults", "claude": {"model": "sonnet"}}

_LOCK_HEAD = (
    "[metadata]\n"
    "lock_version = 1\n"
    "declaration_hash_version = 1\n"
    f'declaration_hash = "sha256:{"0" * 64}"\n'
    'generated_by = "grim 0.16.0"\n'
    'generated_at = "2026-01-01T00:00:00Z"\n\n'
)


@pytest.fixture()
def old(tmp_path: Path) -> GrimRunner:
    """The old grim, in a Claude-detected project with its own GRIM_HOME."""
    assert OLD_BIN is not None
    grim_home = tmp_path / "grim-home"
    grim_home.mkdir()
    project = tmp_path / "project"
    (project / ".claude").mkdir(parents=True)
    runner = GrimRunner(Path(OLD_BIN), grim_home, cwd=project)
    version = runner.run("--version").stdout
    assert "0.15." in version, f"GRIM_OLD_BIN is not a grim 0.15: {version!r}"
    # The version alone cannot tell 0.15.1 from a build of this branch, which still carries it.
    knows = runner.run("schema", "--kind", "settings", check=False).returncode == 0
    assert not knows, "GRIM_OLD_BIN knows the settings kind, so it is not a released grim 0.15"
    runner.run("init")
    return runner


def _project_bytes(old: GrimRunner) -> dict[str, bytes | None]:
    """The declaration and the lock, byte for byte, or None where absent."""
    files = (old.cwd / "grimoire.toml", old.cwd / "grimoire.lock")
    return {path.name: path.read_bytes() if path.exists() else None for path in files}


def test_old_grim_adds_a_plain_artifact(old, registry, unique_repo):
    """Positive control: the old binary installs an ordinary skill here."""
    skill = make_artifact(f"{unique_repo}/plain", "skill", SKILL, tag="1")

    old.run("add", skill.fq, "--name", "plain")

    assert (old.cwd / ".claude" / "skills" / "plain" / "SKILL.md").is_file()


def test_old_grim_rejects_a_lock_with_a_settings_table(old):
    ref = f"localhost:5000/x/a@sha256:{'1' * 64}"
    write_config(old.cwd, skills={"a": "localhost:5000/x/a:1"})
    (old.cwd / "grimoire.lock").write_text(
        _LOCK_HEAD
        + f'[[skill]]\nname = "a"\npinned = "{ref}"\n\n'
        + f'[[settings]]\nname = "s"\npinned = "localhost:5000/x/s@sha256:{"2" * 64}"\n'
    )

    result = old.run("status", check=False)

    assert result.returncode == 78, result.stderr
    assert "settings" in result.stderr and "invalid TOML" in result.stderr


def test_old_grim_rejects_a_config_with_a_settings_table(old):
    write_config(old.cwd, settings={"s": "localhost:5000/x/s:1"})

    result = old.run("status", check=False)

    assert result.returncode == 78, result.stderr
    assert "settings" in result.stderr


def test_old_grim_rejects_the_experimental_settings_key(old):
    write_config(old.cwd, skills={"a": "localhost:5000/x/a:1"}, experimental_settings=True)

    result = old.run("status", check=False)

    assert result.returncode == 78, result.stderr
    assert "settings" in result.stderr


def test_old_grim_rejects_a_bundle_with_a_settings_member(old, registry, unique_repo):
    """The member kind is the refusal's cause, and the project keeps its bytes."""
    settings = make_settings(f"{unique_repo}/s", "1", SETTINGS_DOC)
    bundle = make_bundle(f"{unique_repo}/b", [("settings", "s", settings.fq)], tag="1")
    before = _project_bytes(old)

    result = old.run("add", bundle.fq, "--name", "b", check=False)

    assert result.returncode == 65, result.stderr
    assert "unknown variant `settings`" in result.stderr
    assert _project_bytes(old) == before


def test_old_grim_rejects_a_requires_edge_to_settings(old, registry, unique_repo):
    """0.15.1 reads single-layer manifests only, so it refuses the requires layer
    with 65 whatever kind the edge names, and never parses the edge. The refusal
    comes after it has declared and locked the dependent, and before it writes
    anything for a client: that residual is pinned exactly."""
    settings = make_settings(f"{unique_repo}/s", "1", SETTINGS_DOC)
    parent = make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        SKILL,
        [("settings", "s", settings.fq)],
    )
    declaration = (old.cwd / "grimoire.toml").read_text()

    result = old.run("add", parent.fq, "--name", "parent", check=False)

    assert result.returncode == 65, result.stderr
    assert "expected a single-layer artifact, manifest has 2 layers" in result.stderr
    assert (old.cwd / "grimoire.toml").read_text() == declaration.replace(
        "[skills]\n", f'[skills]\nparent = "{parent.fq}"\n', 1
    )
    lock = (old.cwd / "grimoire.lock").read_text()
    assert lock.count("name = ") == 1 and f'pinned = "{parent.pinned}"' in lock, lock
    assert list((old.cwd / ".claude").iterdir()) == []


def test_old_grim_add_of_a_settings_layer_reads_no_kind(old, registry, unique_repo):
    """The layer's media type is unknown to 0.15.1, so it cannot infer a kind and
    leaves the project's bytes as they were."""
    settings = make_settings(f"{unique_repo}/s", "1", SETTINGS_DOC)
    before = _project_bytes(old)

    result = old.run("add", settings.fq, "--name", "s", check=False)

    assert result.returncode == 65, result.stderr
    assert "could not infer the kind" in result.stderr
    assert _project_bytes(old) == before


def _global_state(kind: str) -> str:
    """A global state file holding one record of `kind` with a settings-shaped output."""
    return json.dumps(
        {
            "version": 2,
            "records": [
                {
                    "kind": kind,
                    "name": "s",
                    "pinned": f"localhost:5000/x/s@sha256:{'3' * 64}",
                    "outputs": [
                        {
                            "client": "claude",
                            "target": {"anchor": "claude-root", "relative": "settings.json"},
                            "content_hash": f"sha256:{'4' * 64}",
                            "entry": "/model",
                        }
                    ],
                }
            ],
        }
    )


def test_old_grim_fails_on_a_global_state_holding_a_settings_record(old):
    """The record's kind is the cause: the same record as an mcp entry reads clean."""
    state = old.grim_home / "state" / "global.json"
    state.parent.mkdir(parents=True, exist_ok=True)
    state.write_text(_global_state("mcp"))
    control = old.run("status", "--global", check=False)
    assert control.returncode == 0, f"positive control: {control.stderr}"
    state.write_text(_global_state("settings"))
    before = state.read_text()

    result = old.run("status", "--global", check=False)

    assert result.returncode == 74, result.stderr
    assert "unknown variant `settings`" in result.stderr
    assert state.read_text() == before


def test_old_grim_hook_allow_drops_settings_consent_keys(old, registry, unique_repo):
    hook = make_hook(f"{unique_repo}/h", "h")
    write_config(old.cwd, hooks={"h": hook.fq})
    old.run("lock")
    old.run("hook", "allow")
    records = list((old.grim_home / "hooks" / "consent").glob("*.json"))
    assert len(records) == 1, records
    record = json.loads(records[0].read_text())
    hook_keys = [k for k in record["hooks"] if not k.startswith("settings:")]
    assert hook_keys, record  # positive control: allow recorded the hook
    settings_key = f"settings:s@{REGISTRY_HOST}/{unique_repo}/s"
    record["hooks"] = [*hook_keys, settings_key]
    records[0].write_text(json.dumps(record, indent=2) + "\n")

    old.run("hook", "allow")

    after = json.loads(records[0].read_text())
    assert after["hooks"] == hook_keys
