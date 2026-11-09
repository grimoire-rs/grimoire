# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Dependency resolution end to end: `grim lock`, `grim update`, `grim add --no-install`.

Fixtures publish dependents with `make_dependent` (a requires layer after the
content layer), plain artifacts with `make_artifact`, and bundles with
`make_bundle`. A relative requires id (`./b:1`) resolves against the
repository directory of the artifact that publishes it.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

from src.helpers import make_artifact, make_bundle, make_dependent, write_config
from src.registry import REGISTRY_HOST, retag


def _skill_files(name: str, body: str = "") -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\n---\n# {name} {body}\n"}


def _leaf(unique_repo: str, name: str, tag: str = "1", body: str = ""):
    return make_artifact(
        f"{unique_repo}/{name}", "skill", _skill_files(name, body), tag=tag
    )


def _dependent(
    unique_repo: str,
    name: str,
    requires: list[tuple[str, str, str]],
    tag: str = "1",
    annotations: dict[str, str] | None = None,
):
    return make_dependent(
        f"{unique_repo}/{name}",
        tag,
        "skill",
        _skill_files(name),
        requires,
        annotations=annotations,
    )


def _lock(project_dir: Path) -> str:
    return (project_dir / "grimoire.lock").read_text()


def _entries(lock_text: str, table: str = "skill") -> dict[str, str]:
    """`name -> pinned` for every `[[<table>]]` entry of a lock."""
    out: dict[str, str] = {}
    for block in lock_text.split(f"[[{table}]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        pinned = re.search(r'^pinned = "([^"]+)"', block, re.M)
        if name and pinned:
            out[name.group(1)] = pinned.group(1)
    return out


def _error(result) -> dict:
    return json.loads(result.stdout)["error"]


def test_transitive_chain_locks(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    c = _leaf(unique_repo, "c")
    b = _dependent(unique_repo, "b", [("skill", "c", "./c:1")])
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)

    rows = runner.json("lock")["items"]

    assert {r["name"] for r in rows} == {"a", "b", "c"}
    lock = _lock(project_dir)
    pins = _entries(lock)
    assert pins["a"].endswith(a.digest)
    assert pins["b"].endswith(b.digest)
    assert pins["c"].endswith(c.digest)
    # The edges are recorded as published, relative ids included.
    assert lock.count("[[skill.requires]]") == 2
    assert 'id = "./b:1"' in lock and 'id = "./c:1"' in lock

    # A transitive node keeps its pin when its tag moves: only `grim update`
    # moves a pin, and `grim lock` reads the carried node by digest.
    c2 = make_artifact(
        f"{unique_repo}/c", "skill", _skill_files("c", "v2"), tag="2"
    )
    retag(f"{unique_repo}/c", "1", c2.digest)
    runner.run("lock")
    assert _entries(_lock(project_dir))["c"].endswith(c.digest)


def test_add_no_install_locks_the_closure(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    c = _leaf(unique_repo, "c")
    a = _dependent(unique_repo, "a", [("skill", "c", "./c:1")])
    write_config(project_dir)
    runner = grim_at(project_dir)

    runner.run("add", "--no-install", "--kind", "skill", "--name", "a", a.fq)

    pins = _entries(_lock(project_dir))
    assert set(pins) == {"a", "c"}
    assert pins["c"].endswith(c.digest)
    # Only the target is declared; the dependency is not.
    assert "c =" not in (project_dir / "grimoire.toml").read_text()


def test_nested_bundle_expands(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    s = _leaf(unique_repo, "s")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "s", s.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    write_config(project_dir, bundles={"outer": outer.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    lock = _lock(project_dir)
    assert _entries(lock)["s"].endswith(s.digest)
    bundles = lock.split("[[bundle]]")[1:]
    assert len(bundles) == 2, lock
    inner_block = next(b for b in bundles if f"{unique_repo}/inner" in b.split("[[")[0])
    assert "transitive = true" in inner_block
    outer_block = next(b for b in bundles if f"{unique_repo}/outer" in b.split("[[")[0])
    assert "transitive" not in outer_block


def test_diamond_equal_digest_merges(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    c = _leaf(unique_repo, "c")
    a = _dependent(unique_repo, "a", [("skill", "c", "./c:1")])
    b = _dependent(unique_repo, "b", [("skill", "c", "./c:1")])
    write_config(project_dir, skills={"a": a.fq, "b": b.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    pins = _entries(_lock(project_dir))
    assert set(pins) == {"a", "b", "c"}
    assert pins["c"].endswith(c.digest)


def test_diamond_conflict_lists_all(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    for tag in ("1", "2", "3"):
        _leaf(unique_repo, "c", tag=tag, body=f"v{tag}")
    a = _dependent(unique_repo, "a", [("skill", "c", "./c:1")])
    b = _dependent(unique_repo, "b", [("skill", "c", "./c:2")])
    d = _dependent(unique_repo, "d", [("skill", "c", "./c:3")])
    write_config(project_dir, skills={"a": a.fq, "b": b.fq, "d": d.fq})
    runner = grim_at(project_dir)

    result = runner.run("lock", check=False)

    assert result.returncode == 78, result.stderr
    for tag in ("1", "2", "3"):
        assert f"c:{tag}" in result.stderr, result.stderr
    assert "declare skill c directly" in result.stderr, result.stderr
    assert not (project_dir / "grimoire.lock").exists()


def test_direct_overrides_edge(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _leaf(unique_repo, "c", tag="1", body="v1")
    c2 = _leaf(unique_repo, "c", tag="2", body="v2")
    a = _dependent(unique_repo, "a", [("skill", "c", "./c:1")])
    write_config(project_dir, skills={"a": a.fq, "c": c2.fq})
    runner = grim_at(project_dir)

    result = runner.run("lock")

    assert _entries(_lock(project_dir))["c"].endswith(c2.digest)
    assert "conflicting" not in result.stderr


def test_cycle_refused(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    _dependent(unique_repo, "b", [("skill", "a", "./a:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)

    result = runner.run("lock", check=False)

    assert result.returncode == 65, result.stderr
    assert "dependency cycle" in result.stderr, result.stderr
    assert "skill/a" in result.stderr and "skill/b" in result.stderr, result.stderr
    assert not (project_dir / "grimoire.lock").exists()


def test_caps_refuse(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Depth: a chain of 34 nodes puts the last one at depth 33, past the cap
    # of 32. Each link is published before the one it requires exists.
    depth = 34
    for i in range(depth):
        requires = [("skill", f"n{i + 1}", f"./n{i + 1}:1")] if i + 1 < depth else []
        _dependent(unique_repo, f"n{i}", requires)
    root = f"{REGISTRY_HOST}/{unique_repo}/n0:1"
    write_config(project_dir, skills={"n0": root})
    runner = grim_at(project_dir)

    result = runner.run("lock", check=False)

    assert result.returncode == 65, result.stderr
    assert "dependency graph too large" in result.stderr, result.stderr
    assert "deeper than 32" in result.stderr, result.stderr

    # Nodes: five mids list 500 leaves each, 2500 distinct transitive nodes
    # against a cap of 2048. The leaves are never published, so the run only
    # ends at the cap if the cap fires before they are looked up.
    nodes_dir = project_dir / "nodes"
    nodes_dir.mkdir()
    mids = []
    for m in range(5):
        leaves = [("skill", f"l{m}x{j}", f"./l{m}x{j}:1") for j in range(500)]
        mids.append(_dependent(unique_repo, f"m{m}", leaves))
    top = _dependent(
        unique_repo, "top", [("skill", f"m{m}", f"./m{m}:1") for m in range(5)]
    )
    write_config(nodes_dir, skills={"top": top.fq})
    nodes_runner = grim_at(nodes_dir)

    result = nodes_runner.run("lock", check=False)

    assert result.returncode == 65, result.stderr
    assert "dependency graph too large" in result.stderr, result.stderr
    assert "more than 2048" in result.stderr, result.stderr


def test_missing_edge_target(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a = _dependent(unique_repo, "a", [("skill", "gone", "./gone:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)

    result = runner.run("lock", check=False)

    assert result.returncode == 79, result.stderr
    assert "tag not found" in result.stderr, result.stderr
    assert "required by skill/a" in result.stderr, result.stderr


def test_relative_ref_after_mirror(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # The same dependent published under a second namespace, as a mirror
    # does: its relative edge must land beside it, not beside the original.
    original = f"{unique_repo}/orig"
    mirror = f"{unique_repo}/mirror"
    make_artifact(f"{original}/b", "skill", _skill_files("b", "orig"), tag="1")
    mirrored_b = make_artifact(
        f"{mirror}/b", "skill", _skill_files("b", "mirror"), tag="1"
    )
    published = make_dependent(
        f"{mirror}/a", "1", "skill", _skill_files("a"), [("skill", "b", "./b:1")]
    )
    write_config(project_dir, skills={"a": published.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    pins = _entries(_lock(project_dir))
    assert pins["b"] == f"{REGISTRY_HOST}/{mirror}/b:1@{mirrored_b.digest}"


def test_cross_registry_edge(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # The registry answers to a second loopback name; an absolute edge keeps
    # the host it names.
    host, _, port = REGISTRY_HOST.partition(":")
    other = "localhost" if host != "localhost" else "127.0.0.1"
    other = f"{other}:{port}" if port else other
    b = _leaf(unique_repo, "b")
    a = _dependent(unique_repo, "a", [("skill", "b", f"{other}/{unique_repo}/b:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)
    runner.env["GRIM_INSECURE_REGISTRIES"] = f"{REGISTRY_HOST},{other}"

    runner.run("lock")

    pins = _entries(_lock(project_dir))
    assert pins["b"] == f"{other}/{unique_repo}/b:1@{b.digest}"
    assert pins["a"].startswith(f"{REGISTRY_HOST}/")


def test_resolve_refuses_unknown_feature(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    b = _dependent(
        unique_repo,
        "b",
        [],
        annotations={"com.grimoire.requires-features": "from-the-future"},
    )
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)

    result = runner.run("lock", format="json", check=False)

    assert result.returncode == 65, result.stderr
    err = _error(result)
    assert (err["exit"], err["reason"]) == (65, "unsupported-artifact-feature"), err
    assert "from-the-future" in err["message"], err
    assert "required by skill/a" in err["message"], err


def test_path_source_requires_change_is_stale(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    c1 = _leaf(unique_repo, "c1")
    c2 = _leaf(unique_repo, "c2")
    skill = project_dir / "skills" / "p" / "SKILL.md"
    skill.parent.mkdir(parents=True)

    def write(dep: str) -> None:
        skill.write_text(
            "---\nname: p\ndescription: Path skill.\nmetadata:\n"
            f'  grim.requires.skills.dep: "{dep}"\n---\n# p\n'
        )

    write(c1.fq)
    (project_dir / "grimoire.toml").write_text('[skills]\np = "./skills/p"\n')
    runner = grim_at(project_dir)

    runner.run("lock")

    lock = _lock(project_dir)
    assert _entries(lock)["dep"].endswith(c1.digest)
    assert f'id = "{c1.fq}"' in lock

    write(c2.fq)
    result = runner.run("install", "--client", "claude", format="json", check=False)

    assert result.returncode == 65, result.stderr
    # The refusal is the content-hash check; it carries no `stale-lock`
    # reason yet (install_error.rs is outside this pipeline's file set).
    assert "changed since the lock was written" in _error(result)["message"]


def _evict_from_store(grim_home: Path, digest: str) -> None:
    algorithm, hex_digest = digest.split(":")
    (grim_home / "blobs" / algorithm / hex_digest[:2] / hex_digest).unlink()


def test_offline_lock_carries_forward(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    b = _leaf(unique_repo, "b")
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    online = _lock(project_dir)

    # `update` rolls the lock first and installs after; the installer cannot
    # read a two-layer artifact yet, so its refusal is expected and the lock
    # it left behind is what this test reads.
    # The store holds the manifest and the requires layer: the edges are read,
    # not carried.
    result = runner.run("update", "--offline", check=False)
    assert "carried from the lock unverified" not in result.stderr
    assert _entries(_lock(project_dir)) == _entries(online)
    assert "[[skill.requires]]" in _lock(project_dir)

    # Evict the manifest: offline cannot read the edges, so the previous
    # lock's `requires` at the same digest stand in, unverified.
    _evict_from_store(grim_home, a.digest)
    result = runner.run("update", "--offline", check=False)
    assert "carried from the lock unverified" in result.stderr, result.stderr
    carried = _lock(project_dir)
    assert _entries(carried) == _entries(online)
    assert _entries(carried)["b"].endswith(b.digest)
    assert 'id = "./b:1"' in carried

    # With no previous entry there is nothing to carry: the node locks edgeless.
    (project_dir / "grimoire.lock").unlink()
    result = runner.run("update", "--offline", check=False)
    assert "carried from the lock unverified" in result.stderr, result.stderr
    bare = _lock(project_dir)
    assert set(_entries(bare)) == {"a"}
    assert "[[skill.requires]]" not in bare


def test_offline_lock_command_carries_forward(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    b = _leaf(unique_repo, "b")
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)

    # Positive control: online, the edges are read from the registry.
    runner.run("lock")
    online = _lock(project_dir)
    assert "[[skill.requires]]" in online

    # Store intact: offline reads the edges, nothing is carried.
    result = runner.run("lock", "--offline")
    assert "carried from the lock unverified" not in result.stderr
    assert _entries(_lock(project_dir)) == _entries(online)

    # Manifest evicted: `grim lock --offline` carries the previous lock's edges.
    _evict_from_store(grim_home, a.digest)
    result = runner.run("lock", "--offline")
    assert "carried from the lock unverified" in result.stderr, result.stderr
    carried = _lock(project_dir)
    assert _entries(carried) == _entries(online)
    assert _entries(carried)["b"].endswith(b.digest)
    assert 'id = "./b:1"' in carried
