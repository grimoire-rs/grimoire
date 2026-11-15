# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Offline, a version claim reads only what this machine already holds.

A claim's target tag (`2` for `x:2.5`) comes from the tag cache and a held pin's
version from the store. A claim the target tag does not answer needs the
registry's tag listing, which offline is always a miss (69), even where the tag
cache holds a tag that would answer it. A target tag the cache lacks is a miss
too. A held pin whose manifest the store lacks, and whose tag and recorded edges
do not show whether it meets the claim, is kept, with one warning that nothing
checked it against the claim. That covers a pin with no version tag and a pin
kept under a wider float.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
import tomllib

from src.helpers import backport, make_dependent, publish_release, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact
from src.runner import GrimRunner

_UNCHECKED = (
    "not in the local store while offline and neither its tag nor the lock's "
    "recorded edges show whether it meets the claim"
)


def _files(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}


def _ref(repo: str, tag: str | None = None) -> str:
    return f"{REGISTRY_HOST}/{repo}" + (f":{tag}" if tag else "")


def _claim(
    unique_repo: str, name: str, target: str, tag: str | None
) -> PublishedArtifact:
    """A dependent `name` whose one edge requires skill `target` at `tag`."""
    edge = ("skill", target, _ref(f"{unique_repo}/{target}", tag))
    return make_dependent(f"{unique_repo}/{name}", "1", "skill", _files(name), [edge])


def _pins(project_dir: Path) -> dict[str, str]:
    text = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    return {e["name"]: e["pinned"] for e in tomllib.loads(text).get("skill", [])}


def _offline(
    runner: GrimRunner, route: str, *args: str
) -> subprocess.CompletedProcess[str]:
    """Run `args` offline under `--format json`, by the global flag or `GRIM_OFFLINE`."""
    if route == "env":
        runner.env["GRIM_OFFLINE"] = "1"
        return runner.run(*args, format="json", check=False)
    return runner.run("--offline", *args, format="json", check=False)


def _offline_miss(result: subprocess.CompletedProcess[str], reference: str) -> str:
    """The 69 error document naming `reference`; returns its message."""
    assert result.returncode == 69, result.stderr
    error = json.loads(result.stdout)["error"]
    assert (error["code"], error["exit"]) == ("unavailable", 69), error
    assert f"({reference}): registry unreachable" in error["message"], error
    assert "offline mode blocked a required network operation" in error["message"]
    return error["message"]


def _evict_from_store(grim_home: Path, digest: str) -> None:
    algorithm, hex_digest = digest.split(":")
    (grim_home / "blobs" / algorithm / hex_digest[:2] / hex_digest).unlink()


def _tag_cache(grim_home: Path, repo: str) -> tuple[Path, dict]:
    """The tag cache file of `repo` and what it holds."""
    for path in (grim_home / "tags").rglob("tags.json"):
        cache = json.loads(path.read_text(encoding="utf-8"))
        if cache["repository"] == repo:
            return path, cache
    raise AssertionError(f"no tag cache file for {repo}")


def _forget_tag(grim_home: Path, repo: str, tag: str) -> None:
    """Drop `repo:tag` from the tag cache, keeping every other tag it holds."""
    path, cache = _tag_cache(grim_home, repo)
    del cache["tags"][tag]
    path.write_text(json.dumps(cache), encoding="utf-8", newline="\n")


@pytest.mark.parametrize("route", ["flag", "env"])
def test_an_offline_claim_that_needs_the_tag_listing_exits_69(
    grim_at, grim_home: Path, project_dir: Path, unique_repo: str, route: str
) -> None:
    """After a backport `2` names 2.3.7, so `x:2.5` finds 2.9.0 only in the listing.

    The online lock resolved `2.9.0` through the listing, so the tag cache holds
    it. Offline the held pin's own tag still shows 2.9.0 and the relock keeps it;
    with no lock the claim needs the listing again, and the cached tag is no
    substitute.
    """
    x = f"{unique_repo}/x"
    publish_release(x, "2.3.7", _files("x"))
    v290 = publish_release(x, "2.9.0", _files("x"))
    backport(x, "2.3.7")
    c = _claim(unique_repo, "c", "x", "2.5")
    write_config(project_dir, skills={"c": c.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    pinned = f"{_ref(x, '2.9.0')}@{v290.digest}"
    assert _pins(project_dir)["x"] == pinned
    assert _tag_cache(grim_home, x)[1]["tags"]["2.9.0"] == v290.digest
    lock = project_dir / "grimoire.lock"

    relocked = _offline(runner, route, "lock")

    assert relocked.returncode == 0, relocked.stderr
    assert _pins(project_dir)["x"] == pinned
    lock.unlink()

    fresh = _offline(runner, route, "lock")

    message = _offline_miss(fresh, _ref(x, "2.5"))
    assert f"required by skill/c ({_ref(c.repo)})" in message, message
    assert not lock.exists()


def test_an_offline_lock_keeps_a_held_pin_the_store_lacks_with_one_warning(
    grim_at, grim_home: Path, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """`b` claims any `x`, so `x` is held at `x:latest`; then `c` claims `x:2`.

    `c` is new to the lock, so its claim was never recorded, and `b`'s bare edge
    and the pin's own `latest` show no version. With `x`'s manifest evicted
    nothing shows the pin's version offline: the pin stays, one warning names it
    and the claim it was not checked against, and online the check passes in
    silence.
    """
    x = f"{unique_repo}/x"
    v230 = publish_release(x, "2.3.0", _files("x"))
    b = _claim(unique_repo, "b", "x", None)
    c = _claim(unique_repo, "c", "x", "2")
    write_config(project_dir, skills={"b": b.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    held = f"{_ref(x, 'latest')}@{v230.digest}"
    assert _pins(project_dir)["x"] == held
    # Another project on this machine resolves `c`, so `c` reads offline.
    other = tmp_path / "other"
    (other / ".claude").mkdir(parents=True)
    write_config(other, skills={"c": c.fq})
    grim_at(other).run("lock")
    _evict_from_store(grim_home, v230.digest)
    write_config(project_dir, skills={"b": b.fq, "c": c.fq})

    result = runner.run("--offline", "lock", check=False)

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["x"] == held
    assert pins["c"] == f"{c.fq}@{c.digest}"
    warned = [line for line in result.stderr.splitlines() if _UNCHECKED in line]
    assert len(warned) == 1, result.stderr
    assert (
        f"{held}: {_UNCHECKED}, so the locked pin is kept without checking it "
        "against ^2" in warned[0]
    ), warned

    online = runner.run("lock", check=False)

    assert online.returncode == 0, online.stderr
    assert _pins(project_dir)["x"] == held
    assert _UNCHECKED not in online.stderr, online.stderr


def test_an_offline_lock_keeps_a_held_float_pin_the_store_lacks_with_one_warning(
    grim_at, grim_home: Path, project_dir: Path, tmp_path: Path, unique_repo: str
) -> None:
    """`b` claims `x:1`, so `x` is held at `x:1`; then `c` claims `x:1.2`.

    The pin's tag `1` and `b`'s recorded edge show a version in `[1, 2)`, which
    `x:1.2` neither takes whole nor rules out. With `x`'s manifest evicted the
    pin stays, one warning names it and the line `^1, ^1.2` it was not checked
    against, and the warning does not say its tag names no version. Online,
    1.3.0 meets the line in silence.
    """
    x = f"{unique_repo}/x"
    v130 = publish_release(x, "1.3.0", _files("x"))
    b = _claim(unique_repo, "b", "x", "1")
    c = _claim(unique_repo, "c", "x", "1.2")
    write_config(project_dir, skills={"b": b.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    held = f"{_ref(x, '1')}@{v130.digest}"
    assert _pins(project_dir)["x"] == held
    # Another project on this machine resolves `c`, so `c` reads offline.
    other = tmp_path / "other"
    (other / ".claude").mkdir(parents=True)
    write_config(other, skills={"c": c.fq})
    grim_at(other).run("lock")
    _evict_from_store(grim_home, v130.digest)
    write_config(project_dir, skills={"b": b.fq, "c": c.fq})

    result = runner.run("--offline", "lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == held
    warned = [line for line in result.stderr.splitlines() if _UNCHECKED in line]
    assert len(warned) == 1, result.stderr
    assert (
        f"{held}: {_UNCHECKED}, so the locked pin is kept without checking it "
        "against ^1, ^1.2" in warned[0]
    ), warned
    assert "names no version" not in result.stderr, result.stderr

    online = runner.run("lock", check=False)

    assert online.returncode == 0, online.stderr
    assert _pins(project_dir)["x"] == held
    assert _UNCHECKED not in online.stderr, online.stderr


@pytest.mark.parametrize("route", ["flag", "env"])
def test_an_offline_claim_whose_target_tag_the_cache_lacks_exits_69(
    grim_at, grim_home: Path, project_dir: Path, unique_repo: str, route: str
) -> None:
    """`a` claims `x:2` while `latest` names 3.0.0, so the claim reads tag `2`.

    With no lock, an offline lock resolves the claim through the cached `2`.
    Once that one entry is gone it exits 69, though the cache still holds `a:1`
    and `x:latest`.
    """
    x = f"{unique_repo}/x"
    v260 = publish_release(x, "2.6.0", _files("x"))
    publish_release(x, "3.0.0", _files("x"))
    a = _claim(unique_repo, "a", "x", "2")
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    pinned = f"{_ref(x, '2')}@{v260.digest}"
    assert _pins(project_dir)["x"] == pinned
    lock = project_dir / "grimoire.lock"
    lock.unlink()

    cached = _offline(runner, route, "lock")

    assert cached.returncode == 0, cached.stderr
    assert _pins(project_dir)["x"] == pinned
    lock.unlink()
    _forget_tag(grim_home, x, "2")

    assert "latest" in _tag_cache(grim_home, x)[1]["tags"]

    missed = _offline(runner, route, "lock")

    message = _offline_miss(missed, _ref(x, "2"))
    assert f"required by skill/a ({_ref(a.repo)})" in message, message
    assert not lock.exists()
