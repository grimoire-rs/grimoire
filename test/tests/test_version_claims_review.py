# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Version claims settle across passes before anything refuses.

A resolve reads a line node's guess (its `latest`, or the cascade tag of the
first claim) before it knows every claim on the node. What fails below a guess
the line later leaves refuses nothing, two majors on one node (78) win over a
line that held no release (79), and a literal or digest claim found waves
deeper still fixes the node. A dropped version's claim still guards a major
downgrade, a refusal declares a tag that exists, and a tagless pin is judged by
the ranges its recorded claims show. Releases come from `publish_release`, so
tags and version annotations match what `grim release` writes.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, publish_lie, publish_release, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner


def _files(name: str) -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}


def _ref(repo: str, tag: str | None = None) -> str:
    return f"{REGISTRY_HOST}/{repo}" + (f":{tag}" if tag else "")


def _at(unique_repo: str, target: str, tag: str) -> tuple[str, str]:
    """An edge onto skill `target` at `tag`."""
    return target, _ref(f"{unique_repo}/{target}", tag)


def _by_digest(unique_repo: str, target: str, digest: str) -> tuple[str, str]:
    """An edge onto skill `target` by `digest`."""
    return target, f"{_ref(f'{unique_repo}/{target}')}@{digest}"


def _release(
    unique_repo: str, name: str, version: str, *edges: tuple[str, str]
) -> PublishedArtifact:
    """Release skill `name` at `version`, cascade included, requiring each `(target, ref)`."""
    requires = [("skill", target, ref) for target, ref in edges] or None
    return publish_release(f"{unique_repo}/{name}", version, _files(name), requires)


def _lock(
    grim_at, project_dir: Path, unique_repo: str, root: str
) -> subprocess.CompletedProcess[str]:
    """Declare skill `root` at tag `1` alone and run one `grim lock` under JSON."""
    write_config(project_dir, skills={root: _ref(f"{unique_repo}/{root}", "1")})
    return grim_at(project_dir).run("lock", format="json", check=False)


def _pins(project_dir: Path) -> dict[str, str]:
    text = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    return {e["name"]: e["pinned"] for e in tomllib.loads(text).get("skill", [])}


def _untag_pin(project_dir: Path, name: str, tagless: str) -> None:
    """Rewrite skill `name`'s `pinned` to `tagless`; the declaration hash still holds."""
    lock = project_dir / "grimoire.lock"
    text = lock.read_text(encoding="utf-8")
    pinned = _pins(project_dir)[name]
    lock.write_text(
        text.replace(f'"{pinned}"', f'"{tagless}"'), encoding="utf-8", newline="\n"
    )


def _assert_two_majors_refused(
    result: subprocess.CompletedProcess[str], project_dir: Path
) -> None:
    """A conflict among claims: exit 78 with no reason, which a major downgrade carries."""
    assert result.returncode == 78, result.stderr
    error = json.loads(result.stdout)["error"]
    assert (error["code"], error["exit"]) == ("config", 78), error
    assert "reason" not in error, error
    assert not (project_dir / "grimoire.lock").exists()


@pytest.mark.parametrize("absent", ["repository", "tag"])
def test_a_missing_tag_below_a_release_the_claim_passes_over_refuses_nothing(
    grim_at, project_dir: Path, unique_repo: str, absent: str
) -> None:
    """`a` claims `x:1`; `latest` names 2.0.0, which requires a `y:stable` that is
    not there. The release the claim takes requires nothing, so the lock never
    reaches `y`."""
    v100 = _release(unique_repo, "x", "1.0.0")
    _release(unique_repo, "x", "2.0.0", _at(unique_repo, "y", "stable"))
    if absent == "tag":
        _release(unique_repo, "y", "1.0.0")
    _release(unique_repo, "a", "1.0.0", _at(unique_repo, "x", "1"))

    result = _lock(grim_at, project_dir, unique_repo, "a")

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["x"] == f"{_ref(v100.repo, '1')}@{v100.digest}", pins
    assert "y" not in pins, pins


def test_two_majors_exit_78_beside_a_missing_tag_below_the_guess(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`b` claims `x:1` and `c` claims `x:2`, whose release requires a `y:stable`
    that is not there. The two majors refuse; the missing tag does not."""
    _release(unique_repo, "x", "1.0.0")
    _release(unique_repo, "x", "2.0.0", _at(unique_repo, "y", "stable"))
    _release(unique_repo, "b", "1.0.0", _at(unique_repo, "x", "1"))
    _release(unique_repo, "c", "1.0.0", _at(unique_repo, "x", "2"))
    _release(
        unique_repo,
        "a",
        "1.0.0",
        _at(unique_repo, "b", "1"),
        _at(unique_repo, "c", "1"),
    )

    result = _lock(grim_at, project_dir, unique_repo, "a")

    _assert_two_majors_refused(result, project_dir)


def test_two_majors_exit_78_when_the_second_arrives_after_the_line_held_no_release(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`a` claims `x:2.9`, above every release of `x`, and `p:1`, whose release
    claims `x:3`. Two majors on `x` refuse with 78 ahead of the empty line's 79."""
    _release(unique_repo, "x", "2.4.0")
    _release(unique_repo, "p", "1.5.0", _at(unique_repo, "x", "3"))
    _release(unique_repo, "p", "2.0.0")
    _release(
        unique_repo,
        "a",
        "1.0.0",
        _at(unique_repo, "x", "2.9"),
        _at(unique_repo, "p", "1"),
    )

    result = _lock(grim_at, project_dir, unique_repo, "a")

    _assert_two_majors_refused(result, project_dir)


def test_a_line_with_no_tagged_release_settles_at_a_digest_claim_two_waves_deeper(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`r` claims `x:2.9` and `p:1.5`; `p` 1.5.0 reaches `q`, which names `x` by the
    digest of a 2.9.1 no tag points at, while `x`'s tags stop at 2.3.0. The
    digest claim fixes `x` inside the line, so the lock settles there."""
    x = f"{unique_repo}/x"
    _release(unique_repo, "x", "2.3.0")
    # The digest never depends on the repository, and a digest as the reference
    # pushes the manifest into `x` with no tag.
    untagged = publish_lie(f"{unique_repo}/scratch", "fixed", "2.9.1", _files("x"))
    publish_lie(x, untagged.digest, "2.9.1", _files("x"))
    _release(unique_repo, "q", "1.0.0", _by_digest(unique_repo, "x", untagged.digest))
    _release(unique_repo, "p", "1.5.0", _at(unique_repo, "q", "1"))
    _release(unique_repo, "p", "2.0.0")
    _release(
        unique_repo,
        "r",
        "1.0.0",
        _at(unique_repo, "x", "2.9"),
        _at(unique_repo, "p", "1.5"),
    )

    result = _lock(grim_at, project_dir, unique_repo, "r")

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"] == f"{_ref(x)}@{untagged.digest}"


def test_two_nodes_fixed_by_literal_tags_in_one_late_wave_settle(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`p` 2.0.0 names `m1:fix` and `m1` 1.0.0 names `m2:fix`, so both nodes turn
    from their line to a literal tag in the first pass. With `p` back at 1.0.0
    only `m1`'s claim fixes `m2`, and both settle."""
    _release(unique_repo, "m2", "1.0.0")
    fixed = publish_lie(f"{unique_repo}/m2", "fix", "1.0.1", _files("m2"))
    m1 = _release(unique_repo, "m1", "1.0.0", _at(unique_repo, "m2", "fix"))
    publish_lie(f"{unique_repo}/m1", "fix", "1.0.0", _files("m1"))
    _release(unique_repo, "p", "1.0.0")
    _release(unique_repo, "p", "2.0.0", _at(unique_repo, "m1", "fix"))
    _release(
        unique_repo,
        "a",
        "1.0.0",
        _at(unique_repo, "m1", "1"),
        _at(unique_repo, "m2", "1"),
        _at(unique_repo, "p", "1"),
    )

    result = _lock(grim_at, project_dir, unique_repo, "a")

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["m1"] == f"{_ref(m1.repo, '1')}@{m1.digest}", pins
    assert pins["m2"] == f"{_ref(fixed.repo, 'fix')}@{fixed.digest}", pins


def test_a_major_downgrade_only_a_dropped_version_claims_still_needs_the_flag(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`a` requires `n1` and `n2` with no tag, locked at 2.0.0 each. Then `n1`
    3.0.0 claims `n2:1` and `n2` 1.0.0 claims `n1:2`: the resolve drops `n1`
    3.0.0, yet its claim alone takes `n2` down a major, so `grim update` refuses
    until `--allow-downgrade`."""
    n1, n2 = f"{unique_repo}/n1", f"{unique_repo}/n2"
    n1_v2 = _release(unique_repo, "n1", "2.0.0")
    _release(unique_repo, "n2", "2.0.0")
    _release(unique_repo, "a", "1.0.0", ("n1", _ref(n1)), ("n2", _ref(n2)))
    runner = grim_at(project_dir)
    write_config(project_dir, skills={"a": _ref(f"{unique_repo}/a", "1")})
    runner.run("lock")
    _release(unique_repo, "n1", "3.0.0", _at(unique_repo, "n2", "1"))
    # `grim release` never lowers a float, so `n2:latest` stays on 2.0.0.
    n2_v1 = publish_release(
        n2, "1.0.0", _files("n2"), [("skill", "n1", _ref(n1, "2"))], cascade=False
    )
    retag(n2, "1.0", n2_v1.digest)
    retag(n2, "1", n2_v1.digest)
    lock = project_dir / "grimoire.lock"
    before = lock.read_text(encoding="utf-8")

    refused = runner.run("update", format="json", check=False)

    assert refused.returncode == 78, refused.stderr
    error = json.loads(refused.stdout)["error"]
    assert (error["exit"], error.get("reason")) == (78, "major-downgrade"), error
    assert "skill/n1 3.0.0 (dropped by this resolve)" in error["message"], error
    assert lock.read_text(encoding="utf-8") == before

    runner.run("update", "--allow-downgrade")

    pins = _pins(project_dir)
    assert pins["n1"].endswith(f"@{n1_v2.digest}"), pins
    assert pins["n2"].endswith(f"@{n2_v1.digest}"), pins


def test_a_line_emptied_beside_a_prerelease_claim_declares_a_tag_that_exists(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`p` 1.2.0 claims `c:3` and `p` 1.5.0 claims `c:2.0.0-rc.1`; `y` 1.1.0 then
    raises `p` past 1.2.0, so 78 names the surviving prerelease claim. Its fix
    declares `c:2.0.0-rc.1`, which `grim lock` takes, not the cascade tag `c:2`,
    which no release creates for a prerelease."""
    c, p = f"{unique_repo}/c", f"{unique_repo}/p"
    a = _ref(f"{unique_repo}/a", "1")
    _release(unique_repo, "y", "1.0.0")
    _release(unique_repo, "a", "1.0.0", _at(unique_repo, "y", "1"))
    runner = grim_at(project_dir)
    write_config(project_dir, skills={"a": a})
    runner.run("lock")
    _release(unique_repo, "c", "2.0.0-rc.1")
    _release(unique_repo, "c", "3.0.0")
    p120 = _release(unique_repo, "p", "1.2.0", _at(unique_repo, "c", "3"))
    _release(unique_repo, "p", "1.5.0", _at(unique_repo, "c", "2.0.0-rc.1"))
    # `grim release` never lowers a float; point `p:1` back as another tool may.
    retag(p, "1", p120.digest)
    retag(p, "latest", p120.digest)
    _release(unique_repo, "y", "1.1.0", _at(unique_repo, "p", "1.4"))
    _release(
        unique_repo,
        "a",
        "1.1.0",
        _at(unique_repo, "p", "1"),
        _at(unique_repo, "y", "1.1"),
    )

    refused = runner.run("update", "a", format="json", check=False)

    assert refused.returncode == 78, refused.stderr
    error = json.loads(refused.stdout)["error"]
    assert (error["code"], error["exit"]) == ("config", 78), error
    declared = re.search(r"declare skill c as `([^`]+)`", error["message"])
    assert declared is not None, error
    assert declared[1] == _ref(c, "2.0.0-rc.1"), error

    write_config(project_dir, skills={"a": a, "c": declared[1]})
    taken = runner.run("lock", format="json", check=False)

    assert taken.returncode == 0, taken.stderr


def test_a_tagless_pin_two_recorded_range_claims_hold_stays_on_lock(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`x` carries no version annotation and its pin names no tag. `p` claims
    `x:2.1` and `q` claims `x:2.3`; read as ranges the two nest at [2.3, 3.0), so
    the pin holds both and a new 2.5.0 moves nothing on `grim lock`."""
    x = f"{unique_repo}/x"
    held = make_artifact(x, "skill", _files("x"), tag="2.3")
    retag(x, "2", held.digest)
    _release(unique_repo, "p", "1.0.0", _at(unique_repo, "x", "2.1"))
    _release(unique_repo, "q", "1.0.0", _at(unique_repo, "x", "2.3"))
    runner = grim_at(project_dir)
    write_config(
        project_dir,
        skills={"p": _ref(f"{unique_repo}/p", "1"), "q": _ref(f"{unique_repo}/q", "1")},
    )
    runner.run("lock")
    tagless = f"{_ref(x)}@{held.digest}"
    _untag_pin(project_dir, "x", tagless)
    _release(unique_repo, "x", "2.5.0")

    result = runner.run("lock", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"].endswith(f"@{held.digest}"), _pins(project_dir)


def test_a_tagless_pin_below_a_raised_bound_names_the_span_its_claim_shows(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`x` carries no version annotation and its pin names no tag; `p` 1.0.0 claims
    `x:2.1`. `grim update p` takes `p` 1.1.0, whose claim `x:3` moves `x` to
    3.0.0, and the one stderr line names the span the recorded claim shows as
    `from`. An edit of `grimoire.toml` would leave the lock stale, and a stale
    lock resolves a tagless pin afresh instead of holding it."""
    x = f"{unique_repo}/x"
    held = make_artifact(x, "skill", _files("x"), tag="2.1")
    retag(x, "2", held.digest)
    _release(unique_repo, "p", "1.0.0", _at(unique_repo, "x", "2.1"))
    runner = grim_at(project_dir)
    write_config(project_dir, skills={"p": _ref(f"{unique_repo}/p", "1")})
    runner.run("lock")
    _untag_pin(project_dir, "x", f"{_ref(x)}@{held.digest}")
    v300 = _release(unique_repo, "x", "3.0.0")
    _release(unique_repo, "p", "1.1.0", _at(unique_repo, "x", "3"))

    result = runner.run("update", "p", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["x"].endswith(f"@{v300.digest}")
    needle = "skill/x moved from >=2.1.0 <3.0.0 to 3.0.0 because"
    moved = [line for line in result.stderr.splitlines() if needle in line]
    assert len(moved) == 1, result.stderr


def test_offline_on_a_fresh_store_a_pin_held_under_a_wider_float_is_kept(
    grim_at, grim_binary: Path, tmp_path: Path, project_dir: Path, unique_repo: str
) -> None:
    """`a` claims `x:1.2`, and the online lock pins `x` under the float `x:1`.
    Offline with an empty store and tag cache, as a fresh checkout of the lock
    has, the float's span [1, 2) only widens the claim, so `a`'s recorded edge
    keeps the pin and the lock stays byte for byte."""
    x = _release(unique_repo, "x", "1.2.0")
    _release(unique_repo, "a", "1.0.0", _at(unique_repo, "x", "1.2"))
    write_config(project_dir, skills={"a": _ref(f"{unique_repo}/a", "1")})
    grim_at(project_dir).run("lock")
    assert _pins(project_dir)["x"] == f"{_ref(x.repo, '1')}@{x.digest}"
    lock = project_dir / "grimoire.lock"
    before = lock.read_text(encoding="utf-8")
    fresh = GrimRunner(grim_binary, tmp_path / "fresh" / "grim-home", cwd=project_dir)

    result = fresh.run("--offline", "lock", format="json", check=False)

    assert result.returncode == 0, result.stderr
    assert lock.read_text(encoding="utf-8") == before


def test_removing_a_retyped_key_keeps_a_pin_a_remaining_version_claim_may_hold(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`a` asks `c:stable` and `q` asks `c:2`, so `c` is pinned at `c:stable` on
    2.3.0 while 2.9.0 is out. `a` leaves `grimoire.toml` by hand, `c = c:2` is
    typed, and `grim remove skill c` keeps `c`'s row, since its tag does not rule
    out `q`'s claim. The next `grim lock` keeps 2.3.0 instead of taking 2.9.0."""
    c = f"{unique_repo}/c"
    v230 = _release(unique_repo, "c", "2.3.0")
    retag(c, "stable", v230.digest)
    _release(unique_repo, "c", "2.9.0")
    _release(unique_repo, "a", "1.0.0", _at(unique_repo, "c", "stable"))
    _release(unique_repo, "q", "1.0.0", _at(unique_repo, "c", "2"))
    runner = grim_at(project_dir)
    q = _ref(f"{unique_repo}/q", "1")
    write_config(project_dir, skills={"a": _ref(f"{unique_repo}/a", "1"), "q": q})
    runner.run("lock")
    assert _pins(project_dir)["c"] == f"{_ref(c, 'stable')}@{v230.digest}"
    write_config(project_dir, skills={"q": q, "c": _ref(c, "2")})

    runner.run("remove", "skill", "c")

    pins = _pins(project_dir)
    assert pins.get("c", "").endswith(f"@{v230.digest}"), pins

    runner.run("lock")

    pins = _pins(project_dir)
    assert pins["c"].endswith(f"@{v230.digest}"), pins
