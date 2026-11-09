# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""What `grim lock` and `grim add` report, and the lock they refuse to touch."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_dependent, write_config
from src.registry import PublishedArtifact, retag, tag_digest


def _files(name: str, body: str = "") -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"
    }


def _skill(
    unique_repo: str, name: str, tag: str = "1", body: str = "v1"
) -> PublishedArtifact:
    return make_artifact(f"{unique_repo}/{name}", "skill", _files(name, body), tag=tag)


def _dependent(
    unique_repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    return make_dependent(f"{unique_repo}/{name}", "1", "skill", _files(name), requires)


def _move_tag(unique_repo: str, name: str, tag: str = "1") -> PublishedArtifact:
    """Publish new bytes under `tag`; `keep` stops zot dropping the old manifest."""
    repo = f"{unique_repo}/{name}"
    old = tag_digest(repo, tag)
    retag(repo, "keep", old)
    moved = make_artifact(repo, "skill", _files(name, "v2"), tag=tag)
    assert moved.digest != old, "a moved tag must change the digest"
    return moved


def test_lock_report_dispositions(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-018: each row says what happened to its pin, and a dropped entry gets a row.

    `kept` is also S-001's `carried`: `unchanged` on a tag that moved.
    """
    kept = _skill(unique_repo, "kept")
    edited = _skill(unique_repo, "edited")
    target = _skill(unique_repo, "edited", tag="2", body="v2")
    gone = _skill(unique_repo, "gone")
    fresh = _skill(unique_repo, "fresh")
    write_config(
        project_dir, skills={"kept": kept.fq, "edited": edited.fq, "gone": gone.fq}
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")
    write_config(
        project_dir, skills={"kept": kept.fq, "edited": target.fq, "fresh": fresh.fq}
    )

    rows = {r["name"]: r for r in runner.json("lock")["items"]}

    assert {name: row.get("disposition") for name, row in rows.items()} == {
        "kept": "carried",
        "edited": "moved",
        "fresh": "added",
        "gone": "dropped",
    }
    assert rows["kept"]["action"] == "unchanged"
    assert rows["gone"]["action"] == "dropped"
    assert rows["gone"]["pinned"] == gone.pinned


def test_add_reports_a_moved_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-019: a pin the added edge moved is listed, and the target is not."""
    _skill(unique_repo, "target")
    first = _dependent(unique_repo, "first", [("skill", "target", "./target:1")])
    write_config(project_dir, skills={"first": first.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _move_tag(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.3", moved.digest)
    second = _dependent(unique_repo, "second", [("skill", "target", "./target:1.3")])

    report = runner.json("add", "--kind", "skill", "--name", "second", second.fq)

    listed = report["moved"]
    assert [(m["kind"], m["name"]) for m in listed] == [("skill", "target")], report
    assert listed[0]["pinned"] == moved.pinned


@pytest.mark.parametrize("plain", [True, False], ids=["plain", "json"])
def test_add_names_a_moved_pin_on_stderr_when_plain(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    plain: bool,
) -> None:
    """RV-18: plain output names `moved` in one stderr line, JSON keeps it to the field.

    Only the binary shows which renderer writes to the real stderr: the report's
    unit tests stop at the line's text.
    """
    _skill(unique_repo, "target")
    first = _dependent(unique_repo, "first", [("skill", "target", "./target:1")])
    write_config(project_dir, skills={"first": first.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _move_tag(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.3", moved.digest)
    second = _dependent(unique_repo, "second", [("skill", "target", "./target:1.3")])
    args = ("add", "--no-install", "--kind", "skill", "--name", "second", second.fq)

    result = runner.run(*args) if plain else runner.run("--format", "json", *args)

    line = "moved 1 pin: skill/target"
    assert (line in result.stderr) is plain, result.stderr
    assert line not in result.stdout, result.stdout


def test_add_lists_its_own_target_in_moved_when_it_moved(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-019: re-pinning the target moves a pin the previous lock held, so `moved` lists it."""
    first = _skill(unique_repo, "target")
    write_config(project_dir, skills={"target": first.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    second = _skill(unique_repo, "target", tag="2", body="v2")

    report = runner.json("add", "--kind", "skill", "--name", "target", second.fq)

    assert report["pinned"] == second.pinned
    assert [(m["kind"], m["name"], m["pinned"]) for m in report["moved"]] == [
        ("skill", "target", second.pinned)
    ]


@pytest.mark.parametrize("command", ["update", "lock", "add"])
def test_offline_run_warns_about_a_pin_it_moved(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    command: str,
) -> None:
    """W-4: offline, a tag answers from the local cache, which can predate the lock.

    The pin still moves, as the offline docs promise; one warning on stderr
    names it and stdout stays the payload. Only a real run shows that each
    command hands its rows over.
    """
    cached = _skill(unique_repo, "pin", tag="1.0", body="cached")
    write_config(project_dir, skills={"pin": cached.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    newer = _skill(unique_repo, "pin", tag="1", body="newer")
    write_config(project_dir, skills={"pin": newer.fq})
    online = runner.run("lock")
    assert "tag cache" not in online.stderr, "an online move resolved fresh"
    if command == "add":
        argv = ("add", "--offline", "--kind", "skill", "--name", "pin", cached.fq)
    else:
        write_config(project_dir, skills={"pin": cached.fq})
        argv = (command, "--offline")

    result = runner.run(*argv, format="json")

    assert result.stderr.count("local tag cache may be older than the lock") == 1, (
        result.stderr
    )
    assert "skill 'pin'" in result.stderr
    assert json.loads(result.stdout), "stdout stays the payload"
    assert cached.digest in (project_dir / "grimoire.lock").read_text()


def test_offline_named_update_warns_once(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """RV-6: a named update crosses the relock seam once, so it warns once.

    Only a real run covers it: the named path installs a scope of its own after
    the relock, and the unit fixtures hold no installable layer.
    """
    cached = _skill(unique_repo, "pin", tag="1.0", body="cached")
    write_config(project_dir, skills={"pin": cached.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    newer = _skill(unique_repo, "pin", tag="2", body="newer")
    lock = project_dir / "grimoire.lock"
    lock.write_text(lock.read_text().replace(cached.digest, newer.digest))

    result = runner.run("update", "--offline", "pin", format="json")

    assert result.stderr.count("local tag cache may be older than the lock") == 1, (
        result.stderr
    )
    assert "skill 'pin'" in result.stderr
    assert cached.digest in lock.read_text()


def _damage(project_dir: Path, damage: str) -> None:
    lock = project_dir / "grimoire.lock"
    if damage == "conflict-markers":
        lock.write_text(f"<<<<<<< ours\n{lock.read_text()}=======\n>>>>>>> theirs\n")
    elif damage == "too-new":
        lock.write_text(
            lock.read_text().replace("lock_version = 1", "lock_version = 99")
        )
    else:
        notes = project_dir / "notes.md"
        notes.write_text("# Not a lock\n")
        lock.unlink()
        os.symlink(notes.name, lock)


@pytest.mark.parametrize("command", ["lock", "add"])
@pytest.mark.parametrize(
    "damage",
    [
        "conflict-markers",
        "too-new",
        pytest.param(
            "link-to-another-file",
            marks=pytest.mark.skipif(
                sys.platform == "win32", reason="symlinks are a Unix layout"
            ),
        ),
    ],
)
def test_corrupt_lock_refuses_and_keeps_the_file(
    grim_at,
    project_dir: Path,
    registry: str,
    unique_repo: str,
    damage: str,
    command: str,
) -> None:
    """S-020: an unreadable lock refuses, because re-resolving would move every pin.

    The refusal says so, and names the recovery that keeps them. W-9: a too-new
    lock's recovery is an upgrade; git only holds the same file. RV-14: that
    refusal still says deleting the file moves every pin.
    """
    pinned = _skill(unique_repo, "pinned")
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"pinned": pinned.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _damage(project_dir, damage)
    lock = project_dir / "grimoire.lock"
    lock_bytes = lock.read_bytes()
    config_bytes = (project_dir / "grimoire.toml").read_bytes()
    argv = ("lock",) if command == "lock" else ("add", "--name", "other", other.fq)

    result = runner.run(*argv, check=False)

    assert result.returncode == 78, result.stderr
    if damage == "too-new":
        assert "upgrade grim" in result.stderr
        assert "moves every pin" in result.stderr
        assert "restore it from git" not in result.stderr
    else:
        assert "moves every pin" in result.stderr
        assert "restore it from git" in result.stderr
    assert lock.read_bytes() == lock_bytes
    assert (project_dir / "grimoire.toml").read_bytes() == config_bytes
    assert lock.is_symlink() == (damage == "link-to-another-file")
