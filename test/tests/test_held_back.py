# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A declaration that holds back a dependency's request says so.

`x` is declared at `x:1` and locked at v1; `y` asks for `x:2`. A tagged
declaration wins, and `add`, `update` and `lock` print one line naming the update
that re-reads the declared tag. `grim status --check` lists the same requests from
the lock's recorded edges. Exits, JSON and the lock are untouched. A declaration
with no tag follows `y`'s request instead.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest
import tomllib

from src.helpers import (
    make_artifact,
    make_bundle,
    make_dependent,
    publish_release,
    write_config,
)
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner

_REQUIRES_BLOCK = re.compile(
    r'\n\[\[skill\.requires\]\]\nkind = "[^"]*"\nname = "[^"]*"\nid = "[^"]*"\n'
)
_REMEDY = "run `grim update -- x`"


def _files(name: str, body: str) -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"
    }


def _publish_x(unique_repo: str, body: str, *tags: str) -> PublishedArtifact:
    first, *rest = tags
    x = make_artifact(f"{unique_repo}/x", "skill", _files("x", body), tag=first)
    for tag in rest:
        retag(f"{unique_repo}/x", tag, x.digest)
    return x


def _y(unique_repo: str, wants: str, body: str = "v1") -> PublishedArtifact:
    return make_dependent(
        f"{unique_repo}/y",
        "1",
        "skill",
        _files("y", body),
        [("skill", "x", f"{REGISTRY_HOST}/{unique_repo}/x:{wants}")],
    )


def _declare_x(
    project_dir: Path, unique_repo: str, tag: str = ":1", **more: str
) -> None:
    # A declaration with no tag yields to a dependency's version claim on `x`.
    write_config(
        project_dir, skills={"x": f"{REGISTRY_HOST}/{unique_repo}/x{tag}", **more}
    )


def _pins(project_dir: Path) -> dict[str, str]:
    lock = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    return {row["name"]: row["pinned"] for row in lock.get("skill", [])}


def _held(stderr: str) -> list[str]:
    return [line for line in stderr.splitlines() if "is requested by" in line]


def _add_y(runner: GrimRunner, y: PublishedArtifact):
    return runner.run("add", "--kind", "skill", "--name", "y", y.fq, check=False)


def _scene(grim_at, project_dir: Path, unique_repo: str):
    """`x` locked at v1 with `latest` on v1; `x:2` is a different digest."""
    x1 = _publish_x(unique_repo, "v1", "latest", "1")
    x2 = _publish_x(unique_repo, "v2", "2")
    _declare_x(project_dir, unique_repo)
    runner = grim_at(project_dir)
    runner.run("lock")
    return runner, x1, x2


def test_add_warns_and_keeps_the_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The issue's case: exit 0, `y` beside `x` v1, the lock row of `x` unchanged."""
    runner, x1, x2 = _scene(grim_at, project_dir, unique_repo)
    before = _pins(project_dir)["x"]
    y = _y(unique_repo, "2")

    result = _add_y(runner, y)

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["x"] == before
    assert pins["x"].endswith(x1.digest)
    assert pins["y"].endswith(y.digest)
    assert (project_dir / ".claude/skills/y/SKILL.md").is_file()
    held = _held(result.stderr)
    assert len(held) == 1, result.stderr
    assert f"skill/x is requested by skill/y as {REGISTRY_HOST}/{unique_repo}/x:2" in held[0]
    assert f"holds it at {x1.digest[:19]}" in held[0]
    assert _REMEDY in held[0]
    assert "or declare the requested reference" in held[0]
    assert x2.digest not in pins["x"]


def test_offline_add_names_no_held_back_request(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Error case: an offline run reads nothing, so it warns nothing."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)
    y = _y(unique_repo, "2")
    online = _add_y(runner, y)
    assert len(_held(online.stderr)) == 1, online.stderr
    runner.run("remove", "skill", "y")
    # Everything `y` needs is in the content store now, so an offline add can finish.
    offline = runner.run(
        "--offline", "add", "--kind", "skill", "--name", "y", y.fq, check=False
    )

    assert offline.returncode == online.returncode == 0, offline.stderr
    assert _held(offline.stderr) == [], offline.stderr


def test_lock_warns_for_a_new_parent_and_not_again(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A repeat run over the same declaration is silent."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)
    y = _y(unique_repo, "2")
    _declare_x(project_dir, unique_repo, y=y.fq)

    first = runner.run("lock", check=False)
    again = runner.run("lock", check=False)

    assert first.returncode == again.returncode == 0, first.stderr + again.stderr
    assert len(_held(first.stderr)) == 1, first.stderr
    assert _REMEDY in first.stderr
    assert _held(again.stderr) == [], again.stderr


def test_update_warns_when_the_parent_moves(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, x1, _ = _scene(grim_at, project_dir, unique_repo)
    old_y = _y(unique_repo, "1")
    _declare_x(project_dir, unique_repo, y=old_y.fq)
    quiet = runner.run("lock", check=False)
    assert _held(quiet.stderr) == [], "x:1 names the held digest"
    retag(f"{unique_repo}/y", "keep", old_y.digest)
    _y(unique_repo, "2", body="v2")

    result = runner.run("update", "--", "y", check=False)

    assert result.returncode == 0, result.stderr
    assert len(_held(result.stderr)) == 1, result.stderr
    assert "skill/x is requested by skill/y as" in result.stderr
    assert _pins(project_dir)["x"].endswith(x1.digest)
    again = runner.run("update", "--", "y", check=False)
    assert _held(again.stderr) == [], again.stderr


def test_a_request_that_names_the_held_digest_warns_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Two tags that name one digest are no held-back request."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)

    result = _add_y(runner, _y(unique_repo, "1"))

    assert result.returncode == 0, result.stderr
    assert _held(result.stderr) == [], result.stderr


def _status(runner: GrimRunner, unique_repo: str, *flags: str, fmt: bool = False):
    scope = ("--registry", f"{REGISTRY_HOST}/{unique_repo}")
    args = (*scope, "status", *flags)
    return runner.run(*args, format="json" if fmt else None, check=False)


def test_status_check_lists_the_request_until_x_moves(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The note stays until `x` moves, then goes. Declared with no tag, `x` follows
    `y`'s request, and the run says so in place of the note."""
    runner, x1, x2 = _scene(grim_at, project_dir, unique_repo)
    y = _y(unique_repo, "2")
    added = _add_y(runner, y)
    assert added.returncode == 0, added.stderr

    listed = _status(runner, unique_repo, "--check")

    assert listed.returncode == 0, listed.stderr
    held = _held(listed.stderr)
    assert len(held) == 1, listed.stderr
    assert _held(added.stderr)[0].endswith(held[0]), "the run and the note say the same thing"

    _declare_x(project_dir, unique_repo, tag="", y=y.fq)
    # `grim lock`: a named update refuses a lock the edit left stale.
    moved = runner.run("lock", check=False)
    assert moved.returncode == 0, moved.stderr
    assert _pins(project_dir)["x"].endswith(x2.digest)
    yielded = [
        line
        for line in moved.stderr.splitlines()
        if "is declared without a tag" in line
    ]
    assert len(yielded) == 1, moved.stderr
    assert "so it follows what skill/y" in yielded[0]
    assert "declare it with a tag to choose one" in yielded[0]
    assert _held(moved.stderr) == [], moved.stderr
    gone = _status(runner, unique_repo, "--check")
    assert gone.returncode == 0, gone.stderr
    assert _held(gone.stderr) == [], gone.stderr


def _requested(held: str) -> str:
    """The reference the note says the parent requests."""
    match = re.search(r" as (\S+), but your declaration holds it at ", held)
    assert match is not None, held
    return match.group(1)


def test_declaring_the_requested_reference_clears_the_note(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The note's second remedy: declare `x` as the reference it names, then `grim lock`."""
    runner, _, x2 = _scene(grim_at, project_dir, unique_repo)
    y = _y(unique_repo, "2")
    held = _held(_add_y(runner, y).stderr)
    assert len(held) == 1 and "or declare the requested reference" in held[0], held
    write_config(project_dir, skills={"x": _requested(held[0]), "y": y.fq})

    relocked = runner.run("lock", check=False)

    assert relocked.returncode == 0, relocked.stderr
    assert _pins(project_dir)["x"] == f"{REGISTRY_HOST}/{unique_repo}/x:2@{x2.digest}"
    assert _held(relocked.stderr) == [], relocked.stderr
    checked = _status(runner, unique_repo, "--check")
    assert checked.returncode == 0, checked.stderr
    assert _held(checked.stderr) == [], checked.stderr


def test_rereading_an_overlapping_declared_tag_clears_the_note(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The note's first remedy: once the declared `x:1` names 1.2.0, which `y`'s
    `x:1.2` also names, the update the note names takes it and the note goes."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)
    shipped = publish_release(f"{unique_repo}/x", "1.2.0", _files("x", "v12"))
    y = _y(unique_repo, "1.2")
    added = _add_y(runner, y)
    held = _held(added.stderr)
    assert added.returncode == 0, added.stderr
    assert len(held) == 1 and _REMEDY in held[0], added.stderr

    updated = runner.run("update", "--", "x", check=False)

    assert updated.returncode == 0, updated.stderr
    assert _pins(project_dir)["x"] == f"{REGISTRY_HOST}/{unique_repo}/x:1@{shipped.digest}"
    assert _held(updated.stderr) == [], updated.stderr
    checked = _status(runner, unique_repo, "--check")
    assert checked.returncode == 0, checked.stderr
    assert _held(checked.stderr) == [], checked.stderr


def test_status_check_adds_nothing_where_it_must_not(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Plain `status`, offline `--check` and a lock without edges list nothing."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)
    assert _add_y(runner, _y(unique_repo, "2")).returncode == 0

    plain = _status(runner, unique_repo)
    offline = runner.run(
        "--offline", "--registry", f"{REGISTRY_HOST}/{unique_repo}", "status", "--check"
    )
    lock = project_dir / "grimoire.lock"
    lock.write_text(
        _REQUIRES_BLOCK.sub("\n", lock.read_text(encoding="utf-8")),
        encoding="utf-8",
        newline="\n",
    )
    edgeless = _status(runner, unique_repo, "--check")

    assert plain.returncode == offline.returncode == edgeless.returncode == 0
    assert _held(plain.stderr) == []
    assert _held(offline.stderr) == [], offline.stderr
    assert _held(edgeless.stderr) == [], edgeless.stderr


def test_status_check_json_has_the_same_shape_with_a_held_back_request(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Stdout is the JSON report alone, with the keys it has without a held-back request."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)
    y = _y(unique_repo, "1")
    assert _add_y(runner, y).returncode == 0
    control = _status(runner, unique_repo, "--check", fmt=True)
    runner.run("remove", "skill", "y")
    retag(f"{unique_repo}/y", "keep", y.digest)
    held_y = _y(unique_repo, "2", body="v2")
    assert _add_y(runner, held_y).returncode == 0

    held = _status(runner, unique_repo, "--check", fmt=True)

    assert held.returncode == control.returncode == 0, held.stderr
    assert _held(held.stderr) == [], "JSON mode prints no advisory notes"
    report, base = json.loads(held.stdout), json.loads(control.stdout)
    assert set(report) == set(base)
    assert report["checked"] is True
    assert [set(item) for item in report["items"]] == [set(item) for item in base["items"]]
    row = {item["name"]: item for item in report["items"]}["x"]
    assert row["state"] == "installed"
    assert row["pinned"].endswith(_pins(project_dir)["x"].rsplit("@", 1)[1])


def test_a_marketplace_part_run_prints_no_held_back_warning(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The same shape warns in a project and stays silent in a part."""
    runner, _, _ = _scene(grim_at, project_dir, unique_repo)
    y = _y(unique_repo, "2")
    control = _add_y(runner, y)
    assert len(_held(control.stderr)) == 1, control.stderr
    x_ref = f"{REGISTRY_HOST}/{unique_repo}/x:1"
    manifest = project_dir / "marketplace.toml"
    manifest.write_text(
        f"[plugins.team]\ninclude = [{json.dumps(x_ref)}]\n", encoding="utf-8", newline="\n"
    )
    first = runner.run("update", "--marketplace", str(manifest), check=False)
    manifest.write_text(
        f"[plugins.team]\ninclude = [{json.dumps(x_ref)}, {json.dumps(y.fq)}]\n",
        encoding="utf-8",
        newline="\n",
    )

    result = runner.run("update", "--marketplace", str(manifest), check=False)

    assert first.returncode == result.returncode == 0, first.stderr + result.stderr
    assert _held(result.stderr) == [], result.stderr


def test_status_check_lists_a_request_held_back_from_a_declared_bundle(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A declared bundle holds its pin as a skill does; `--check` names the request."""
    m1 = _publish_x(unique_repo, "m1", "1")
    m2 = _publish_x(unique_repo, "m2", "2")
    member = f"{REGISTRY_HOST}/{unique_repo}/x"
    b1 = make_bundle(f"{unique_repo}/b", [("skill", "x", f"{member}:1")], tag="1")
    b2 = make_bundle(f"{unique_repo}/b", [("skill", "x", f"{member}:2")], tag="2")
    assert b1.digest != b2.digest and m1.digest != m2.digest
    write_config(project_dir, bundles={"b": f"{REGISTRY_HOST}/{unique_repo}/b:1"})
    runner = grim_at(project_dir)
    runner.run("lock")
    y = make_dependent(
        f"{unique_repo}/y",
        "1",
        "skill",
        _files("y", "v1"),
        [("bundle", "b", f"{REGISTRY_HOST}/{unique_repo}/b:2")],
    )
    added = _add_y(runner, y)
    assert added.returncode == 0, added.stderr

    listed = _status(runner, unique_repo, "--check")

    assert listed.returncode == 0, listed.stderr
    held = _held(listed.stderr)
    assert len(held) == 1, listed.stderr
    assert f"bundle/b is requested by skill/y as {REGISTRY_HOST}/{unique_repo}/b:2" in held[0]
    assert "run `grim update -- b`" in held[0]
