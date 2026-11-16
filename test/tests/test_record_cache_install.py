# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim install`` over a record that lags the lock while the files match it.

A record is trusted while the files match it. When they do not and the lock
names another pin, the install gate compares the files with a render of the
lock's pin: a match re-pins the record and writes no client byte, outputs split
between the two pins install as a pin move, and anything else is the ``65``
``modified`` refusal it always was. Install halves of
``research_install_record_cache_test_matrix.md`` S01, S03, S07, S15, S16, S17,
S21, S22, S26, S27, S28, S33 to S36, S38, S40, S41 and S59, plus ADR
validation cases 2, 3, 6 and 22.

World: a skill ``moving`` and a rule ``style`` whose tags move from v1 to v2,
and a skill ``steady`` whose tag never moves. The artifact-shape cases build
their own worlds on the same team pull.
"""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, write_config
from src.registry import REGISTRY_HOST, retag, tag_digest
from src.runner import GrimRunner
from src.scenarios import (
    Team,
    assert_unchanged,
    fresh_runner,
    git,
    move_tag,
    state_snapshot,
)

EDIT = "# local edit\n"
NAMES = ("moving", "style", "steady")


def _moving(version: str) -> dict[str, str]:
    return {
        "moving/SKILL.md": f"# moving {version}\n",
        "moving/ref.md": f"ref {version}\n",
    }


def _style(version: str) -> dict[str, str]:
    return {"style.md": f"---\npaths: ['**/*.rs']\n---\n# style {version}\n"}


def _publish_v1(repo: str) -> None:
    make_artifact(f"{repo}/moving", "skill", _moving("v1"), tag="rolling")
    make_artifact(f"{repo}/style", "rule", _style("v1"), tag="rolling")
    make_artifact(
        f"{repo}/steady", "skill", {"steady/SKILL.md": "# steady\n"}, tag="stable"
    )


def _publish_v2(repo: str) -> None:
    move_tag(f"{repo}/moving", _moving("v2"), tag="rolling", kind="skill")
    move_tag(f"{repo}/style", _style("v2"), tag="rolling", kind="rule")


def _declare(root: Path, repo: str, clients: tuple[str, ...] = ()) -> None:
    options = {"clients": json.dumps(list(clients))} if clients else None
    write_config(
        root,
        skills={
            "moving": f"{REGISTRY_HOST}/{repo}/moving:rolling",
            "steady": f"{REGISTRY_HOST}/{repo}/steady:stable",
        },
        rules={"style": f"{REGISTRY_HOST}/{repo}/style:rolling"},
        options=options,
    )


# ---------------------------------------------------------------------------
# Where a scope keeps its lock, its state and its client outputs
# ---------------------------------------------------------------------------


@dataclass
class Place:
    runner: GrimRunner
    flags: tuple[str, ...]
    lock: Path
    state: Path
    root: Path

    def run(self, *args: str):
        """Run ``grim --format json`` and return ``(exit, parsed stdout)``."""
        result = self.runner.run(*self.flags, *args, format="json", check=False)
        try:
            doc = json.loads(result.stdout)
        except json.JSONDecodeError:
            doc = {}
        return result.returncode, doc

    def skill(self, name: str, client: str = "claude") -> Path:
        folder = {"claude": ".claude", "copilot": ".github"}[client]
        return self.root / folder / "skills" / name

    def rule(self, name: str) -> Path:
        return self.root / ".claude" / "rules" / f"{name}.md"

    def outputs(self) -> list[Path]:
        files = [
            p
            for name in ("moving", "steady")
            for p in sorted(self.skill(name).rglob("*"))
            if p.is_file()
        ]
        return [*files, self.rule("style")]


def _project(runner: GrimRunner) -> Place:
    work = runner.cwd
    assert work is not None
    return Place(
        runner, (), work / "grimoire.lock", work / ".grimoire" / "state.json", work
    )


def _global(runner: GrimRunner) -> Place:
    home = runner.grim_home
    return Place(
        runner,
        ("--global",),
        home / "grimoire.lock",
        home / "state" / "global.json",
        runner.home,
    )


def _digest(pinned: str) -> str:
    return pinned.rsplit("@", 1)[1]


def _record_pins(place: Place) -> dict[str, str]:
    doc = json.loads(place.state.read_text(encoding="utf-8"))
    return {r["name"]: _digest(r["pinned"]) for r in doc["records"]}


def _lock_pins(place: Place) -> dict[str, str]:
    doc = tomllib.loads(place.lock.read_text(encoding="utf-8"))
    rows = [*doc.get("skill", []), *doc.get("rule", []), *doc.get("mcp", [])]
    return {row["name"]: _digest(row["pinned"]) for row in rows}


def _rows(doc: dict) -> dict[str, str]:
    return {row["name"]: row["status"] for row in doc.get("items", [])}


def _frozen(place: Place):
    """mtime and inode of every client output: a rewrite moves at least one."""
    paths = place.outputs()
    return state_snapshot(paths[0], *paths[1:])


def _assert_repinned(place: Place, rc: int, doc: dict) -> None:
    assert rc == 0, doc
    assert _rows(doc) == {name: "unchanged" for name in NAMES}
    assert _record_pins(place) == _lock_pins(place)


def _records(place: Place) -> dict[str, dict]:
    return {
        r["name"]: r
        for r in json.loads(place.state.read_text(encoding="utf-8"))["records"]
    }


def _assert_refused(
    place: Place, rc: int, doc: dict, before: dict[str, dict], *names: str
) -> None:
    assert rc == 65, doc
    assert doc["error"]["reason"] == "modified"
    after = _records(place)
    assert {n: after[n] for n in names} == {n: before[n] for n in names}


# ---------------------------------------------------------------------------
# A team pull: Bob installed v1, Alice updated to v2 and committed the outputs
# ---------------------------------------------------------------------------


def _bob(team: Team) -> Place:
    return _project(team.bob.runner)


def _team_pull(
    team: Callable[[str], Team],
    declare: Callable[[Team], None],
    roll: Callable[[Team], None],
    *,
    ignore: str = "",
) -> Team:
    """Bob installed what ``declare`` names; his pull brings Alice's update after ``roll``.

    ``ignore`` lands in the first commit as a ``.gitignore``, so those outputs
    stay on each machine at the version it last wrote.
    """
    t = team("CO")
    if ignore:
        gitignore = t.alice.work / ".gitignore"
        gitignore.write_text(ignore, encoding="utf-8", newline="\n")
    declare(t)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    roll(t)
    t.alice.runner.run("update")
    t.alice.commit_all("chore: v2")
    t.alice.push()
    t.bob.pull()
    return t


def _pulled(
    team: Callable[[str], Team],
    repo: str,
    clients: tuple[str, ...] = (),
    *,
    ignore: str = "",
) -> Team:
    """Bob holds the record at v1; his pull brought Alice's v2 lock and outputs."""

    def declare(t: Team) -> None:
        _publish_v1(repo)
        _declare(t.alice.work, repo, clients)

    return _team_pull(team, declare, lambda _: _publish_v2(repo), ignore=ignore)


def test_install_after_a_pull_re_pins_and_writes_no_client_byte(team, unique_repo):
    # research_install_record_cache_test_matrix.md S03; ADR validation case 2
    bob = _bob(_pulled(team, unique_repo))
    assert _record_pins(bob) != _lock_pins(bob)
    outputs = _frozen(bob)
    rc, doc = bob.run("install")
    _assert_repinned(bob, rc, doc)
    assert "moving v2" in (bob.skill("moving") / "SKILL.md").read_text(encoding="utf-8")
    assert_unchanged(outputs)


def test_install_twice_after_a_pull_writes_nothing_the_second_time(team, unique_repo):
    # research_install_record_cache_test_matrix.md S59
    bob = _bob(_pulled(team, unique_repo))
    rc, doc = bob.run("install")
    _assert_repinned(bob, rc, doc)
    record = bob.state.read_bytes()
    outputs = _frozen(bob)
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert set(_rows(doc).values()) == {"unchanged"}
    # Bytes, not mtime: an unchanged install rewrites identical state today.
    assert bob.state.read_bytes() == record
    assert_unchanged(outputs)


def test_install_after_a_pull_with_the_pin_not_stored_offline_refuses(
    team, unique_repo
):
    # Proof only upgrades: content it cannot read leaves today's refusal.
    bob = _bob(_pulled(team, unique_repo))
    before = _records(bob)
    outputs = _frozen(bob)
    rc, doc = bob.run("--offline", "install")
    _assert_refused(bob, rc, doc, before, *NAMES)
    assert_unchanged(outputs)


@pytest.mark.parametrize(
    ("clients", "edited"),
    [(("claude",), "claude"), (("claude", "copilot"), "copilot")],
    ids=["one-client", "two-clients"],
)
def test_a_hand_edit_on_top_of_a_pull_refuses_and_keeps_the_record(
    team, unique_repo, clients, edited
):
    # research_install_record_cache_test_matrix.md S22; ADR validation case 6
    bob = _bob(_pulled(team, unique_repo, clients))
    edited_file = bob.skill("moving", edited) / "SKILL.md"
    edited_file.write_text(
        edited_file.read_text(encoding="utf-8") + EDIT, encoding="utf-8", newline="\n"
    )
    before = _records(bob)
    rc, doc = bob.run("install")
    # Only the edited node keeps its record; the pulled rule still re-pins.
    _assert_refused(bob, rc, doc, before, "moving")
    assert _record_pins(bob)["style"] == _lock_pins(bob)["style"]
    assert edited_file.read_text(encoding="utf-8").endswith(EDIT)


def test_an_absent_output_of_another_client_is_restored_after_the_re_pin(
    team, unique_repo
):
    bob = _bob(_pulled(team, unique_repo, ("claude", "copilot")))
    shutil.rmtree(bob.skill("moving", "copilot"))
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc)["moving"] == "updated"
    assert "moving v2" in (bob.skill("moving", "copilot") / "SKILL.md").read_text(
        encoding="utf-8"
    )
    assert _record_pins(bob) == _lock_pins(bob)


def test_install_after_each_checkout_of_a_committed_branch_re_pins(team, unique_repo):
    # research_install_record_cache_test_matrix.md S07; ADR validation case 3
    t = _pulled(team, unique_repo)
    bob = _bob(t)
    bob.run("install")
    older = git(t.alice.work, "rev-parse", "HEAD~1")
    for ref in (older, "main"):
        git(t.bob.work, "checkout", "-q", ref)
        assert _record_pins(bob) != _lock_pins(bob)
        outputs = _frozen(bob)
        rc, doc = bob.run("install")
        _assert_repinned(bob, rc, doc)
        rc, doc = bob.run("install")
        assert rc == 0, doc
        assert_unchanged(outputs)


def test_install_over_a_state_file_from_a_worktree_on_another_branch(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md S26
    t = _pulled(team, unique_repo)
    bob = _bob(t)
    old_tree = tmp_path / "bob-old"
    git(t.bob.work, "worktree", "add", "-q", "--detach", str(old_tree), "HEAD~1")
    old = _project(
        GrimRunner(t.bob.runner.binary, t.bob.runner.grim_home, cwd=old_tree)
    )
    assert old.run("install")[0] == 0
    bob.run("install")
    bob.state.write_bytes(old.state.read_bytes())
    assert _record_pins(bob)["moving"] != _lock_pins(bob)["moving"]
    assert _record_pins(bob)["steady"] == _lock_pins(bob)["steady"]
    outputs = _frozen(bob)
    rc, doc = bob.run("install")
    _assert_repinned(bob, rc, doc)
    assert_unchanged(outputs)


# ---------------------------------------------------------------------------
# One machine: a state file restored from before an update
# ---------------------------------------------------------------------------


def _machine(tmp_path: Path, repo: str, scope: str) -> Place:
    """Installed at v1, state saved, updated to v2; the state is not restored yet."""
    if scope == "project":
        work = tmp_path / "work"
        (work / ".claude").mkdir(parents=True)
        place = _project(fresh_runner(tmp_path, "machine", cwd=work))
        _declare(work, repo)
    else:
        place = _global(fresh_runner(tmp_path, "machine"))
        (place.runner.home / ".claude").mkdir(parents=True)
        _declare(place.runner.grim_home, repo)
    _publish_v1(repo)
    assert place.run("lock")[0] == 0
    assert place.run("install")[0] == 0
    return place


@pytest.mark.parametrize("scope", ["project", "global"])
def test_install_over_a_restored_older_state_file_re_pins(tmp_path, unique_repo, scope):
    # research_install_record_cache_test_matrix.md S27
    place = _machine(tmp_path, unique_repo, scope)
    saved = place.state.read_bytes()
    _publish_v2(unique_repo)
    assert place.run("update")[0] == 0
    place.state.write_bytes(saved)
    assert _record_pins(place) != _lock_pins(place)
    outputs = _frozen(place)
    rc, doc = place.run("install")
    _assert_repinned(place, rc, doc)
    assert_unchanged(outputs)


def test_install_after_an_interrupted_update_re_pins_and_re_renders(
    tmp_path, unique_repo
):
    # research_install_record_cache_test_matrix.md S28
    place = _machine(tmp_path, unique_repo, "project")
    saved = place.state.read_bytes()
    kept = tmp_path / "moving-v1"
    shutil.copytree(place.skill("moving"), kept)
    _publish_v2(unique_repo)
    assert place.run("update")[0] == 0
    place.state.write_bytes(saved)
    shutil.rmtree(place.skill("moving"))
    shutil.copytree(kept, place.skill("moving"))
    rc, doc = place.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {
        "moving": "updated",
        "style": "unchanged",
        "steady": "unchanged",
    }
    assert "moving v2" in (place.skill("moving") / "SKILL.md").read_text(
        encoding="utf-8"
    )
    assert _record_pins(place) == _lock_pins(place)


# ---------------------------------------------------------------------------
# Artifact shapes behind a team pull: an MCP entry, a support directory, a
# render that embeds the pin, a shared pool, a renamed member, a path source
# ---------------------------------------------------------------------------

MCP_DESCRIPTOR = """\
description = "Record-cache test server."

[server]
transport = "stdio"
command = "grim"
args = [{args}]
"""

MCP_FILES = {"claude": ".mcp.json", "codex": ".codex/config.toml"}


def _release_mcp(t: Team, repo: str, version: str, args: str) -> None:
    descriptor = t.alice.work.parent / "src" / "srv.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(
        MCP_DESCRIPTOR.format(args=args), encoding="utf-8", newline="\n"
    )
    ref = f"{REGISTRY_HOST}/{repo}/srv:{version}"
    t.alice.runner.run("release", str(descriptor), ref, "--kind", "mcp")


def _mcp_pulled(
    team: Callable[[str], Team], repo: str, *clients: str, ignore: str = ""
) -> Team:
    """Bob adopted Alice's MCP entry; her update changed its value."""

    def declare(t: Team) -> None:
        _release_mcp(t, repo, "1.0.0", '"mcp"')
        (t.alice.work / "grimoire.toml").write_text(
            f"[options]\nclients = {json.dumps(list(clients))}\n\n"
            f'[mcp]\nsrv = "{REGISTRY_HOST}/{repo}/srv:1"\n',
            encoding="utf-8",
            newline="\n",
        )

    def roll(t: Team) -> None:
        _release_mcp(t, repo, "1.1.0", '"mcp", "--verbose"')

    return _team_pull(team, declare, roll, ignore=ignore)


def _adopted(place: Place, name: str) -> list[bool]:
    return [o.get("adopted", False) for o in _records(place)[name]["outputs"]]


@pytest.mark.parametrize("client", ["claude", "codex"])
def test_install_after_a_pulled_mcp_value_change_re_pins_and_keeps_adopted(
    team, unique_repo, client
):
    # research_install_record_cache_test_matrix.md S36, S38
    bob = _bob(_mcp_pulled(team, unique_repo, client))
    config = bob.root / MCP_FILES[client]
    assert "--verbose" in config.read_text(encoding="utf-8")
    assert _adopted(bob, "srv") == [True]
    assert _record_pins(bob) != _lock_pins(bob)
    before = state_snapshot(config)
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"srv": "unchanged"}
    assert _record_pins(bob) == _lock_pins(bob)
    assert _adopted(bob, "srv") == [True]
    assert_unchanged(before)


def test_a_hand_edited_mcp_entry_after_a_pull_refuses_and_keeps_the_record(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S36
    bob = _bob(_mcp_pulled(team, unique_repo, "claude"))
    config = bob.root / ".mcp.json"
    doc = json.loads(config.read_text(encoding="utf-8"))
    doc["mcpServers"]["srv"]["args"].append("--edited")
    config.write_text(json.dumps(doc, indent=2), encoding="utf-8", newline="\n")
    before = _records(bob)
    rc, out = bob.run("install")
    _assert_refused(bob, rc, out, before, "srv")
    assert "--edited" in config.read_text(encoding="utf-8")


def _guide(version: str, support: bool) -> dict[str, str]:
    files = {"guide.md": f"---\npaths: ['**/*.md']\n---\n# guide {version}\n"}
    if support:
        files["guide/notes.md"] = f"notes {version}\n"
    return files


@pytest.mark.parametrize("gains", [True, False], ids=["gains", "loses"])
def test_install_after_a_pull_re_pins_a_rule_whose_support_directory_changed(
    team, unique_repo, gains
):
    # research_install_record_cache_test_matrix.md S33, S34
    repo = f"{unique_repo}/guide"

    def declare(t: Team) -> None:
        make_artifact(repo, "rule", _guide("v1", not gains), tag="rolling")
        write_config(t.alice.work, rules={"guide": f"{REGISTRY_HOST}/{repo}:rolling"})

    def roll(_: Team) -> None:
        move_tag(repo, _guide("v2", gains), tag="rolling", kind="rule")

    bob = _bob(_team_pull(team, declare, roll))
    support = bob.root / ".claude" / "rules" / "guide"
    assert support.is_dir() is gains
    files = [bob.rule("guide"), *sorted(p for p in support.rglob("*") if p.is_file())]
    before = state_snapshot(files[0], *files[1:])
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"guide": "unchanged"}
    assert _record_pins(bob) == _lock_pins(bob)
    [output] = _records(bob)["guide"]["outputs"]
    assert ("support_dir" in output) is gains, output
    assert_unchanged(before)


def _republish(repo: str, files: dict[str, str], kind: str, build: str) -> None:
    """Push the same files again under ``rolling``: a new digest, so a new pin."""
    old = tag_digest(repo, "rolling")
    retag(repo, f"keep-{old.removeprefix('sha256:')[:12]}", old)
    make_artifact(
        repo,
        kind,
        files,
        tag="rolling",
        annotations={"org.opencontainers.image.revision": build},
    )


def _opencode_style(t: Team, repo: str) -> None:
    make_artifact(repo, "rule", _style("v1"), tag="rolling")
    write_config(
        t.alice.work,
        rules={"style": f"{REGISTRY_HOST}/{repo}:rolling"},
        options={"clients": json.dumps(["opencode"])},
    )


def _opencode_rule(place: Place) -> Path:
    return place.root / ".opencode" / "rules" / "style.md"


def test_install_after_a_pull_proves_a_render_against_the_lock_pin(team, unique_repo):
    # research_install_record_cache_test_matrix.md S35
    repo = f"{unique_repo}/style"
    t = _team_pull(
        team,
        lambda t: _opencode_style(t, repo),
        lambda _: _republish(repo, _style("v1"), "rule", "b"),
    )
    bob = _bob(t)
    rule = _opencode_rule(bob)
    # The files did not change; only the pin the render embeds did.
    assert _lock_pins(bob)["style"] in rule.read_text(encoding="utf-8")
    assert _record_pins(bob) != _lock_pins(bob)
    before = state_snapshot(rule)
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"style": "unchanged"}
    assert _record_pins(bob) == _lock_pins(bob)
    assert_unchanged(before)


def _lock_only_bump(t: Team, roll: Callable[[], None]) -> None:
    roll()
    t.alice.runner.run("update")
    git(t.alice.work, "add", "grimoire.lock")
    git(t.alice.work, "commit", "-q", "-m", "chore: bump lock")
    t.alice.push()


def test_a_clone_holding_a_render_of_an_older_pin_refuses_it(team, unique_repo):
    # research_install_record_cache_test_matrix.md S35
    repo = f"{unique_repo}/style"
    t = team("CO")
    _opencode_style(t, repo)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    _lock_only_bump(t, lambda: _republish(repo, _style("v1"), "rule", "b"))
    t.bob.pull()
    bob = _bob(t)
    rule = _opencode_rule(bob)
    contents = rule.read_bytes()
    assert _lock_pins(bob)["style"] not in rule.read_text(encoding="utf-8")
    rc, doc = bob.run("install")
    assert rc == 65, doc
    assert doc["error"]["reason"] == "untracked-destination"
    assert rule.read_bytes() == contents


def test_install_after_a_pull_re_pins_a_skill_three_clients_share(team, unique_repo):
    # research_install_record_cache_test_matrix.md S40
    repo = f"{unique_repo}/moving"

    def declare(t: Team) -> None:
        make_artifact(repo, "skill", _moving("v1"), tag="rolling")
        write_config(
            t.alice.work,
            skills={"moving": f"{REGISTRY_HOST}/{repo}:rolling"},
            options={"clients": json.dumps(["codex", "zed", "amp"])},
        )

    def roll(_: Team) -> None:
        move_tag(repo, _moving("v2"), tag="rolling", kind="skill")

    bob = _bob(_team_pull(team, declare, roll))
    pool = bob.root / ".agents" / "skills" / "moving"
    files = sorted(p for p in pool.rglob("*") if p.is_file())
    assert "moving v2" in (pool / "SKILL.md").read_text(encoding="utf-8")
    before = state_snapshot(files[0], *files[1:])
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"moving": "unchanged"}
    outputs = _records(bob)["moving"]["outputs"]
    assert sorted(o["client"] for o in outputs) == ["amp", "codex", "zed"]
    assert len({json.dumps(o["target"], sort_keys=True) for o in outputs}) == 1
    assert len({o["content_hash"] for o in outputs}) == 1
    assert _record_pins(bob) == _lock_pins(bob)
    assert_unchanged(before)


def test_install_after_a_pull_adopts_a_bundle_member_under_its_new_name(
    team, unique_repo
):
    # S-024 of the plan: a bundle renames a member.
    member = make_artifact(
        f"{unique_repo}/helper", "skill", {"helper/SKILL.md": "# helper\n"}, tag="v1"
    )
    bundle_repo = f"{unique_repo}/pack"

    def declare(t: Team) -> None:
        make_bundle(bundle_repo, [("skill", "helper", member.fq)], tag="rolling")
        write_config(
            t.alice.work, bundles={"pack": f"{REGISTRY_HOST}/{bundle_repo}:rolling"}
        )

    def roll(_: Team) -> None:
        old = tag_digest(bundle_repo, "rolling")
        retag(bundle_repo, f"keep-{old.removeprefix('sha256:')[:12]}", old)
        make_bundle(bundle_repo, [("skill", "aide", member.fq)], tag="rolling")

    bob = _bob(_team_pull(team, declare, roll))
    aide = bob.skill("aide") / "SKILL.md"
    assert not bob.skill("helper").exists()
    contents = aide.read_bytes()
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc)["aide"] == "unchanged"
    assert _record_pins(bob)["aide"] == _lock_pins(bob)["aide"]
    assert aide.read_bytes() == contents


def _local_skill(root: Path, version: str) -> None:
    source = root / "skills" / "local" / "SKILL.md"
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text(
        f"---\nname: local\ndescription: Local.\n---\n# local {version}\n",
        encoding="utf-8",
        newline="\n",
    )


def test_install_through_config_after_a_pulled_path_source_edit_re_pins(team):
    # research_install_record_cache_test_matrix.md S41
    def declare(t: Team) -> None:
        _local_skill(t.alice.work, "v1")
        write_config(t.alice.work, skills={"local": "./skills/local"})

    def roll(t: Team) -> None:
        _local_skill(t.alice.work, "v2")
        t.alice.runner.run("lock")

    t = _team_pull(team, declare, roll)
    work = t.bob.work
    # Run from outside the workspace, so the path source resolves through --config.
    outside = GrimRunner(t.bob.runner.binary, t.bob.runner.grim_home, cwd=work.parent)
    bob = Place(
        outside,
        ("--config", f"{work.name}/grimoire.toml"),
        work / "grimoire.lock",
        work / ".grimoire" / "state.json",
        work,
    )
    skill = bob.skill("local") / "SKILL.md"
    assert "local v2" in skill.read_text(encoding="utf-8")
    lock = tomllib.loads(bob.lock.read_text(encoding="utf-8"))["skill"][0]
    assert _records(bob)["local"]["hash"] != lock["hash"]
    before = state_snapshot(skill)
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"local": "unchanged"}
    assert _records(bob)["local"]["hash"] == lock["hash"]
    assert_unchanged(before)


# ---------------------------------------------------------------------------
# No record yet, and one artifact at two versions
# ---------------------------------------------------------------------------


def _alice_v1(t: Team, repo: str) -> None:
    _publish_v1(repo)
    _declare(t.alice.work, repo)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()


def _contents(place: Place) -> dict[Path, bytes]:
    return {path: path.read_bytes() for path in place.outputs()}


def test_install_in_a_fresh_clone_adopts_every_output_and_writes_no_byte(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S01; ADR validation case 22
    t = team("CO")
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = _bob(t)
    contents = _contents(bob)
    assert not bob.state.exists()
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {name: "unchanged" for name in NAMES}
    assert bob.state.is_file()
    assert _record_pins(bob) == _lock_pins(bob)
    assert _contents(bob) == contents


def test_install_after_stashing_and_popping_an_update_never_refuses(team, unique_repo):
    # research_install_record_cache_test_matrix.md S15
    t = team("CO")
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = _bob(t)
    assert bob.run("install")[0] == 0
    _publish_v2(unique_repo)
    make_artifact(
        f"{unique_repo}/fresh", "skill", {"fresh/SKILL.md": "# fresh\n"}, tag="stable"
    )
    assert bob.run("update")[0] == 0
    assert bob.run("add", f"{REGISTRY_HOST}/{unique_repo}/fresh:stable")[0] == 0
    fresh = bob.skill("fresh") / "SKILL.md"
    for step in (("stash", "-q"), ("stash", "pop", "-q")):
        git(t.bob.work, *step)
        rc, doc = bob.run("install")
        assert rc == 0, (step, doc)
        assert set(_rows(doc).values()) == {"unchanged"}, (step, doc)
        assert _record_pins(bob)["moving"] == _lock_pins(bob)["moving"], step
        # The new artifact's files are untracked, so they stay with their record.
        assert fresh.is_file(), step
        assert "fresh" in _records(bob), step


@pytest.mark.parametrize("model", ["CO", "GO"])
def test_install_after_git_clean_adopts_committed_outputs_or_installs_afresh(
    team, unique_repo, model
):
    # research_install_record_cache_test_matrix.md S16
    t = team(model)
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = _bob(t)
    assert bob.run("install")[0] == 0
    git(t.bob.work, "clean", "-q", "-fdx")
    assert not bob.state.exists()
    rc, doc = bob.run("install")
    assert rc == 0, doc
    expected = "unchanged" if model == "CO" else "installed"
    assert _rows(doc) == {name: expected for name in NAMES}
    assert _record_pins(bob) == _lock_pins(bob)


def test_install_after_a_lock_only_bump_re_renders_over_the_record(team, unique_repo):
    # research_install_record_cache_test_matrix.md S17 (a)
    t = team("CO")
    _alice_v1(t, unique_repo)
    t.bob.pull()
    bob = _bob(t)
    assert bob.run("install")[0] == 0
    _lock_only_bump(t, lambda: _publish_v2(unique_repo))
    t.bob.pull()
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {
        "moving": "updated",
        "style": "updated",
        "steady": "unchanged",
    }
    assert "moving v2" in (bob.skill("moving") / "SKILL.md").read_text(encoding="utf-8")
    assert _record_pins(bob) == _lock_pins(bob)


def test_a_fresh_clone_after_a_lock_only_bump_refuses_the_older_files(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S17 (b)
    t = team("CO")
    _alice_v1(t, unique_repo)
    _lock_only_bump(t, lambda: _publish_v2(unique_repo))
    t.bob.pull()
    bob = _bob(t)
    contents = _contents(bob)
    rc, doc = bob.run("install")
    assert rc == 65, doc
    assert doc["error"]["reason"] == "untracked-destination"
    assert _contents(bob) == contents


def _two_versions(team: Callable[[str], Team], repo: str) -> Place:
    """``.claude/`` committed at v2; Bob's ignored ``.github/`` and his record at v1."""
    return _bob(_pulled(team, repo, ("claude", "copilot"), ignore=".github/\n"))


def test_install_over_one_artifact_at_two_versions_rolls_it_forward(team, unique_repo):
    # research_install_record_cache_test_matrix.md S21
    bob = _two_versions(team, unique_repo)
    claude = bob.skill("moving") / "SKILL.md"
    copilot = bob.skill("moving", "copilot") / "SKILL.md"
    assert "moving v2" in claude.read_text(encoding="utf-8")
    assert "moving v1" in copilot.read_text(encoding="utf-8")
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {
        "moving": "updated",
        "style": "updated",
        "steady": "unchanged",
    }
    assert "moving v2" in copilot.read_text(encoding="utf-8")
    assert _record_pins(bob) == _lock_pins(bob)


def test_one_artifact_at_two_versions_with_an_edit_refuses_and_keeps_the_record(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S21
    bob = _two_versions(team, unique_repo)
    copilot = bob.skill("moving", "copilot") / "SKILL.md"
    copilot.write_text(
        copilot.read_text(encoding="utf-8") + EDIT, encoding="utf-8", newline="\n"
    )
    before = _records(bob)
    rc, doc = bob.run("install")
    _assert_refused(bob, rc, doc, before, "moving")
    assert copilot.read_text(encoding="utf-8").endswith(EDIT)


def test_install_over_an_mcp_entry_at_two_versions_rolls_it_forward(team, unique_repo):
    # research_install_record_cache_test_matrix.md S21, through the MCP gate
    t = _mcp_pulled(team, unique_repo, "claude", "codex", ignore=".codex/\n")
    bob = _bob(t)
    codex = bob.root / MCP_FILES["codex"]
    assert "--verbose" in (bob.root / MCP_FILES["claude"]).read_text(encoding="utf-8")
    assert "--verbose" not in codex.read_text(encoding="utf-8")
    rc, doc = bob.run("install")
    assert rc == 0, doc
    assert _rows(doc) == {"srv": "updated"}
    assert "--verbose" in codex.read_text(encoding="utf-8")
    assert _record_pins(bob) == _lock_pins(bob)
