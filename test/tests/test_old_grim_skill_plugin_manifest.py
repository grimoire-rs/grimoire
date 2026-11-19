"""A skill grim 0.15.1 shipped with a Claude plugin manifest directory at its root.

grim 0.15.1 packs and installs such a skill; this grim refuses it with exit 65
and the reason ``skill-plugin-manifest``.  A skill an older grim already
installed keeps its files until ``grim update`` or ``grim uninstall``, and
every install-type run says so instead of touching them.  The binary that
publishes and installs is the *old* one, given by ``GRIM_OLD_BIN`` (the v0.15.1
release binary, checked by ``src.old_grim``); the module skips when it is
unset.  Both binaries share one ``GRIM_HOME`` and one project, as on one machine.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from src.helpers import release_with_grim, write_config
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

NAME = "manifested"
ENTRY = f"{NAME}/.claude-plugin/plugin.json"
_SKILL_MD = f"---\nname: {NAME}\ndescription: a skill\n---\n# {NAME}\n"
WITH_ENTRY = {f"{NAME}/SKILL.md": _SKILL_MD, ENTRY: '{"name": "x"}\n'}
CLEAN = {f"{NAME}/SKILL.md": _SKILL_MD + "v2\n"}


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


@pytest.fixture()
def current(grim_at, project_dir: Path, registry: str) -> GrimRunner:
    return grim_at(project_dir)


@pytest.fixture()
def old(old_bin: Path, current: GrimRunner, project_dir: Path) -> GrimRunner:
    return GrimRunner(old_bin, current.grim_home, cwd=project_dir)


@pytest.fixture()
def installed(old: GrimRunner, project_dir: Path, unique_repo: str) -> str:
    """grim 0.15.1 releases `1.0.0` with the entry and installs it; returns the repo."""
    repo = f"{unique_repo}/{NAME}"
    release_with_grim(old, repo, "1.0.0", WITH_ENTRY)
    write_config(project_dir, skills={NAME: f"{REGISTRY_HOST}/{repo}:1"})
    old.run("lock")
    old.run("install")
    # Positive control: 0.15.1 really wrote the entry, so a stale binary cannot pass below.
    assert (
        project_dir / ".claude" / "skills" / NAME / ".claude-plugin" / "plugin.json"
    ).is_file()
    return repo


def _snapshot(root: Path) -> dict[str, bytes | None]:
    """Every path under `root`, files by content and directories as None."""
    return {
        str(p.relative_to(root)): p.read_bytes() if p.is_file() else None
        for p in sorted(root.rglob("*"))
    }


def _entries_named_manifest(root: Path) -> list[Path]:
    return [p for p in root.rglob(".claude-plugin*")]


def test_install_refuses_an_installed_skill_with_a_manifest_entry(
    installed, current: GrimRunner, project_dir: Path
) -> None:
    before = _snapshot(project_dir)
    assert ".grimoire/state.json" in before  # positive control: state was written

    result = current.run("install", check=False)

    assert result.returncode == 65, result.stderr
    reason = json.loads(current.run("install", format="json", check=False).stdout)
    assert reason["error"]["reason"] == "skill-plugin-manifest", reason
    assert _snapshot(project_dir) == before


@pytest.mark.parametrize("flags", [["--offline"], ["--force"]])
def test_install_flags_do_not_lift_the_refusal(
    installed, current: GrimRunner, project_dir: Path, flags: list[str]
) -> None:
    before = _snapshot(project_dir)
    assert ".grimoire/state.json" in before

    result = current.run("install", *flags, check=False)

    assert result.returncode == 65, result.stderr
    assert _snapshot(project_dir) == before


def test_status_still_reads_an_installed_skill_with_a_manifest_entry(
    installed, current: GrimRunner
) -> None:
    first = current.run("status", "--format", "json", check=False)
    second = current.run("status", "--format", "json", check=False)

    assert first.returncode == 0, first.stderr
    assert first.stdout == second.stdout
    rows = [row for row in _walk(json.loads(first.stdout)) if row.get("name") == NAME]
    assert rows, first.stdout  # positive control: the row is there
    assert all(row.get("state") == "installed" for row in rows), json.dumps(rows)
    assert len([line for line in first.stderr.splitlines() if NAME in line]) == 1, (
        first.stderr
    )


def _walk(node):
    if isinstance(node, dict):
        yield node
        for value in node.values():
            yield from _walk(value)
    elif isinstance(node, list):
        for value in node:
            yield from _walk(value)


def test_uninstall_removes_an_installed_skill_with_a_manifest_entry(
    installed, current: GrimRunner, project_dir: Path, tmp_path: Path
) -> None:
    copy = tmp_path / "project-copy"
    shutil.copytree(project_dir, copy)
    skill_dir = copy / ".claude" / "skills" / NAME
    assert skill_dir.is_dir()  # positive control

    result = GrimRunner(current.binary, current.grim_home, cwd=copy).run(
        "uninstall", "skill", NAME, check=False
    )

    assert result.returncode == 0, result.stderr
    assert not skill_dir.exists()
    assert _entries_named_manifest(copy / ".claude") == []


def test_update_to_a_version_without_the_entry_replaces_the_installed_skill(
    installed: str, old: GrimRunner, current: GrimRunner, project_dir: Path
) -> None:
    release_with_grim(old, installed, "1.1.0", CLEAN)

    result = current.run("update", check=False)

    assert result.returncode == 0, result.stderr
    skill_dir = project_dir / ".claude" / "skills" / NAME
    assert "v2" in (skill_dir / "SKILL.md").read_text(encoding="utf-8")
    assert _entries_named_manifest(project_dir) == []
    # grim 0.15.1 reads what the update left, as an older grim on the same machine would.
    assert old.run("install", check=False).returncode == 0
