# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A numeric tag on a dependency edge claims a version range, Cargo style.

`x:2` claims `>=2.0.0 <3.0.0`, `x:2.1` claims `>=2.1.0 <3.0.0`, no tag claims
any version. A node resolves through its major's cascade tag, else the newest
listed release in range. A held pin stays while every claim accepts it, two
majors on one node refuse with 78, and a move to a lower major needs
`--allow-downgrade`. Releases come from `publish_release`, which stamps the
version annotation and, published in ascending order as here, tags the way
`grim release` does.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest
import tomllib

from src.helpers import (
    backport,
    make_artifact,
    make_dependent,
    publish_lie,
    publish_release,
    write_config,
)
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner


def _files(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}


def _ref(repo: str, tag: str | None = None) -> str:
    return f"{REGISTRY_HOST}/{repo}" + (f":{tag}" if tag else "")


def _dependent(
    unique_repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    return make_dependent(f"{unique_repo}/{name}", "1", "skill", _files(name), requires)


def _claim(
    unique_repo: str, name: str, target: str, tag: str | None
) -> PublishedArtifact:
    """A dependent `name` whose one edge requires skill `target` at `tag`."""
    return _dependent(
        unique_repo, name, [("skill", target, _ref(f"{unique_repo}/{target}", tag))]
    )


def _text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _pins(project_dir: Path) -> dict[str, str]:
    lock = tomllib.loads(_text(project_dir / "grimoire.lock"))
    return {e["name"]: e["pinned"] for e in lock.get("skill", [])}


def _json_rows(runner: GrimRunner, *args: str) -> tuple[dict[str, dict], str]:
    """Run `args` under `--format json`, expect exit 0, return rows by name and stderr."""
    result = runner.run(*args, format="json", check=False)
    assert result.returncode == 0, result.stderr
    return {r["name"]: r for r in json.loads(result.stdout)["items"]}, result.stderr


def _lines(stderr: str, needle: str) -> list[str]:
    return [line for line in stderr.splitlines() if needle in line]


def _two_majors(unique_repo: str) -> tuple[str, PublishedArtifact, PublishedArtifact]:
    """`x` released at 2.6.0 then 3.1.0, so `2` names 2.6.0 and `latest` 3.1.0."""
    x = f"{unique_repo}/x"
    v260 = publish_release(x, "2.6.0", _files("x"))
    v310 = publish_release(x, "3.1.0", _files("x"))
    return x, v260, v310


def _held_bare(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, str, PublishedArtifact, PublishedArtifact]:
    """A bare direct `x` locked at 3.1.0 through `latest`."""
    x, v260, v310 = _two_majors(unique_repo)
    write_config(project_dir, skills={"x": _ref(x)})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir)["x"] == f"{_ref(x, 'latest')}@{v310.digest}"
    return runner, x, v260, v310


def _downgrade_line(held: str) -> str:
    return (
        f"would move from 3.1.0 to 2.6.0, a lower major version, because skill/a "
        f"require 2; declare skill x as `{held}` in grimoire.toml"
    )


def test_two_claims_in_one_major_settle_on_the_cascade_tag(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    b = f"{unique_repo}/b"
    publish_release(b, "2.1.4", _files("b"))
    v230 = publish_release(b, "2.3.0", _files("b"))
    a = _claim(unique_repo, "a", "b", "2")
    c = _claim(unique_repo, "c", "b", "2.1")
    write_config(project_dir, skills={"a": a.fq, "c": c.fq})

    rows, stderr = _json_rows(grim_at(project_dir), "lock")

    assert _pins(project_dir)["b"] == f"{_ref(b, '2')}@{v230.digest}"
    assert rows["b"]["disposition"] == "added", rows["b"]
    assert "conflicting versions" not in stderr, stderr


def test_a_release_inside_the_claim_moves_nothing_until_update(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    b = f"{unique_repo}/b"
    publish_release(b, "2.1.4", _files("b"))
    v230 = publish_release(b, "2.3.0", _files("b"))
    a = _claim(unique_repo, "a", "b", "2")
    c = _claim(unique_repo, "c", "b", "2.1")
    write_config(project_dir, skills={"a": a.fq, "c": c.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    v240 = publish_release(b, "2.4.0", _files("b"))

    rows, _ = _json_rows(runner, "lock")

    assert rows["b"]["disposition"] == "carried", rows["b"]
    assert _pins(project_dir)["b"] == f"{_ref(b, '2')}@{v230.digest}"

    updated, _ = _json_rows(runner, "update")

    assert updated["b"]["action"] == "updated", updated["b"]
    assert updated["b"]["new"] == v240.digest, updated["b"]
    assert _pins(project_dir)["b"] == f"{_ref(b, '2')}@{v240.digest}"


@pytest.mark.parametrize("flags", [(), ("--allow-downgrade",)], ids=["plain", "flag"])
def test_claims_in_two_majors_exit_78_even_with_allow_downgrade(
    grim_at, project_dir: Path, unique_repo: str, flags: tuple[str, ...]
) -> None:
    b = f"{unique_repo}/b"
    publish_release(b, "1.0.0", _files("b"))
    publish_release(b, "2.0.0", _files("b"))
    a = _claim(unique_repo, "a", "b", "1")
    c = _claim(unique_repo, "c", "b", "2")
    write_config(project_dir, skills={"a": a.fq, "c": c.fq})

    result = grim_at(project_dir).run("--format", "json", "lock", *flags, check=False)

    assert result.returncode == 78, result.stderr
    error = json.loads(result.stdout)["error"]
    assert error["code"] == "config", error
    assert "reason" not in error, error
    assert f"{_ref(b, '1')} required by skill/a" in error["message"], error
    assert f"{_ref(b, '2')} required by skill/c" in error["message"], error
    assert "declare skill b directly in grimoire.toml with a tag" in error["message"]
    assert not (project_dir / "grimoire.lock").exists()


@pytest.mark.parametrize("command", ["add", "update"])
def test_claims_in_two_majors_exit_78_on_add_and_update_even_with_allow_downgrade(
    grim_at, project_dir: Path, unique_repo: str, command: str
) -> None:
    """`c` moves from claiming `b:1` to `b:2` beside `a`'s `b:1`; `add` and
    `update` each meet it on their own path, and the flag settles neither."""
    b = f"{unique_repo}/b"
    publish_release(b, "1.0.0", _files("b"))
    publish_release(b, "2.0.0", _files("b"))
    a = _claim(unique_repo, "a", "b", "1")
    old_c = _claim(unique_repo, "c", "b", "1")
    declared = {"a": a.fq} if command == "add" else {"a": a.fq, "c": old_c.fq}
    write_config(project_dir, skills=declared)
    runner = grim_at(project_dir)
    runner.run("lock")
    retag(old_c.repo, "keep", old_c.digest)
    c = _claim(unique_repo, "c", "b", "2")
    files = (project_dir / "grimoire.toml", project_dir / "grimoire.lock")
    before = [_text(path) for path in files]
    args = ("add", c.fq) if command == "add" else ("update",)

    result = runner.run(*args, "--allow-downgrade", check=False)

    assert result.returncode == 78, result.stderr
    assert "conflicting versions" in result.stderr, result.stderr
    assert [_text(path) for path in files] == before


def test_two_major_tags_on_one_manifest_still_exit_78(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """One grim build holds one version, even when both tags name the same bytes."""
    b = f"{unique_repo}/b"
    release = publish_release(b, "2.0.0", _files("b"))
    retag(b, "3", release.digest)
    a = _claim(unique_repo, "a", "b", "2")
    c = _claim(unique_repo, "c", "b", "3")
    write_config(project_dir, skills={"a": a.fq, "c": c.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 78, result.stderr
    assert "conflicting versions" in result.stderr, result.stderr
    assert not (project_dir / "grimoire.lock").exists()


def test_a_new_lower_bound_moves_a_held_pin_and_says_why(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    b = f"{unique_repo}/b"
    publish_release(b, "2.1.4", _files("b"))
    a = _claim(unique_repo, "a", "b", "2")
    write_config(project_dir, skills={"a": a.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    v230 = publish_release(b, "2.3.0", _files("b"))
    c = _claim(unique_repo, "c", "b", "2.3")
    write_config(project_dir, skills={"a": a.fq, "c": c.fq})

    rows, stderr = _json_rows(runner, "lock")

    assert rows["b"]["disposition"] == "moved", rows["b"]
    assert _pins(project_dir)["b"] == f"{_ref(b, '2')}@{v230.digest}"
    moved = _lines(stderr, "skill/b moved from 2.1.4 to 2.3.0 because")
    assert len(moved) == 1, stderr


def test_lock_refuses_a_major_downgrade_until_allowed(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, x, v260, v310 = _held_bare(grim_at, project_dir, unique_repo)
    a = _claim(unique_repo, "a", "x", "2")
    write_config(project_dir, skills={"x": _ref(x), "a": a.fq})
    lock = project_dir / "grimoire.lock"
    before = _text(lock)

    refused = runner.run("lock", check=False)

    assert refused.returncode == 78, refused.stderr
    assert (
        f"{_downgrade_line(f'{_ref(x)}@{v310.digest}')}, "
        "or rerun `grim lock --allow-downgrade` to take 2.6.0"
    ) in refused.stderr, refused.stderr
    assert _text(lock) == before

    rows, _ = _json_rows(runner, "lock", "--allow-downgrade")

    assert rows["x"]["disposition"] == "moved", rows["x"]
    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"


def test_a_downgraded_pin_stays_after_its_claim_leaves_until_update(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, x, v260, v310 = _held_bare(grim_at, project_dir, unique_repo)
    a = _claim(unique_repo, "a", "x", "2")
    write_config(project_dir, skills={"x": _ref(x), "a": a.fq})
    runner.run("lock", "--allow-downgrade")
    write_config(project_dir, skills={"x": _ref(x)})

    runner.run("lock")

    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"

    runner.run("update")

    assert _pins(project_dir)["x"] == f"{_ref(x, 'latest')}@{v310.digest}"


def test_add_refuses_a_major_downgrade_naming_rerun_add(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, x, v260, v310 = _held_bare(grim_at, project_dir, unique_repo)
    a = _claim(unique_repo, "a", "x", "2")
    config = project_dir / "grimoire.toml"
    lock = project_dir / "grimoire.lock"
    before = (_text(config), _text(lock))

    refused = runner.run("add", a.fq, check=False)

    assert refused.returncode == 78, refused.stderr
    assert (
        f"{_downgrade_line(f'{_ref(x)}@{v310.digest}')}, then rerun `grim add {a.fq}`, "
        f"or rerun `grim add {a.fq} --allow-downgrade`"
    ) in refused.stderr, refused.stderr
    assert (_text(config), _text(lock)) == before

    runner.run("add", a.fq, "--allow-downgrade")

    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"


def _a_claims_a_lower_major_of_x(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[GrimRunner, str, PublishedArtifact, PublishedArtifact, PublishedArtifact]:
    """Bare `x` locked at 3.1.0 beside `a` at tag `1`, which then moves to a claim of `x:2`."""
    x, v260, v310 = _two_majors(unique_repo)
    plain = make_artifact(f"{unique_repo}/a", "skill", _files("a"), tag="1")
    write_config(project_dir, skills={"x": _ref(x), "a": plain.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    retag(plain.repo, "keep", plain.digest)
    _claim(unique_repo, "a", "x", "2")
    return runner, x, plain, v260, v310


def test_update_refuses_a_major_downgrade_naming_its_own_rerun(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, x, _, v260, v310 = _a_claims_a_lower_major_of_x(
        grim_at, project_dir, unique_repo
    )
    lock = project_dir / "grimoire.lock"
    before = _text(lock)
    held = f"{_ref(x)}@{v310.digest}"

    named = runner.run("update", "a", check=False)

    assert named.returncode == 78, named.stderr
    assert (
        f"{_downgrade_line(held)}, or run `grim update --allow-downgrade -- a x`"
    ) in named.stderr, named.stderr

    bare = runner.run("update", check=False)

    assert bare.returncode == 78, bare.stderr
    assert (
        f"{_downgrade_line(held)}, or rerun `grim update --allow-downgrade` to take 2.6.0"
    ) in bare.stderr, bare.stderr
    assert _text(lock) == before

    runner.run("update", "--allow-downgrade")

    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"


def test_a_named_update_takes_the_downgrade_its_fix_names(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    runner, x, _, v260, _ = _a_claims_a_lower_major_of_x(
        grim_at, project_dir, unique_repo
    )
    refused = runner.run("update", "a", check=False)
    assert refused.returncode == 78, refused.stderr
    unflagged = runner.run("update", "--", "a", "x", check=False)
    assert unflagged.returncode == 78, "naming x alone meets the same guard"

    runner.run("update", "--allow-downgrade", "--", "a", "x")

    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"


@pytest.mark.parametrize("command", ["lock", "add"])
def test_declaring_the_held_digest_keeps_it_past_a_major_downgrade(
    grim_at, project_dir: Path, unique_repo: str, command: str
) -> None:
    runner, x, _, v310 = _held_bare(grim_at, project_dir, unique_repo)
    a = _claim(unique_repo, "a", "x", "2")
    if command == "lock":
        write_config(project_dir, skills={"x": _ref(x), "a": a.fq})
    args = ("lock",) if command == "lock" else ("add", a.fq)
    held = f"{_ref(x)}@{v310.digest}"
    refused = runner.run(*args, check=False)
    assert refused.returncode == 78, refused.stderr
    assert _downgrade_line(held) in refused.stderr, refused.stderr

    kept = {"x": held, "a": a.fq} if command == "lock" else {"x": held}
    write_config(project_dir, skills=kept)
    result = runner.run(*args, check=False)

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["x"].endswith(f"@{v310.digest}"), pins
    assert "a" in pins, pins


def test_a_named_update_keeps_a_declared_digest_after_the_relock_it_asks_for(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A named update refuses a lock its config no longer matches (65) and
    names `grim lock` first; the update after it keeps 3.1.0."""
    runner, x, a, _, v310 = _a_claims_a_lower_major_of_x(
        grim_at, project_dir, unique_repo
    )
    held = f"{_ref(x)}@{v310.digest}"
    refused = runner.run("update", "a", check=False)
    assert refused.returncode == 78, refused.stderr
    assert _downgrade_line(held) in refused.stderr, refused.stderr

    write_config(project_dir, skills={"x": held, "a": a.fq})
    stale = runner.run("update", "a", check=False)

    assert stale.returncode == 65, stale.stderr
    assert "run `grim lock`, then this update" in stale.stderr, stale.stderr

    runner.run("lock")
    result = runner.run("update", "a", check=False)

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["x"].endswith(f"@{v310.digest}"), pins
    assert not pins["a"].endswith(f"@{a.digest}"), "a moved to its claiming release"


def test_a_bare_direct_line_yields_to_a_dependency_major_claim(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x, v260, _ = _two_majors(unique_repo)
    a = _claim(unique_repo, "a", "x", "2")
    write_config(project_dir, skills={"x": _ref(x), "a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"
    yielded = _lines(
        result.stderr, "skill/x is declared without a tag, so it follows what skill/a"
    )
    assert len(yielded) == 1, result.stderr
    assert "declare it with a tag to choose one" in yielded[0]


def test_a_tagged_direct_line_wins_over_a_dependency_claim(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x, _, v310 = _two_majors(unique_repo)
    write_config(project_dir, skills={"x": _ref(x, "3.1.0")})
    runner = grim_at(project_dir)
    runner.run("lock")
    a = _claim(unique_repo, "a", "x", "2")
    write_config(project_dir, skills={"x": _ref(x, "3.1.0"), "a": a.fq})

    result = runner.run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, '3.1.0')}@{v310.digest}"
    held_back = _lines(
        result.stderr, f"skill/x is requested by skill/a as {_ref(x, '2')}"
    )
    assert len(held_back) == 1, result.stderr


def test_a_claim_from_another_repository_leaves_the_bare_line_in_charge(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x, _, v310 = _two_majors(unique_repo)
    publish_release(f"{unique_repo}/other/x", "2.0.0", _files("x"))
    a = _dependent(
        unique_repo, "a", [("skill", "x", _ref(f"{unique_repo}/other/x", "2"))]
    )
    write_config(project_dir, skills={"x": _ref(x), "a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, 'latest')}@{v310.digest}"
    assert "skill/x is declared directly from" in result.stderr, result.stderr
    assert (
        "the direct declaration wins (did you mean to switch publishers?)"
        in result.stderr
    ), result.stderr


def test_dropping_the_tag_from_a_direct_line_keeps_its_pin_until_update(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x, v260, v310 = _two_majors(unique_repo)
    write_config(project_dir, skills={"x": _ref(x, "2")})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"
    write_config(project_dir, skills={"x": _ref(x)})

    runner.run("lock")

    assert _pins(project_dir)["x"] == f"{_ref(x, '2')}@{v260.digest}"

    runner.run("update")

    assert _pins(project_dir)["x"] == f"{_ref(x, 'latest')}@{v310.digest}"


def test_a_minor_claim_resolves_through_a_minor_tag_without_an_annotation(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    minor = make_artifact(x, "skill", _files("x"), tag="1.2")
    retag(x, "1", minor.digest)
    a = _claim(unique_repo, "a", "x", "1.2")
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, '1.2')}@{minor.digest}"


def test_a_minor_claim_above_every_minor_tag_exits_79(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    minor = make_artifact(x, "skill", _files("x"), tag="1.2")
    retag(x, "1", minor.digest)
    a = _claim(unique_repo, "a", "x", "1.3")
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 79, result.stderr
    assert f"no release of {_ref(x)} satisfies ^1.3" in result.stderr, result.stderr
    assert not (project_dir / "grimoire.lock").exists()


@pytest.mark.parametrize("remedy", ["allow", "include"])
def test_update_marketplace_refuses_a_major_downgrade_until_allowed(
    grim_at, project_dir: Path, unique_repo: str, remedy: str
) -> None:
    """Each half of the refusal's remedy, followed as printed, ends the refusal."""
    x, v260, v310 = _two_majors(unique_repo)
    manifest = project_dir / "marketplace.toml"
    manifest.write_text(
        f"[plugins.p]\ninclude = [{json.dumps(_ref(x))}]\n",
        encoding="utf-8",
        newline="\n",
    )
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", str(manifest))
    lock = project_dir / "marketplace.lock"
    assert _part_pins(lock, "p")["x"] == f"{_ref(x, 'latest')}@{v310.digest}"
    a = _claim(unique_repo, "a", "x", "2")
    manifest.write_text(
        f"[plugins.p]\ninclude = [{json.dumps(_ref(x))}, {json.dumps(a.fq)}]\n",
        encoding="utf-8",
        newline="\n",
    )
    before = _text(lock)

    refused = runner.run(
        "--format", "json", "update", "--marketplace", str(manifest), "p", check=False
    )

    assert refused.returncode == 78, refused.stderr
    assert json.loads(refused.stdout)["error"]["reason"] == "major-downgrade"
    assert (
        "would move from 3.1.0 to 2.6.0, a lower major version, because skill/a "
        f"require 2; include skill x as `{_ref(x)}@{v310.digest}` in plugin 'p' of "
        f"{manifest}, or run `grim update --marketplace {manifest} p --allow-downgrade`"
    ) in refused.stderr, refused.stderr
    assert _text(lock) == before

    if remedy == "include":
        included = re.search(r"include skill x as `([^`]+)`", refused.stderr)
        assert included is not None, refused.stderr
        manifest.write_text(
            f"[plugins.p]\ninclude = [{json.dumps(included.group(1))}, {json.dumps(a.fq)}]\n",
            encoding="utf-8",
            newline="\n",
        )
        kept = runner.run("update", "--marketplace", str(manifest), "p", check=False)
        assert kept.returncode == 0, kept.stderr
        assert _part_pins(lock, "p")["x"].endswith(f"@{v310.digest}"), _text(lock)
        return

    runner.run("update", "--marketplace", str(manifest), "p", "--allow-downgrade")

    assert _part_pins(lock, "p")["x"] == f"{_ref(x, '2')}@{v260.digest}"


def _part_pins(lock: Path, plugin: str) -> dict[str, str]:
    """Skill name to `pinned` for one plugin's part of a `marketplace.lock`."""
    entries = tomllib.loads(_text(lock)).get("skill", [])
    return {e["name"]: e["pinned"] for e in entries if e.get("plugin") == plugin}


def test_a_minor_claim_above_the_newest_release_names_the_newest(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    publish_release(x, "2.4.0", _files("x"))
    a = _claim(unique_repo, "a", "x", "2.9")
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 79, result.stderr
    assert (
        f"no release of {_ref(x)} satisfies ^2.9; the newest is 2.4.0" in result.stderr
    ), result.stderr
    assert not (project_dir / "grimoire.lock").exists()


def test_a_newest_tag_whose_manifest_names_another_version_says_so(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    publish_release(x, "1.0.0", _files("x"))
    publish_lie(x, "1.5.0", "3.1.0", _files("x"))
    a = _claim(unique_repo, "a", "x", "1.5")
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 79, result.stderr
    assert (
        f"no release of {_ref(x)} satisfies ^1.5; the newest is 1.5.0, "
        "whose manifest names version 3.1.0"
    ) in result.stderr, result.stderr
    assert not (project_dir / "grimoire.lock").exists()


@pytest.mark.parametrize(
    ("claim", "prereleases", "tail"),
    [
        ("1", ["1.0.0-rc.1", "1.0.0-rc.2"], "^1"),
        ("2.1.0-rc.1", ["2.0.0-rc.1", "2.0.0-rc.2"], "^2.1.0-rc.1"),
    ],
    ids=["release-claim", "prerelease-claim"],
)
def test_a_repository_of_only_prereleases_names_its_newest_prerelease(
    grim_at,
    project_dir: Path,
    unique_repo: str,
    claim: str,
    prereleases: list[str],
    tail: str,
) -> None:
    x = f"{unique_repo}/x"
    for version in prereleases:
        publish_release(x, version, _files("x"))
    a = _claim(unique_repo, "a", "x", claim)
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 79, result.stderr
    assert (
        f"no release of {_ref(x)} satisfies {tail}; it lists only prereleases, "
        f"the newest is {prereleases[-1]}"
    ) in result.stderr, result.stderr
    assert not (project_dir / "grimoire.lock").exists()


@pytest.mark.parametrize(
    ("claim", "tag", "version"),
    [("2", "2", "2.3.7"), ("2.5", "2.9.0", "2.9.0")],
    ids=["major-follows-the-backport", "minor-reads-the-listing"],
)
def test_a_backported_major_tag_answers_only_the_claims_it_satisfies(
    grim_at,
    project_dir: Path,
    unique_repo: str,
    claim: str,
    tag: str,
    version: str,
) -> None:
    x = f"{unique_repo}/x"
    releases = {v: publish_release(x, v, _files("x")) for v in ("2.3.7", "2.9.0")}
    backport(x, "2.3.7")
    a = _claim(unique_repo, "a", "x", claim)
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, tag)}@{releases[version].digest}"


def test_a_major_claim_resolves_a_repository_with_no_latest_tag(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    major = make_artifact(x, "skill", _files("x"), tag="1")
    a = _claim(unique_repo, "a", "x", "1")
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, '1')}@{major.digest}"


def test_a_tagless_claim_takes_the_highest_listed_release_without_latest(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    publish_release(x, "1.0.0", _files("x"), cascade=False)
    newest = publish_release(x, "1.2.0", _files("x"), cascade=False)
    a = _claim(unique_repo, "a", "x", None)
    write_config(project_dir, skills={"a": a.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x, '1.2.0')}@{newest.digest}"


def _by_digest(
    unique_repo: str, name: str, target: str, digest: str
) -> PublishedArtifact:
    """A dependent `name` whose one edge requires skill `target` by `digest`."""
    ref = f"{_ref(f'{unique_repo}/{target}')}@{digest}"
    return _dependent(unique_repo, name, [("skill", target, ref)])


@pytest.mark.parametrize(
    ("version", "claim"), [("1.0.0", "1"), ("1.5.0", "1.5"), ("1.5.0", "1.5.0")]
)
def test_a_digest_claim_beside_a_range_locks_the_same_bytes_twice(
    grim_at, project_dir: Path, unique_repo: str, version: str, claim: str
) -> None:
    x = f"{unique_repo}/x"
    releases = {v: publish_release(x, v, _files("x")) for v in ("1.0.0", "1.5.0")}
    p0 = _by_digest(unique_repo, "p0", "x", releases[version].digest)
    p1 = _claim(unique_repo, "p1", "x", claim)
    write_config(project_dir, skills={"p0": p0.fq, "p1": p1.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    first = _text(project_dir / "grimoire.lock")

    runner.run("lock")

    assert _text(project_dir / "grimoire.lock") == first
    assert _pins(project_dir)["x"] == f"{_ref(x)}@{releases[version].digest}"


def test_a_range_beside_a_digest_claim_needs_no_tag_of_its_own(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    x = f"{unique_repo}/x"
    publish_release(x, "1.0.0", _files("x"))
    v150 = publish_release(x, "1.5.0", _files("x"))
    p0 = _by_digest(unique_repo, "p0", "x", v150.digest)
    p1 = _claim(unique_repo, "p1", "x", "1.2")
    write_config(project_dir, skills={"p0": p0.fq, "p1": p1.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x)}@{v150.digest}"


@pytest.mark.parametrize("tag", [None, "latest"])
def test_an_any_claim_beside_a_digest_claim_adds_no_constraint(
    grim_at, project_dir: Path, unique_repo: str, tag: str | None
) -> None:
    x = f"{unique_repo}/x"
    v100 = publish_release(x, "1.0.0", _files("x"))
    publish_release(x, "2.0.0", _files("x"))
    p0 = _by_digest(unique_repo, "p0", "x", v100.digest)
    p1 = _claim(unique_repo, "p1", "x", tag)
    write_config(project_dir, skills={"p0": p0.fq, "p1": p1.fq})

    result = grim_at(project_dir).run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x)}@{v100.digest}"
