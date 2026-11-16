# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim status`` over a record that lags the lock while the files match it.

A record is trusted while the files match it. When they do not and the lock
names another pin, ``status`` compares the files with a render of the lock's
pin read from the local store alone. A match reads ``installed`` and is
recorded, so a second run writes nothing; outputs split between the record's pin and the lock's
read ``outdated``; anything else reads ``modified``, and content missing from
the store adds one warning naming ``grim install``. ``stale`` and ``missing``
come first. Status halves of ``research_install_record_cache_test_matrix.md``
S04, S05, S11, S12, S13, S15, S21, S26, S27, S33, S34, S49 and E20, plus ADR
validation case 4.

World: a skill ``moving`` and a rule ``style`` whose tags move from v1 to v2,
and a skill ``steady`` whose tag never moves.
"""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, write_config
from src.oci_proxy import OciProxy
from src.registry import REGISTRY_HOST
from src.runner import GrimRunner
from src.scenarios import (
    Snapshot,
    Team,
    allow_insecure,
    assert_unchanged,
    cold_runner,
    fresh_runner,
    git,
    move_tag,
    state_snapshot,
)

NAMES = ("moving", "style", "steady")
INSTALL = "grim install"


def _moving(version: str) -> dict[str, str]:
    return {
        "moving/SKILL.md": f"# moving {version}\n",
        "moving/ref.md": f"ref {version}\n",
    }


def _style(version: str, support: bool) -> dict[str, str]:
    files = {"style.md": f"---\npaths: ['**/*.rs']\n---\n# style {version}\n"}
    if support:
        files["style/ref.md"] = f"style ref {version}\n"
    return files


def _publish_v1(repo: str, support: bool = False) -> None:
    make_artifact(f"{repo}/moving", "skill", _moving("v1"), tag="rolling")
    make_artifact(f"{repo}/style", "rule", _style("v1", support), tag="rolling")
    make_artifact(
        f"{repo}/steady", "skill", {"steady/SKILL.md": "# steady\n"}, tag="stable"
    )


def _publish_v2(repo: str, support: bool = False) -> None:
    move_tag(f"{repo}/moving", _moving("v2"), tag="rolling", kind="skill")
    move_tag(f"{repo}/style", _style("v2", support), tag="rolling", kind="rule")


def _declare(
    root: Path, repo: str, clients: tuple[str, ...] = (), host: str = REGISTRY_HOST
) -> None:
    options = {"clients": json.dumps(list(clients))} if clients else None
    write_config(
        root,
        skills={
            "moving": f"{host}/{repo}/moving:rolling",
            "steady": f"{host}/{repo}/steady:stable",
        },
        rules={"style": f"{host}/{repo}/style:rolling"},
        options=options,
    )


# ---------------------------------------------------------------------------
# Where a scope keeps its state and its client outputs
# ---------------------------------------------------------------------------


@dataclass
class Place:
    runner: GrimRunner
    flags: tuple[str, ...]
    state: Path
    root: Path

    def status(
        self, *args: str, runner: GrimRunner | None = None, pre: tuple[str, ...] = ()
    ):
        """Run ``grim --format json status``; return ``(exit, parsed stdout, stderr)``."""
        result = (runner or self.runner).run(
            *pre, *self.flags, "status", *args, format="json", check=False
        )
        try:
            doc = json.loads(result.stdout)
        except json.JSONDecodeError:
            doc = {}
        return result.returncode, doc, result.stderr

    def skill(self, name: str, client: str = "claude") -> Path:
        folder = {"claude": ".claude", "copilot": ".github"}[client]
        return self.root / folder / "skills" / name

    def rule(self, name: str, client: str = "claude") -> Path:
        if client == "copilot":
            return self.root / ".github" / "instructions" / f"{name}.instructions.md"
        return self.root / ".claude" / "rules" / f"{name}.md"

    def snapshot(self):
        """``state`` bytes, mtime and inode, plus mtime and inode of every client file."""
        files = [
            p
            for folder in (".claude", ".github")
            for p in sorted((self.root / folder).rglob("*"))
            if p.is_file()
        ]
        return state_snapshot(self.state, *files)


def _project(runner: GrimRunner) -> Place:
    work = runner.cwd
    assert work is not None
    return Place(runner, (), work / ".grimoire" / "state.json", work)


def _global(runner: GrimRunner) -> Place:
    return Place(
        runner, ("--global",), runner.grim_home / "state" / "global.json", runner.home
    )


def _states(doc: dict) -> dict[str, str]:
    return {row["name"]: row["state"] for row in doc.get("items", [])}


def _install_lines(stderr: str) -> list[str]:
    return [line for line in stderr.splitlines() if INSTALL in line]


def _keys(doc: dict) -> tuple[frozenset[str], set[frozenset[str]]]:
    """The top-level key set and every item's key set of a JSON report."""
    return frozenset(doc), {frozenset(row) for row in doc["items"]}


def _assert_installed(rc: int, doc: dict, stderr: str) -> None:
    assert rc == 0, doc
    assert _states(doc) == {name: "installed" for name in NAMES}
    assert _install_lines(stderr) == []


def _assert_recorded(snap: Snapshot) -> None:
    """The run wrote the state file and left every client file as it was."""
    state, *outputs = snap.files
    assert state.path.read_bytes() != state.content
    assert_unchanged(Snapshot(tuple(outputs)))


def _assert_silent(place: Place, runner: GrimRunner | None = None) -> None:
    """A second run finds nothing to prove and writes nothing."""
    snap = place.snapshot()
    _assert_installed(*place.status(runner=runner))
    assert_unchanged(snap)


# ---------------------------------------------------------------------------
# A team pull: Bob installed v1, Alice updated to v2 and committed the outputs
# ---------------------------------------------------------------------------


def _warm(team: Team) -> GrimRunner:
    """Bob's workspace on Alice's machine, whose store holds every pin she rendered."""
    a = team.alice.runner
    runner = GrimRunner(a.binary, a.grim_home, cwd=team.bob.work)
    runner.env = dict(a.env)
    return runner


def _bob(team: Team) -> Place:
    return _project(team.bob.runner)


def _v1(
    team: Callable[[str], Team],
    repo: str,
    clients: tuple[str, ...] = (),
    *,
    support: bool = False,
    proxy: OciProxy | None = None,
) -> Team:
    """Alice installed v1 and pushed; Bob pulled and installed it."""
    t = team("CO")
    host = REGISTRY_HOST
    if proxy is not None:
        host = proxy.host
        allow_insecure(t.alice.runner, host)
        allow_insecure(t.bob.runner, host)
    _publish_v1(repo, support)
    _declare(t.alice.work, repo, clients, host)
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    return t


def _alice_to_v2(t: Team, repo: str, *, support: bool = False) -> None:
    _publish_v2(repo, support)
    t.alice.runner.run("update")
    t.alice.commit_all("chore: v2")
    t.alice.push()


def _pulled(
    team: Callable[[str], Team],
    repo: str,
    clients: tuple[str, ...] = (),
    *,
    proxy: OciProxy | None = None,
) -> Team:
    """Bob holds the record at v1; his pull brought Alice's v2 lock and outputs."""
    t = _v1(team, repo, clients, proxy=proxy)
    _alice_to_v2(t, repo)
    t.bob.pull()
    return t


def test_status_after_a_pull_with_the_pin_stored_reads_installed_and_records_the_pin(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S05; ADR validation case 4
    t = _v1(team, unique_repo)
    bob = _bob(t)
    _, before, _ = bob.status()
    _alice_to_v2(t, unique_repo)
    t.bob.pull()
    snap = bob.snapshot()
    rc, doc, stderr = bob.status(runner=_warm(t))
    _assert_installed(rc, doc, stderr)
    outputs = [
        (out["client"], Path(out["path"]).resolve())
        for out in {row["name"]: row for row in doc["items"]}["moving"]["outputs"]
    ]
    assert outputs == [("claude", bob.skill("moving").resolve())]
    assert _keys(doc) == _keys(before)
    # Part B: status records the proven pin, and a second run writes nothing.
    _assert_recorded(snap)
    _assert_silent(bob, _warm(t))


def test_status_after_a_pull_without_the_pin_stored_names_grim_install_once(
    team, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md S04; ADR validation case 4
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy=proxy)
    bob = _bob(t)
    _, before, _ = bob.status()
    _alice_to_v2(t, unique_repo)
    t.bob.pull()
    snap = bob.snapshot()
    requests = sum(proxy.counts.values())
    # Offline: online, status now fetches the missing pin and adopts it.
    rc, doc, stderr = bob.status(pre=("--offline",))
    assert rc == 0, doc
    assert _states(doc) == {
        "moving": "modified",
        "style": "modified",
        "steady": "installed",
    }
    (line,) = _install_lines(stderr)
    assert "WARN" in line
    for token in ("skill", "'moving'", "rule", "'style'"):
        assert token in line
    assert "steady" not in line
    assert _keys(doc) == _keys(before)
    assert sum(proxy.counts.values()) == requests
    assert_unchanged(snap)


def test_status_check_proves_from_the_store_alone(team, unique_repo):
    # research_install_record_cache_test_matrix.md S49
    bob = _bob(_pulled(team, unique_repo))
    snap = bob.snapshot()
    # `--registry` keeps the catalog load of `--check` on the test registry.
    rc, doc, stderr = bob.status(
        "--check", pre=("--registry", f"{REGISTRY_HOST}/{unique_repo}")
    )
    assert rc == 0, doc
    rows = {row["name"]: row for row in doc["items"]}
    assert doc["checked"] is True
    assert rows["moving"]["state"] == "modified"
    assert rows["moving"]["update_available"] is False
    assert rows["steady"]["update_available"] is False
    assert len(_install_lines(stderr)) == 1
    assert_unchanged(snap)


# ---------------------------------------------------------------------------
# Rows 1 to 3 read no content: a merge that takes one side, a stale lock
# ---------------------------------------------------------------------------


def _grim_home_tree(runner: GrimRunner) -> list[str]:
    return sorted(
        p.relative_to(runner.grim_home).as_posix() for p in runner.grim_home.rglob("*")
    )


def test_rows_one_to_three_create_nothing_under_grim_home_and_send_no_request(
    team, unique_repo, oci_proxy, tmp_path
):
    proxy = oci_proxy("count")
    t = _v1(team, unique_repo, proxy=proxy)
    bob = _project(cold_runner(t.bob.runner, tmp_path, "empty"))
    _alice_to_v2(t, unique_repo)
    git(t.bob.work, "fetch", "-q", "origin")
    requests = sum(proxy.counts.values())
    # Row 1 everywhere, then row 2 (their lock, our outputs) and row 3 (an edit).
    rc, doc, _ = bob.status()
    assert rc == 0, doc
    assert set(_states(doc).values()) == {"installed"}
    git(t.bob.work, "checkout", "origin/main", "--", "grimoire.lock")
    steady = bob.skill("steady") / "SKILL.md"
    steady.write_text("# steady\n# local edit\n", encoding="utf-8", newline="\n")
    rc, doc, stderr = bob.status()
    assert rc == 0, doc
    assert _states(doc) == {
        "moving": "outdated",
        "style": "outdated",
        "steady": "modified",
    }
    assert _install_lines(stderr) == []
    assert _grim_home_tree(bob.runner) == []
    assert sum(proxy.counts.values()) == requests


def test_a_merge_taking_their_lock_and_our_outputs_reads_outdated(team, unique_repo):
    # research_install_record_cache_test_matrix.md S11
    t = _v1(team, unique_repo)
    bob = _bob(t)
    _alice_to_v2(t, unique_repo)
    git(t.bob.work, "fetch", "-q", "origin")
    git(t.bob.work, "checkout", "origin/main", "--", "grimoire.lock")
    snap = bob.snapshot()
    rc, doc, stderr = bob.status(runner=_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {
        "moving": "outdated",
        "style": "outdated",
        "steady": "installed",
    }
    assert _install_lines(stderr) == []
    assert_unchanged(snap)


def test_a_merge_taking_our_lock_and_their_outputs_reads_modified(team, unique_repo):
    # research_install_record_cache_test_matrix.md S12
    t = _v1(team, unique_repo)
    bob = _bob(t)
    _alice_to_v2(t, unique_repo)
    git(t.bob.work, "fetch", "-q", "origin")
    git(t.bob.work, "checkout", "origin/main", "--", ".claude")
    snap = bob.snapshot()
    for runner in (t.bob.runner, _warm(t)):
        rc, doc, stderr = bob.status(runner=runner)
        assert rc == 0, doc
        assert _states(doc) == {
            "moving": "modified",
            "style": "modified",
            "steady": "installed",
        }
        assert _install_lines(stderr) == []
    assert_unchanged(snap)


def test_a_merge_that_leaves_the_lock_stale_reads_stale_and_proves_nothing(
    team, unique_repo, oci_proxy
):
    # research_install_record_cache_test_matrix.md S13
    proxy = oci_proxy("count")
    t = _pulled(team, unique_repo, proxy=proxy)
    bob = _bob(t)
    make_artifact(
        f"{unique_repo}/extra", "skill", {"extra/SKILL.md": "# extra\n"}, tag="v1"
    )
    config = t.bob.work / "grimoire.toml"
    text = config.read_text(encoding="utf-8").replace(
        "[skills]\n", f'[skills]\nextra = "{proxy.host}/{unique_repo}/extra:v1"\n', 1
    )
    config.write_text(text, encoding="utf-8", newline="\n")
    snap = bob.snapshot()
    requests = sum(proxy.counts.values())
    rc, doc, stderr = bob.status()
    assert rc == 0, doc
    assert _states(doc) == {name: "stale" for name in ("extra", *NAMES)}
    assert _install_lines(stderr) == []
    assert sum(proxy.counts.values()) == requests
    assert_unchanged(snap)
    # After `grim lock` the lock is fresh again and the pulled render proves.
    assert t.bob.runner.run("lock", check=False).returncode == 0
    rc, doc, stderr = bob.status(runner=_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {"extra": "missing", **{name: "installed" for name in NAMES}}


def test_an_absent_output_reads_missing_before_any_proof(team, unique_repo):
    # research_install_record_cache_test_matrix.md E20
    bob = _bob(_pulled(team, unique_repo, ("claude", "copilot")))
    shutil.rmtree(bob.skill("moving", "copilot"))
    bob.rule("style", "copilot").unlink()
    snap = bob.snapshot()
    rc, doc, stderr = bob.status()
    assert rc == 0, doc
    assert _states(doc) == {
        "moving": "missing",
        "style": "missing",
        "steady": "installed",
    }
    assert _install_lines(stderr) == []
    assert_unchanged(snap)


# ---------------------------------------------------------------------------
# Where a lagging record comes from: a stash, two versions, another tree's state
# ---------------------------------------------------------------------------


def test_status_reads_installed_across_a_stashed_update(team, unique_repo):
    # research_install_record_cache_test_matrix.md S15
    t = _v1(team, unique_repo)
    bob = _bob(t)
    _publish_v2(unique_repo)
    assert t.bob.runner.run("update", check=False).returncode == 0
    for step in ((), ("stash", "-q"), ("stash", "pop", "-q")):
        if step:
            git(t.bob.work, *step)
        snap = bob.snapshot()
        _assert_installed(*bob.status())
        if step:
            # Part B: the stashed or popped pin proves and is recorded.
            _assert_recorded(snap)
            snap = bob.snapshot()
            _assert_installed(*bob.status())
        assert_unchanged(snap)


def test_one_artifact_at_two_versions_reads_outdated(team, unique_repo):
    # research_install_record_cache_test_matrix.md S21 (open question 2, recommended answer)
    t = _pulled(team, unique_repo, ("claude", "copilot"))
    bob = _bob(t)
    git(t.bob.work, "checkout", "HEAD~1", "--", ".github")
    snap = bob.snapshot()
    rc, doc, stderr = bob.status(runner=_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {
        "moving": "outdated",
        "style": "outdated",
        "steady": "installed",
    }
    assert _install_lines(stderr) == []
    assert_unchanged(snap)


def test_status_over_a_state_file_from_a_worktree_on_another_branch(
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
    assert old.runner.run("install", check=False).returncode == 0
    assert t.bob.runner.run("install", check=False).returncode == 0
    bob.state.write_bytes(old.state.read_bytes())
    snap = bob.snapshot()
    _assert_installed(*bob.status())
    # Part B: status records the proven pin, and a second run writes nothing.
    _assert_recorded(snap)
    _assert_silent(bob)


@pytest.mark.parametrize("scope", ["project", "global"])
def test_status_over_a_restored_older_state_file_reads_installed(
    tmp_path, unique_repo, scope
):
    # research_install_record_cache_test_matrix.md S27
    if scope == "project":
        work = tmp_path / "work"
        (work / ".claude").mkdir(parents=True)
        place = _project(fresh_runner(tmp_path, "machine", cwd=work))
        _declare(work, unique_repo)
    else:
        place = _global(fresh_runner(tmp_path, "machine"))
        (place.runner.home / ".claude").mkdir(parents=True)
        _declare(place.runner.grim_home, unique_repo)
    _publish_v1(unique_repo)
    for step in ("lock", "install"):
        assert place.runner.run(*place.flags, step, check=False).returncode == 0
    saved = place.state.read_bytes()
    _publish_v2(unique_repo)
    assert place.runner.run(*place.flags, "update", check=False).returncode == 0
    place.state.write_bytes(saved)
    snap = place.snapshot()
    _assert_installed(*place.status())
    # Part B: status records the proven pin, and a second run writes nothing.
    _assert_recorded(snap)
    _assert_silent(place)


# ---------------------------------------------------------------------------
# A rule whose support directory comes or goes with the version
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("before", "after"),
    [(False, True), (True, False)],
    ids=["gains-support", "loses-support"],
)
def test_a_rule_whose_support_directory_changes_reads_installed_twice(
    team, unique_repo, before, after
):
    # research_install_record_cache_test_matrix.md S33 (gains), S34 (loses)
    t = _v1(team, unique_repo, support=before)
    bob = _bob(t)
    assert (bob.root / ".claude" / "rules" / "style").is_dir() == before
    _alice_to_v2(t, unique_repo, support=after)
    t.bob.pull()
    assert (bob.root / ".claude" / "rules" / "style").is_dir() == after
    snap = bob.snapshot()
    _assert_installed(*bob.status(runner=_warm(t)))
    # Part B: the first run records the proven pin, the second writes nothing.
    _assert_recorded(snap)
    _assert_silent(bob, _warm(t))


# ---------------------------------------------------------------------------
# An MCP entry whose value changed in a pull; a copy of a workspace with state
# ---------------------------------------------------------------------------

MCP_DESCRIPTOR = """\
description = "Record-cache test server."

[server]
transport = "stdio"
command = "grim"
args = [{args}]
"""


def _release_mcp(t: Team, repo: str, version: str, args: str) -> None:
    descriptor = t.alice.work.parent / "src" / "srv.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(
        MCP_DESCRIPTOR.format(args=args), encoding="utf-8", newline="\n"
    )
    ref = f"{REGISTRY_HOST}/{repo}/srv:{version}"
    t.alice.runner.run("release", str(descriptor), ref, "--kind", "mcp")


def _pins(place: Place) -> tuple[dict[str, str], dict[str, str]]:
    """The digests the record and the lock name, by artifact name."""
    state = json.loads(place.state.read_text(encoding="utf-8"))
    lock = tomllib.loads((place.root / "grimoire.lock").read_text(encoding="utf-8"))
    rows = [*lock.get("skill", []), *lock.get("rule", []), *lock.get("mcp", [])]
    return (
        {r["name"]: r["pinned"].rsplit("@", 1)[1] for r in state["records"]},
        {r["name"]: r["pinned"].rsplit("@", 1)[1] for r in rows},
    )


def test_status_after_a_pulled_mcp_value_change_records_the_pin_then_stays_silent(
    team, unique_repo
):
    # research_install_record_cache_test_matrix.md S36 (status half); plan scenario S-024
    t = team("CO")
    repo = unique_repo
    _release_mcp(t, repo, "1.0.0", '"mcp"')
    (t.alice.work / "grimoire.toml").write_text(
        f'[options]\nclients = ["claude"]\n\n[mcp]\nsrv = "{REGISTRY_HOST}/{repo}/srv:1"\n',
        encoding="utf-8",
        newline="\n",
    )
    t.alice.runner.run("lock")
    t.alice.runner.run("install")
    t.alice.commit_all("feat: v1")
    t.alice.push()
    t.bob.pull()
    t.bob.runner.run("install")
    _release_mcp(t, repo, "1.1.0", '"mcp", "--verbose"')
    t.alice.runner.run("update")
    t.alice.commit_all("chore: v2")
    t.alice.push()
    t.bob.pull()
    bob = _bob(t)
    record, lock = _pins(bob)
    assert record["srv"] != lock["srv"]
    rc, doc, stderr = bob.status(runner=_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {"srv": "installed"}
    record, lock = _pins(bob)
    assert record["srv"] == lock["srv"]
    snap = state_snapshot(bob.state, bob.root / ".mcp.json")
    rc, doc, stderr = bob.status(runner=_warm(t))
    assert rc == 0, doc
    assert _states(doc) == {"srv": "installed"}
    assert "adopted" not in stderr
    assert_unchanged(snap)


def test_status_over_a_copy_of_the_workspace_and_its_state_reads_installed_and_writes_nothing(
    team, unique_repo, tmp_path
):
    # research_install_record_cache_test_matrix.md S23 (status half); plan scenario S-009
    t = _v1(team, unique_repo)
    bob = _bob(t)
    copy = tmp_path / "copy"
    shutil.copytree(bob.root, copy, symlinks=True)
    runner = GrimRunner(bob.runner.binary, bob.runner.grim_home, cwd=copy)
    runner.env = dict(bob.runner.env)
    place = _project(runner)
    snap = place.snapshot()
    rc, doc, stderr = place.status()
    _assert_installed(rc, doc, stderr)
    assert_unchanged(snap)
