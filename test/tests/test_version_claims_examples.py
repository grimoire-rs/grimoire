# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Dependency tags read as version claims, end to end.

The first part runs the worked examples: a node resolves to the newest release
every claim on it allows, a version dropped from the graph keeps the narrowing
it caused, a move only a dropped version asked for refuses, and a moved pin
whose files were edited is saved to the lock but refused on disk.

The second part covers held pins: a plain `grim lock` keeps a pin while it
answers every claim, a new claim it does not answer moves it, and only
`grim update` moves a pin whose claims still hold.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest
import tomllib

from src.helpers import (
    make_artifact,
    make_dependent,
    publish_lie,
    publish_release,
    write_config,
)
from src.registry import REGISTRY_HOST, PublishedArtifact, retag, tag_digest
from src.runner import GrimRunner

# Strips `:tag` from `pinned` only: a registry port is followed by `/`, never
# by `@sha256:`, so `host:5000/repo@sha256:…` survives untouched.
_ADVISORY_TAG = re.compile(r'(?m)^(pinned = "[^"@]*):[^/"@]+(@sha256:)')
_DROPPED = "which this resolve dropped"


def _files(name: str, body: str = "") -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"
    }


def _edge(name: str, tag: str | None = None) -> tuple[str, str, str]:
    return ("skill", name, f"./{name}" if tag is None else f"./{name}:{tag}")


def _release(
    unique_repo: str,
    name: str,
    version: str,
    requires: list[tuple[str, str, str]] | None = None,
) -> PublishedArtifact:
    """Publish `version` of skill `name` with its cascade tags, annotated."""
    return publish_release(
        f"{unique_repo}/{name}", version, _files(name, version), requires
    )


def _dependent(
    unique_repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    """An unannotated skill at tag `1` that requires `requires`."""
    return make_dependent(f"{unique_repo}/{name}", "1", "skill", _files(name), requires)


def _pins(project_dir: Path) -> dict[str, str]:
    lock = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    return {row["name"]: row["pinned"] for row in lock.get("skill", [])}


def _digests(project_dir: Path) -> dict[str, str]:
    return {name: pin.split("@", 1)[1] for name, pin in _pins(project_dir).items()}


def _strip_advisory_tags(project_dir: Path) -> None:
    """Rewrite the lock as a build before `pinned` carried its tag wrote it."""
    path = project_dir / "grimoire.lock"
    text = _ADVISORY_TAG.sub(r"\1\2", path.read_text(encoding="utf-8"))
    path.write_text(text, encoding="utf-8", newline="\n")
    assert not re.search(r'(?m)^pinned = "[^"]*:[^/"@]+@', text), text


def _move_tag(repo: str, tag: str, body: str) -> PublishedArtifact:
    """Point `tag` at new unannotated bytes, keeping the old manifest tagged."""
    retag(repo, "keep", tag_digest(repo, tag))
    name = repo.rsplit("/", 1)[1]
    return make_artifact(repo, "skill", _files(name, body), tag=tag)


def _project(tmp_path: Path, name: str) -> Path:
    project = tmp_path / name
    (project / ".claude").mkdir(parents=True)
    return project


# ---------------------------------------------------------------------------
# Worked examples
# ---------------------------------------------------------------------------


def _publish_newest_within_claims(unique_repo: str) -> dict[str, PublishedArtifact]:
    """P 3.0.0 asks for C `0.2`; M asks for P `2`; K asks for C `0`."""
    g = [_release(unique_repo, "g", v) for v in ("0.1.0", "0.3.0")]
    c = [
        _release(unique_repo, "c", "0.2.0", [_edge("g", "0.1")]),
        _release(unique_repo, "c", "0.5.0", [_edge("g", "0.3")]),
    ]
    p = [
        _release(unique_repo, "p", "2.0.0"),
        _release(unique_repo, "p", "3.0.0", [_edge("c", "0.2")]),
    ]
    _release(unique_repo, "k", "1.0.0", [_edge("c", "0")])
    return {"g": g[1], "c": c[1], "p": p[0]}


def test_lock_resolves_each_node_to_the_newest_release_its_claims_allow(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """M's `2` holds P below its newest major, so P 3.0.0's claim on C never binds.

    Renaming M to Z changes the order the graph is walked in, never the pins.
    """
    expected = _publish_newest_within_claims(unique_repo)
    _release(unique_repo, "m", "1.0.0", [_edge("p", "2")])
    _release(unique_repo, "z", "1.0.0", [_edge("p", "2")])
    root = _release(
        unique_repo, "a", "1.0.0", [_edge("p"), _edge("m", "1"), _edge("k", "1")]
    )
    renamed_root = _release(
        unique_repo, "a", "2.0.0", [_edge("p"), _edge("z", "1"), _edge("k", "1")]
    )
    write_config(project_dir, skills={"a": root.fq})
    renamed = _project(tmp_path, "renamed")
    write_config(renamed, skills={"a": renamed_root.fq})

    result = grim_at(project_dir).run("lock", check=False)
    renamed_result = grim_at(renamed).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert renamed_result.returncode == 0, renamed_result.stderr
    assert _DROPPED not in result.stderr + renamed_result.stderr
    digests = _digests(project_dir)
    assert {n: digests[n] for n in ("p", "c", "g")} == {
        n: art.digest for n, art in expected.items()
    }, digests
    renamed_digests = _digests(renamed)
    shared = ("p", "c", "g", "k")
    assert {n: renamed_digests[n] for n in shared} == {n: digests[n] for n in shared}


def test_lock_keeps_the_narrowing_a_dropped_version_caused_and_says_so(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """N1 3.0.0 narrows N2 to `1`, whose N2 1.0.0 then drops N1 3.0.0.

    The narrowing stays, so the resolve ends on N1 2.0.0 and N2 1.0.0 rather
    than going round again, and one stderr line names the dropped version.
    """
    n1_two = _release(unique_repo, "n1", "2.0.0")
    n2_one = _release(unique_repo, "n2", "1.0.0", [_edge("n1", "2")])
    _release(unique_repo, "n2", "2.0.0")
    _release(unique_repo, "n1", "3.0.0", [_edge("n2", "1")])
    root = _release(unique_repo, "a", "1.0.0", [_edge("n1"), _edge("n2")])
    write_config(project_dir, skills={"a": root.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    digests = _digests(project_dir)
    assert (digests["n1"], digests["n2"]) == (n1_two.digest, n2_one.digest), digests
    lines = [line for line in result.stderr.splitlines() if _DROPPED in line]
    assert len(lines) == 1, result.stderr
    assert (
        "skill/n2 stays in ^1 because skill/n1 3.0.0, which this resolve dropped, "
        "required 1"
    ) in lines[0], lines[0]


def _x_pulls_q_past_a_dropped_claim(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, PublishedArtifact, PublishedArtifact, PublishedArtifact]:
    """X pulls U, P and Q up a minor each; Q 1.8.0 claims R `2`, Q 1.6.0 claimed `1.5`.

    The project locked A over Q 1.6.0 and R 1.5.3 before X and R 2.0.0 shipped.
    Returns the runner, A, X and R 2.0.0.
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
    x = _release(unique_repo, "x", "1.0.0", [_edge("u", "1.5")])
    return runner, root, x, r_two


def _r_two(unique_repo: str) -> str:
    return f"{REGISTRY_HOST}/{unique_repo}/r:2"


def test_add_refuses_a_move_only_a_dropped_version_asked_for_and_names_both_fixes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Q 1.6.0 left the graph, so its claim cannot decide R and `grim add` refuses
    with the two fixes, leaving `grimoire.toml` as it was. A bare `grim update`
    resolves afresh and lands on R 2.0.0, after which the add goes through.
    """
    runner, _, x, r_two = _x_pulls_q_past_a_dropped_claim(
        grim_at, project_dir, unique_repo
    )
    config = project_dir / "grimoire.toml"
    config_before = config.read_bytes()
    lock_before = (project_dir / "grimoire.lock").read_bytes()

    result = runner.run("add", x.fq, check=False)

    assert result.returncode == 78, result.stderr
    message = " ".join(result.stderr.split())
    assert "^1.5, ^2 cannot all hold" in message, message
    assert "skill/q 1.6.0, which claimed 1.5, is no longer in the graph" in message
    assert (
        f"declare skill r as `{_r_two(unique_repo)}` in grimoire.toml, "
        f"then rerun `grim add {x.fq}`, or run `grim update`, "
        f"then rerun `grim add {x.fq}`"
    ) in message, message
    assert config.read_bytes() == config_before
    assert (project_dir / "grimoire.lock").read_bytes() == lock_before

    update = runner.run("update", check=False)

    assert update.returncode == 0, update.stderr
    assert _digests(project_dir)["r"] == r_two.digest

    added = runner.run("add", x.fq, check=False)

    assert added.returncode == 0, added.stderr
    digests = _digests(project_dir)
    assert (digests["r"], digests["x"]) == (r_two.digest, x.digest), digests


def test_lock_refuses_a_move_only_a_dropped_version_asked_for_until_update(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Declaring X by hand meets the refusal on `grim lock`; a bare update
    resolves every pin afresh, X with them."""
    runner, root, x, r_two = _x_pulls_q_past_a_dropped_claim(
        grim_at, project_dir, unique_repo
    )
    write_config(project_dir, skills={"a": root.fq, "x": x.fq})
    lock_before = (project_dir / "grimoire.lock").read_bytes()

    result = runner.run("lock", check=False)

    assert result.returncode == 78, result.stderr
    assert (
        f"declare skill r as `{_r_two(unique_repo)}` in grimoire.toml, "
        "or run `grim update` to resolve every pin afresh"
    ) in " ".join(result.stderr.split()), result.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == lock_before

    update = runner.run("update", check=False)

    assert update.returncode == 0, update.stderr
    digests = _digests(project_dir)
    assert (digests["r"], digests["x"]) == (r_two.digest, x.digest), digests


@pytest.mark.parametrize("command", ["lock", "add"])
def test_declaring_the_surviving_claim_clears_a_move_only_a_dropped_version_asked_for(
    grim_at, project_dir: Path, registry: str, unique_repo: str, command: str
) -> None:
    runner, root, x, r_two = _x_pulls_q_past_a_dropped_claim(
        grim_at, project_dir, unique_repo
    )
    declared = {"a": root.fq, "x": x.fq} if command == "lock" else {"a": root.fq}
    write_config(project_dir, skills=declared)
    args = ("lock",) if command == "lock" else ("add", x.fq)
    refused = runner.run(*args, check=False)
    assert refused.returncode == 78, refused.stderr

    write_config(project_dir, skills={**declared, "r": _r_two(unique_repo)})
    result = runner.run(*args, check=False)

    assert result.returncode == 0, result.stderr
    digests = _digests(project_dir)
    assert (digests["r"], digests["x"]) == (r_two.digest, x.digest), digests


def test_add_saves_a_moved_pin_whose_edited_files_it_refuses_to_overwrite(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """X's `2.4` moves R from 2.1.0 to 2.4.0; R's installed file was edited.

    The lock moves and the disk refuses: `grim add` saves the lock and exits
    65 naming R and `grim install --force`, `--force` on the add stays scoped
    to X, and `grim install --force` writes R without moving any pin again.
    """
    _release(unique_repo, "r", "2.1.0")
    root = _release(unique_repo, "a", "1.0.0", [_edge("r", "2")])
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.run("add", root.fq)
    r_new = _release(unique_repo, "r", "2.4.0")
    installed = project_dir / ".claude" / "skills" / "r" / "SKILL.md"
    installed.write_text("hand edited\n", encoding="utf-8", newline="\n")
    x = _release(unique_repo, "x", "1.0.0", [_edge("r", "2.4")])

    first = runner.run("add", x.fq, check=False)

    assert first.returncode == 65, first.stderr
    pins = _pins(project_dir)
    assert pins["r"].endswith(r_new.digest), pins
    assert "skill 'r'" in first.stderr, first.stderr
    assert "grim install --force" in first.stderr, first.stderr
    assert installed.read_text(encoding="utf-8") == "hand edited\n"

    forced = runner.run("add", "--force", x.fq, check=False)

    assert forced.returncode == 65, forced.stderr
    assert "grim install --force" in forced.stderr, forced.stderr
    assert _pins(project_dir) == pins
    assert installed.read_text(encoding="utf-8") == "hand edited\n"

    runner.run("install", "--force")

    assert _pins(project_dir) == pins
    assert "2.4.0" in installed.read_text(encoding="utf-8")


# ---------------------------------------------------------------------------
# Held pins
# ---------------------------------------------------------------------------


def test_update_moves_a_tagless_pin_a_plain_lock_kept(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A tagless, unannotated pin reads its version from the edge recorded onto it.

    The lock is in the shape a build before `pinned` carried its tag wrote.
    `x:2` moving keeps the pin under `grim lock`; `grim update` moves it.
    """
    repo = f"{unique_repo}/x"
    held = make_artifact(repo, "skill", _files("x", "held"), tag="2")
    q = _dependent(unique_repo, "q", [_edge("x", "2")])
    write_config(project_dir, skills={"q": q.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _strip_advisory_tags(project_dir)
    moved = _move_tag(repo, "2", "moved")

    kept = runner.json("lock")

    rows = {row["name"]: row for row in kept["items"]}
    assert _digests(project_dir)["x"] == held.digest
    assert rows["x"]["disposition"] != "moved", rows["x"]

    runner.run("update")

    assert _digests(project_dir)["x"] == moved.digest


def _nested_edges_onto_a_tagless_pin(
    unique_repo: str, project_dir: Path, runner: GrimRunner
) -> PublishedArtifact:
    """Lock an unannotated `x` that P1 claims as `2` and P2 as `2.1`, then strip its tag."""
    repo = f"{unique_repo}/x"
    held = make_artifact(repo, "skill", _files("x", "held"), tag="2")
    retag(repo, "2.1", held.digest)
    p1 = _dependent(unique_repo, "p1", [_edge("x", "2")])
    p2 = _dependent(unique_repo, "p2", [_edge("x", "2.1")])
    write_config(project_dir, skills={"p1": p1.fq, "p2": p2.fq})
    runner.run("lock")
    assert _digests(project_dir)["x"] == held.digest
    _strip_advisory_tags(project_dir)
    return held


def test_add_keeps_a_tagless_pin_the_narrowest_recorded_edge_places_in_a_new_claim(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`2.1` nests in `2`, so the pin stands in [2.1, 2.2) and answers a new `2.1`.

    Read from `2` alone it would stand in [2, 3) and move to 2.1.9.
    """
    runner = grim_at(project_dir)
    held = _nested_edges_onto_a_tagless_pin(unique_repo, project_dir, runner)
    _release(unique_repo, "x", "2.1.9")
    p3 = _dependent(unique_repo, "p3", [_edge("x", "2.1")])

    result = runner.run("add", p3.fq, check=False)

    assert result.returncode == 0, result.stderr
    assert _digests(project_dir)["x"] == held.digest


def test_add_moves_a_tagless_pin_the_narrowest_recorded_edge_places_below_a_new_claim(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A pin in [2.1, 2.2) does not answer a new `2.3` and moves to 2.3.0."""
    runner = grim_at(project_dir)
    _nested_edges_onto_a_tagless_pin(unique_repo, project_dir, runner)
    target = _release(unique_repo, "x", "2.3.0")
    p3 = _dependent(unique_repo, "p3", [_edge("x", "2.3")])

    result = runner.run("add", p3.fq, check=False)

    assert result.returncode == 0, result.stderr
    assert _digests(project_dir)["x"] == target.digest


def test_lock_moves_a_held_pin_a_new_range_claim_rules_out(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A new parent's `2` rules out the held 1.2.0, so `grim lock` moves it to 2.0.0.

    The release alone moves nothing: the pin answers every claim it had.
    """
    held = _release(unique_repo, "x", "1.2.0")
    p1 = _dependent(unique_repo, "p1", [_edge("x")])
    write_config(project_dir, skills={"p1": p1.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    target = _release(unique_repo, "x", "2.0.0")
    runner.run("lock")
    assert _digests(project_dir)["x"] == held.digest
    p2 = _dependent(unique_repo, "p2", [_edge("x", "2")])
    write_config(project_dir, skills={"p1": p1.fq, "p2": p2.fq})

    report = runner.json("lock")

    rows = {row["name"]: row for row in report["items"]}
    assert _digests(project_dir)["x"] == target.digest
    assert rows["x"]["disposition"] == "moved", rows["x"]


@pytest.mark.parametrize(("annotated", "exit_code"), [("3.1.0", 78), ("2.5.0", 0)])
def test_lock_refuses_a_held_pin_its_recorded_claim_no_longer_admits(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    annotated: str,
    exit_code: int,
) -> None:
    """P1 claims `x:2` and P2 the literal `x:v2`; the pin is then hand-set to a lie.

    A digest that says 3.1.0 fails the recorded `2`, and with a literal claim on
    the node the pin cannot be let go, so `grim lock` exits 78 and writes
    nothing. One that says 2.5.0 answers `2` and stays.
    """
    repo = f"{unique_repo}/x"
    honest = _release(unique_repo, "x", "2.0.0")
    retag(repo, "v2", honest.digest)
    p1 = _dependent(unique_repo, "p1", [_edge("x", "2")])
    p2 = _dependent(unique_repo, "p2", [_edge("x", "v2")])
    write_config(project_dir, skills={"p1": p1.fq, "p2": p2.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _digests(project_dir)["x"] == honest.digest
    lie = publish_lie(repo, "lie", annotated, _files("x", "lie"))
    lock = project_dir / "grimoire.lock"
    text = lock.read_text(encoding="utf-8")
    assert text.count(honest.digest) == 1, text
    lock.write_text(
        text.replace(honest.digest, lie.digest), encoding="utf-8", newline="\n"
    )
    before = lock.read_bytes()

    result = runner.run("lock", check=False)

    assert result.returncode == exit_code, result.stderr
    assert lock.read_bytes() == before
    if exit_code:
        assert "skill 'x'" in result.stderr, result.stderr
        assert "conflicting versions" in result.stderr, result.stderr
