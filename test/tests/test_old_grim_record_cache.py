# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""grim 0.15.1 on one side of a team whose records lag the lock.

A teammate who renders with 0.15.1 commits the same bytes this grim renders, so
this grim's install proves them against the lock's pin and re-pins. 0.15.1 has
no such proof: in the same state it refuses with ``65`` ``modified``, and this
grim installs the very same workspace. Install halves of
``research_install_record_cache_test_matrix.md`` S18 and S65. The old binary
comes from ``GRIM_OLD_BIN`` (checked by ``src.old_grim``); the module skips
when it is unset.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, write_config
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.scenarios import Member, Team, assert_unchanged, move_tag, state_snapshot

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

NAMES = ("moving", "style")
# The OpenCode rule is generated: its render embeds the pin it came from.
CLIENTS = ("claude", "opencode")


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


def _old(member: Member, old_bin: Path) -> GrimRunner:
    """0.15.1 on ``member``'s machine: same home, same workspace."""
    return GrimRunner(old_bin, member.runner.grim_home, cwd=member.work)


def _publish(repo: str, version: str) -> None:
    moving = {"moving/SKILL.md": f"# moving {version}\n"}
    style = {"style.md": f"---\npaths: ['**/*.rs']\n---\n# style {version}\n"}
    if version == "v1":
        make_artifact(f"{repo}/moving", "skill", moving, tag="rolling")
        make_artifact(f"{repo}/style", "rule", style, tag="rolling")
    else:
        move_tag(f"{repo}/moving", moving, tag="rolling", kind="skill")
        move_tag(f"{repo}/style", style, tag="rolling", kind="rule")


def _declare(root: Path, repo: str) -> None:
    write_config(
        root,
        skills={"moving": f"{REGISTRY_HOST}/{repo}/moving:rolling"},
        rules={"style": f"{REGISTRY_HOST}/{repo}/style:rolling"},
        options={"clients": json.dumps(list(CLIENTS))},
    )


def _digest(pinned: str) -> str:
    return pinned.rsplit("@", 1)[1]


def _records(work: Path) -> dict[str, dict]:
    doc = json.loads((work / ".grimoire" / "state.json").read_text(encoding="utf-8"))
    return {r["name"]: r for r in doc["records"]}


def _record_pins(work: Path) -> dict[str, str]:
    return {name: _digest(r["pinned"]) for name, r in _records(work).items()}


def _lock_pins(work: Path) -> dict[str, str]:
    doc = tomllib.loads((work / "grimoire.lock").read_text(encoding="utf-8"))
    return {
        row["name"]: _digest(row["pinned"]) for row in [*doc["skill"], *doc["rule"]]
    }


def _outputs(work: Path) -> list[Path]:
    return [
        work / ".claude" / "skills" / "moving" / "SKILL.md",
        work / ".claude" / "rules" / "style.md",
        work / ".opencode" / "skills" / "moving" / "SKILL.md",
        work / ".opencode" / "rules" / "style.md",
    ]


def _install(runner: GrimRunner) -> tuple[int, dict]:
    result = runner.run("install", format="json", check=False)
    try:
        return result.returncode, json.loads(result.stdout)
    except json.JSONDecodeError:
        return result.returncode, {"stderr": result.stderr}


def _rows(doc: dict) -> dict[str, str]:
    return {row["name"]: row["status"] for row in doc.get("items", [])}


def _v1_shared(t: Team, repo: str, bob: GrimRunner) -> None:
    """Alice locks and commits v1 with this grim; ``bob`` installs it after his pull."""
    _publish(repo, "v1")
    _declare(t.alice.work, repo)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    assert _install(bob)[0] == 0


def test_install_over_a_pull_a_teammate_rendered_with_old_grim_re_pins(
    team, unique_repo, old_bin
):
    # research_install_record_cache_test_matrix.md S18
    t = team("CO")
    _v1_shared(t, unique_repo, t.bob.runner)
    _publish(unique_repo, "v2")
    _old(t.alice, old_bin).run("update")
    t.alice.commit_all("chore: v2 from grim 0.15.1")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    rule = work / ".opencode" / "rules" / "style.md"
    assert _lock_pins(work)["style"] in rule.read_text(encoding="utf-8")
    assert _record_pins(work) != _lock_pins(work)
    paths = _outputs(work)
    outputs = state_snapshot(paths[0], *paths[1:])
    rc, doc = _install(t.bob.runner)
    assert rc == 0, doc
    assert _rows(doc) == {name: "unchanged" for name in NAMES}
    assert _record_pins(work) == _lock_pins(work)
    assert_unchanged(outputs)


def test_old_grim_refuses_a_pulled_update_this_grim_re_pins(team, unique_repo, old_bin):
    # research_install_record_cache_test_matrix.md S65: 0.15.1 cannot prove a pull.
    t = team("CO")
    bob_old = _old(t.bob, old_bin)
    _v1_shared(t, unique_repo, bob_old)
    _publish(unique_repo, "v2")
    t.alice.runner.run("update")
    t.alice.commit_all("chore: v2")
    t.alice.push()
    t.bob.pull()
    work = t.bob.work
    before = _records(work)
    contents = {path: path.read_bytes() for path in _outputs(work)}
    rc, doc = _install(bob_old)
    assert rc == 65, doc
    assert doc["error"]["reason"] == "modified"
    assert _records(work) == before
    assert {path: path.read_bytes() for path in _outputs(work)} == contents
    # The same workspace, installed with this grim.
    rc, doc = _install(t.bob.runner)
    assert rc == 0, doc
    assert _rows(doc) == {name: "unchanged" for name in NAMES}
    assert _record_pins(work) == _lock_pins(work)
    assert {path: path.read_bytes() for path in _outputs(work)} == contents
