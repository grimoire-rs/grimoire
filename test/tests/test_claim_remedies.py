# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A version-claim refusal prints only remedies that clear it when followed."""

from __future__ import annotations

import re
import shlex
from pathlib import Path

import tomllib

from src.helpers import backport, publish_release, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner

_COMMAND = re.compile(r"`grim ([^`]+)`")
_DROPPED = "which this resolve dropped, required "


def _files(name: str, body: str) -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"
    }


def _edge(name: str, tag: str) -> tuple[str, str, str]:
    return ("skill", name, f"./{name}:{tag}")


def _any(unique_repo: str, name: str) -> tuple[str, str, str]:
    """An edge onto `name` with no tag, which claims any version."""
    return ("skill", name, f"{REGISTRY_HOST}/{unique_repo}/{name}")


def _release(
    unique_repo: str,
    name: str,
    version: str,
    requires: list[tuple[str, str, str]] | None = None,
) -> PublishedArtifact:
    return publish_release(
        f"{unique_repo}/{name}", version, _files(name, version), requires
    )


def _digests(project_dir: Path) -> dict[str, str]:
    lock = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    return {row["name"]: row["pinned"].split("@", 1)[1] for row in lock["skill"]}


def test_a_named_update_refused_in_conflict_form_names_only_what_clears_it(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim update u` moves P and so Q to 1.8.0, which claims R `2`, while the
    held Q 1.6.0 taught R `1.5`. Naming R too still reads Q 1.6.0 first and
    refuses again, so the fix offers only declaring R, which then settles.
    """
    _release(unique_repo, "p", "1.0.0")
    _release(unique_repo, "u", "1.2.0", [_edge("p", "1")])
    _release(unique_repo, "r", "1.5.3")
    _release(unique_repo, "q", "1.6.0", [_edge("r", "1.5")])
    root = _release(
        unique_repo, "a", "1.0.0", [_edge("q", "1"), _edge("u", "1"), _edge("p", "1")]
    )
    write_config(project_dir, skills={"a": root.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    r_two = _release(unique_repo, "r", "2.0.0")
    _release(unique_repo, "q", "1.8.0", [_edge("r", "2")])
    _release(unique_repo, "p", "1.1.0", [_edge("q", "1.8")])
    _release(unique_repo, "u", "1.5.0", [_edge("p", "1.1")])

    refused = runner.run("update", "u", check=False)

    assert refused.returncode == 78, refused.stderr
    line = next(
        (text for text in refused.stderr.splitlines() if "cannot all hold" in text),
        None,
    )
    assert line is not None, refused.stderr
    fix = line.split("is no longer in the graph; ", 1)[1]
    declare = f"{REGISTRY_HOST}/{unique_repo}/r:2"
    assert fix.startswith(f"declare skill r as `{declare}` in grimoire.toml"), fix
    for command in _COMMAND.findall(fix):
        followed = runner.run(*shlex.split(command), check=False)
        assert followed.returncode == 0, f"`grim {command}`: {followed.stderr}"

    write_config(project_dir, skills={"a": root.fq, "r": declare})
    runner.run("lock")
    updated = runner.run("update", "u", check=False)

    assert updated.returncode == 0, updated.stderr
    assert _digests(project_dir)["r"] == r_two.digest


def _c_moved_only_by_a_dropped_claim(
    unique_repo: str, requires: list[tuple[str, str, str]] | None = None
) -> PublishedArtifact:
    """Publish `c` 2.9.0 asking `requires`, `p` 1.2.0 claiming `c:2.5` and 1.5.0
    claiming any `c`, with `p:1` on 1.2.0, and `y` 1.1.0 asking `p:1.4`. A node
    asking `p:1` and `y:1.1` reads `p` 1.2.0 and drops it, so a held `c` 2.3.0
    would move to 2.9.0 only for the dropped version's claim. Returns `c` 2.9.0."""
    moved = _release(unique_repo, "c", "2.9.0", requires)
    _release(unique_repo, "p", "1.2.0", [_edge("c", "2.5")])
    _release(unique_repo, "p", "1.5.0", [_any(unique_repo, "c")])
    # `grim release` never lowers a float; point `p:1` back as another tool may.
    backport(f"{unique_repo}/p", "1.2.0")
    _release(unique_repo, "y", "1.1.0", [_edge("p", "1.4")])
    return moved


def _dropped_claim_world(
    grim_at, project_dir: Path, unique_repo: str, *held: str
) -> tuple[GrimRunner, PublishedArtifact]:
    """`a` requires `c` and each of `held` with no tag and `u:1`, locked with `c`
    at 2.3.0, `u` at 1.0.0 and each of `held` at 2.0.0. Then `u` 1.1.0 asks `p:1`
    and `y:1.1`, so `c` would move to 2.9.0 only for a dropped version's claim.
    Returns the runner and `c` 2.9.0."""
    _release(unique_repo, "c", "2.3.0")
    _release(unique_repo, "u", "1.0.0")
    for name in held:
        _release(unique_repo, name, "2.0.0")
    edges = [_any(unique_repo, name) for name in ("c", *held)]
    root = _release(unique_repo, "a", "1.0.0", [*edges, _edge("u", "1")])
    write_config(project_dir, skills={"a": root.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _c_moved_only_by_a_dropped_claim(unique_repo)
    _release(unique_repo, "u", "1.1.0", [_edge("p", "1"), _edge("y", "1.1")])
    return runner, moved


def _fix_of(refused, claim: str) -> str:
    """The fix the pin-form refusal prints, after the dropped version's claim."""
    line = next(
        (text for text in refused.stderr.splitlines() if _DROPPED in text), None
    )
    assert line is not None, refused.stderr
    return line.split(f"{_DROPPED}{claim}; ", 1)[1]


def _follow(runner: GrimRunner, fix: str) -> str:
    """Run each command `fix` names, as printed, until one exits 0. Every command
    before it must refuse a major downgrade, which the one after it allows.
    Returns the command that settled."""
    commands = _COMMAND.findall(fix)
    assert len(commands) == 2, fix
    for command in commands:
        followed = runner.run(*shlex.split(command), check=False)
        if followed.returncode == 0:
            return command
        assert followed.returncode == 78, f"`grim {command}`: {followed.stderr}"
        assert "a lower major version" in followed.stderr, followed.stderr
    raise AssertionError(f"no command in the fix settles: {fix}")


def _refuse_lock(runner: GrimRunner, project_dir: Path, unique_repo: str) -> str:
    """Declare `u:1.1`, so `grim lock` reads `p` 1.2.0, drops it, and refuses to
    move the held `c`; return the fix it prints."""
    root = f"{REGISTRY_HOST}/{unique_repo}/a:1.0.0"
    write_config(
        project_dir, skills={"a": root, "u": f"{REGISTRY_HOST}/{unique_repo}/u:1.1"}
    )
    before = (project_dir / "grimoire.lock").read_bytes()

    refused = runner.run("lock", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "would move from 2.3.0 to 2.9.0" in refused.stderr, refused.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == before
    return _fix_of(refused, "2.5")


def test_a_lock_refused_for_a_dropped_claim_names_an_update_that_settles(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The fix names `grim update`, which, run as printed, settles `c` where the
    refusal said it would move."""
    runner, moved = _dropped_claim_world(grim_at, project_dir, unique_repo)
    fix = _refuse_lock(runner, project_dir, unique_repo)

    settled = _follow(runner, fix)

    assert settled == "update", fix
    assert _digests(project_dir)["c"] == moved.digest


def test_a_lock_fix_whose_update_meets_a_downgrade_settles_with_its_flag(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """As above, with `n1` and `n2` held at 2.0.0 and then `n1` 3.0.0 claiming
    `n2:1` and `n2` 1.0.0 claiming `n1:2`. The printed `grim update` drops `n1`
    3.0.0, whose claim alone takes `n2` down a major, so it refuses; the flagged
    update the fix names instead settles `c` and `n2`."""
    runner, moved = _dropped_claim_world(grim_at, project_dir, unique_repo, "n1", "n2")
    _release(unique_repo, "n1", "3.0.0", [_edge("n2", "1")])
    n2 = f"{unique_repo}/n2"
    # `grim release` never lowers a float, so `n2:latest` stays on 2.0.0.
    lower = publish_release(
        n2, "1.0.0", _files("n2", "1.0.0"), [_edge("n1", "2")], cascade=False
    )
    retag(n2, "1.0", lower.digest)
    retag(n2, "1", lower.digest)
    fix = _refuse_lock(runner, project_dir, unique_repo)

    settled = _follow(runner, fix)

    assert settled == "update --allow-downgrade", fix
    digests = _digests(project_dir)
    assert (digests["c"], digests["n2"]) == (moved.digest, lower.digest)


def test_a_named_update_refused_for_a_dropped_claim_names_the_node_too(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim update u` moves `u` to 1.1.0, which reads `p` 1.2.0 and drops it, and
    the held `c` would move only for its claim. The fix names `c` too, which, run
    as printed, settles `c` where the refusal said it would move."""
    runner, moved = _dropped_claim_world(grim_at, project_dir, unique_repo)
    before = (project_dir / "grimoire.lock").read_bytes()

    refused = runner.run("update", "u", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "would move from 2.3.0 to 2.9.0" in refused.stderr, refused.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == before
    fix = _fix_of(refused, "2.5")
    assert "name c too" in fix, fix
    settled = _follow(runner, fix)

    assert settled == "update -- u c", fix
    assert _digests(project_dir)["c"] == moved.digest


def test_a_named_update_fix_that_meets_a_downgrade_settles_with_its_flag(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`a` requires `c` and `u` with no tag, locked at 2.3.0 and 2.0.0. `grim
    update u` moves `u` to 2.1.0, which asks `p:1` and `y:1.1`, and refuses in pin
    form. Naming `c` too reads `c` 2.9.0, which asks any `k`; `k` 3.0.0 claims
    `u:1` and `u` 1.1.0 claims `k:2`, so the resolve drops `k` 3.0.0, yet its
    claim alone takes `u` down a major and the update refuses. The flagged update
    the fix names instead settles `c` and `u`."""
    _release(unique_repo, "c", "2.3.0")
    # Released below 2.0.0 first, so the cascade leaves `u:latest` on 2.0.0.
    lower = _release(unique_repo, "u", "1.1.0", [_edge("k", "2")])
    _release(unique_repo, "u", "2.0.0")
    edges = [_any(unique_repo, "c"), _any(unique_repo, "u")]
    root = _release(unique_repo, "a", "1.0.0", edges)
    write_config(project_dir, skills={"a": root.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _c_moved_only_by_a_dropped_claim(unique_repo, [_any(unique_repo, "k")])
    _release(unique_repo, "k", "2.0.0")
    _release(unique_repo, "k", "3.0.0", [_edge("u", "1")])
    _release(unique_repo, "u", "2.1.0", [_edge("p", "1"), _edge("y", "1.1")])

    refused = runner.run("update", "u", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "would move from 2.3.0 to 2.9.0" in refused.stderr, refused.stderr
    fix = _fix_of(refused, "2.5")
    assert "name c too" in fix, fix
    settled = _follow(runner, fix)

    assert settled == "update --allow-downgrade -- u c", fix
    digests = _digests(project_dir)
    assert (digests["c"], digests["u"]) == (moved.digest, lower.digest)
