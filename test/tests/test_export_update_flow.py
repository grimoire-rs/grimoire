# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Export → update → re-export flow (design record S-019, C-023, C-011–C-013).

A declared plugin exported under `--version v1.2.0` carries `1.2.0+<h>`,
where `h` hashes the rendered tree. A patch published under the same
floating tag reaches the plugin only through `grim update --marketplace`;
the re-export then carries a new suffix and differs only in the patched
member's files and the manifest.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import tomllib

from src.helpers import make_artifact
from src.registry import push_artifact
from src.runner import GrimRunner

_KINDS = ("skill", "rule", "agent", "mcp")
_CLAUDE_MANIFEST = ".claude-plugin/plugin.json"


def _skill_md(name: str, heading: str) -> str:
    return f"---\nname: {name}\ndescription: Demo skill.\n---\n# {heading}\n\nBody.\n"


def _plan_files(notes: str) -> dict[str, str]:
    return {
        "team-plan/SKILL.md": _skill_md("team-plan", "Plan"),
        "team-plan/notes.md": notes,
    }


def _suffix(root: Path, base: str) -> str:
    """C-002/C-003: 12 hex of SHA-256 over the compact JSON `[name, exec, sha256]` triples
    of the produced tree, its manifest's version put back to `base`."""
    inventory = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        data = path.read_bytes()
        if rel == _CLAUDE_MANIFEST:
            version = json.loads(data)["version"]
            data = data.replace(f'"version": "{version}"'.encode(), f'"version": "{base}"'.encode())
        exec_bit = sys.platform != "win32" and bool(path.stat().st_mode & 0o111)
        inventory.append((rel, exec_bit, hashlib.sha256(data).hexdigest()))
    inventory.sort(key=lambda entry: entry[0].encode())
    blob = json.dumps([list(t) for t in inventory], separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(blob).hexdigest()[:12]


def _lock_digests(lock_path: Path, plugin: str) -> dict[tuple[str, str], str]:
    """`(kind, name) -> digest` of one plugin's pins in a marketplace lock."""
    lock = tomllib.loads(lock_path.read_text())
    return {
        (kind, e["name"]): e["pinned"].rsplit("@", 1)[1]
        for kind in _KINDS
        for e in lock.get(kind, [])
        if e.get("plugin") == plugin
    }


def _tree(root: Path) -> dict[str, bytes]:
    """Relative POSIX path → bytes of every regular file under ``root``."""
    out = {}
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            p = Path(dirpath) / f
            out[p.relative_to(root).as_posix()] = p.read_bytes()
    return out


def _export(runner: GrimRunner, *args: str) -> subprocess.CompletedProcess[str]:
    return runner.run(
        "export",
        "plugin",
        "--plugin",
        "team",
        "--client",
        "claude",
        "-o",
        "dist",
        *args,
        format="json",
        check=False,
    )


def _version(result: subprocess.CompletedProcess[str], root: Path) -> str:
    assert result.returncode == 0, f"rc={result.returncode}\nstderr: {result.stderr}"
    (item,) = json.loads(result.stdout)["items"]
    doc = json.loads((root / _CLAUDE_MANIFEST).read_text())
    assert item["version"] == doc["version"], (item, doc)
    return doc["version"]


# S-019 / C-023 / C-011 / C-013: version override survives an update; only the
# suffix moves, and only the patched member's files and the manifest change.
def test_s019_version_override_bumps_suffix_after_update(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    work = tmp_path / "work"
    work.mkdir()
    runner = grim_at(work)
    reg = f"{registry}/{unique_repo}"

    plan_v1 = make_artifact(
        f"{unique_repo}/team-plan", "skill", _plan_files("Planning notes.\n"), tag="1"
    ).digest
    make_artifact(
        f"{unique_repo}/team-review",
        "skill",
        {"team-review/SKILL.md": _skill_md("team-review", "Review")},
        tag="1",
    )
    make_artifact(
        f"{unique_repo}/team-reviewer",
        "agent",
        {
            "team-reviewer.md": "---\nname: team-reviewer\ndescription: Reviews code.\n---\n# Reviewer\nAgent body.\n"
        },
        tag="1",
    )
    members = [
        {"kind": "skill", "name": "team-plan", "id": f"{reg}/team-plan:1"},
        {"kind": "skill", "name": "team-review", "id": f"{reg}/team-review:1"},
        {"kind": "agent", "name": "team-reviewer", "id": f"{reg}/team-reviewer:1"},
    ]
    push_artifact(
        f"{unique_repo}/team-stack",
        "1",
        json.dumps({"members": members}).encode(),
        "bundle",
    )
    (work / "marketplace.toml").write_text(
        f'[plugins.team]\ninclude = ["{reg}/team-stack:1"]\n'
    )
    lock_path = work / "marketplace.lock"
    root = work / "dist" / "team.claude"

    v1 = _version(_export(runner, "--version", "v1.2.0"), root)
    pins_v1 = _lock_digests(lock_path, "team")
    assert set(pins_v1) == {
        ("skill", "team-plan"),
        ("skill", "team-review"),
        ("agent", "team-reviewer"),
    }
    assert pins_v1[("skill", "team-plan")] == plan_v1
    h = _suffix(root, "1.2.0")
    assert v1 == f"1.2.0+{h}"
    tree_v1 = _tree(root)

    plan_v2 = make_artifact(
        f"{unique_repo}/team-plan", "skill", _plan_files("Patched notes.\n"), tag="1"
    ).digest
    assert plan_v2 != plan_v1

    upd = runner.run(
        "update",
        "--marketplace",
        "marketplace.toml",
        "team",
        format="json",
        check=False,
    )
    assert upd.returncode == 0, f"rc={upd.returncode}\nstderr: {upd.stderr}"
    rows = {(r["kind"], r["name"]): r for r in json.loads(upd.stdout)["items"]}
    plan_row = rows[("skill", "team-plan")]
    assert (
        plan_row["plugin"],
        plan_row["action"],
        plan_row["old"],
        plan_row["new"],
    ) == ("team", "updated", plan_v1, plan_v2), plan_row
    assert all(
        r["action"] == "unchanged"
        for key, r in rows.items()
        if key != ("skill", "team-plan")
    ), rows

    pins_v2 = _lock_digests(lock_path, "team")
    assert pins_v2 == {**pins_v1, ("skill", "team-plan"): plan_v2}

    v2 = _version(_export(runner, "--version", "v1.2.0", "--force"), root)
    h2 = _suffix(root, "1.2.0")
    assert h2 != h
    assert v2 == f"1.2.0+{h2}"

    tree_v2 = _tree(root)
    assert set(tree_v2) == set(tree_v1)
    changed = {p for p in tree_v1 if tree_v1[p] != tree_v2[p]}
    assert changed == {_CLAUDE_MANIFEST, "skills/team-plan/notes.md"}, changed

    # C-023: build metadata on --version is refused, and nothing is replaced.
    bad = _export(runner, "--version", "1.2.0+ci", "--force")
    assert bad.returncode == 65, f"rc={bad.returncode}\nstderr: {bad.stderr}"
    assert json.loads(bad.stdout)["error"]["exit"] == 65
    assert _tree(root) == tree_v2
