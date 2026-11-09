# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Only `grim update` moves a pin; `lock`, `add` and `remove` keep what is locked.

Each scenario moves a tag after locking, so a kept pin is told apart from a tag
that never moved, and the old manifest stays tagged: zot drops a manifest once
its last tag moves, and a carried pin is read by digest.
"""

from __future__ import annotations

import json
import re
import shutil
import urllib.error
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, make_dependent, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, retag, tag_digest
from src.runner import GrimRunner

# Strips `:tag` from `pinned` only: a registry port is followed by `/`, never
# by `@sha256:`, so `host:5000/repo@sha256:…` survives untouched.
_ADVISORY_TAG = re.compile(r'(?m)^(pinned = "[^"@]*):[^/"@]+(@sha256:)')
_REQUIRES_BLOCK = re.compile(
    r'\n\[\[skill\.requires\]\]\nkind = "[^"]*"\nname = "[^"]*"\nid = "[^"]*"\n'
)
_PRE_CHANGE_GENERATED_AT = "2020-01-02T03:04:05Z"


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


def _move_tag(
    unique_repo: str, name: str, tag: str = "1", body: str = "v2"
) -> PublishedArtifact:
    """Publish new bytes under `tag`, keeping the manifest it left reachable."""
    repo = f"{unique_repo}/{name}"
    old = tag_digest(repo, tag)
    retag(repo, "keep", old)
    moved = make_artifact(repo, "skill", _files(name, body), tag=tag)
    assert moved.digest != old, "a moved tag must change the digest"
    return moved


def _add(runner, ref: PublishedArtifact, name: str, kind: str = "skill"):
    return runner.run("add", "--kind", kind, "--name", name, ref.fq, check=False)


def _lock(project_dir: Path) -> dict:
    return tomllib.loads((project_dir / "grimoire.lock").read_text())


def _pins(project_dir: Path, kind: str = "skill") -> dict[str, str]:
    return {e["name"]: e["pinned"] for e in _lock(project_dir).get(kind, [])}


def _installed(project_dir: Path, name: str) -> str:
    return (project_dir / f".claude/skills/{name}/SKILL.md").read_text()


def _rewrite_as_pre_change(project_dir: Path, *, edges: bool = True) -> None:
    """Rewrite the lock as a build before recorded references wrote it.

    Built by stripping, so it stays pre-change once `pinned` carries tags.
    `edges=False` also drops `requires`, which 0.15.1 never wrote.
    """
    path = project_dir / "grimoire.lock"
    text = _ADVISORY_TAG.sub(r"\1\2", path.read_text())
    if not edges:
        text = _REQUIRES_BLOCK.sub("\n", text)
    text = re.sub(
        r'(?m)^generated_at = "[^"]*"$',
        f'generated_at = "{_PRE_CHANGE_GENERATED_AT}"',
        text,
    )
    path.write_text(text)
    assert not re.search(r'(?m)^pinned = "[^"]*:[^/"@]+@', text), text
    assert _lock(project_dir)["metadata"]["lock_version"] == 1


def _drop_bundle_rows(project_dir: Path) -> None:
    """Cut every `[[bundle]]` table, as a lock written before member lists."""
    path = project_dir / "grimoire.lock"
    kept, cutting = [], False
    for line in path.read_text().splitlines(keepends=True):
        if line.startswith("["):
            cutting = line.startswith(("[[bundle]]", "[[bundle."))
        if not cutting:
            kept.append(line)
    path.write_text("".join(kept))
    assert "bundle" not in _lock(project_dir)


def _rewrite_as_grim_0_3(project_dir: Path) -> None:
    """Rewrite the lock as grim 0.3 wrote it.

    Tagless pins, no edges and no member lists, and one `bundle` stamp per row:
    grim 0.3 kept only the first bundle that provided a member.
    """
    _rewrite_as_pre_change(project_dir, edges=False)
    _drop_bundle_rows(project_dir)
    path = project_dir / "grimoire.lock"
    lines, out, stamped, i = path.read_text().splitlines(), [], False, 0
    while i < len(lines):
        if lines[i] == "[[skill.bundles]]":
            repo, tag = lines[i + 1].split('"')[1], lines[i + 2].split('"')[1]
            if not stamped:
                while out and not out[-1]:
                    out.pop()
                out += [f'bundle = "{repo}"', f'bundle_tag = "{tag}"', ""]
                stamped = True
            i += 3
            continue
        if lines[i].startswith("["):
            stamped = False
        out.append(lines[i])
        i += 1
    path.write_text("\n".join(out) + "\n")
    assert all("bundles" not in row for row in _lock(project_dir)["skill"])


def _evict_from_store(grim_home: Path, digest: str) -> None:
    algorithm, hex_digest = digest.split(":")
    (grim_home / "blobs" / algorithm / hex_digest[:2] / hex_digest).unlink()


def test_install_state_stays_tagless(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C-009: the lock records the tag, the state `grim install` writes does not."""
    skill = _skill(unique_repo, "s")
    write_config(project_dir, skills={"s": skill.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir) == {
        "s": f"{REGISTRY_HOST}/{unique_repo}/s:1@{skill.digest}"
    }

    runner.run("install", "--client", "claude")

    state = json.loads((project_dir / ".grimoire" / "state.json").read_text())
    assert [record["pinned"] for record in state["records"]] == [skill.pinned]


def test_lock_keeps_pins_after_a_tag_moves(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-001: a moved tag stays out of the lock and `install` writes the old bytes.

    The row's `carried` is asserted by `test_lock_report_dispositions`, on the
    same moved tag: the `disposition` field is not part of the lock semantics
    this test guards.
    """
    kept = _skill(unique_repo, "kept")
    write_config(project_dir, skills={"kept": kept.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")

    rows = {r["name"]: r for r in runner.json("lock")["items"]}

    assert _pins(project_dir)["kept"].endswith(kept.digest)
    assert rows["kept"]["action"] == "unchanged", rows
    runner.run("install")
    assert "kept v1" in _installed(project_dir, "kept")


def test_add_keeps_other_pins(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-002: `grim add` resolves its target and leaves every other pin."""
    kept = _skill(unique_repo, "kept")
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"kept": kept.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")

    result = _add(runner, other, "other")

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["other"].endswith(other.digest)
    assert pins["kept"].endswith(kept.digest)


def test_add_of_a_missing_tag_exits_79(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-002 error case: a tag the registry lacks exits 79 and writes nothing."""
    kept = _skill(unique_repo, "kept")
    write_config(project_dir, skills={"kept": kept.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    config = (project_dir / "grimoire.toml").read_bytes()
    lock = (project_dir / "grimoire.lock").read_bytes()

    result = runner.run(
        "add",
        "--kind",
        "skill",
        "--name",
        "absent",
        f"{REGISTRY_HOST}/{unique_repo}/absent:1",
        check=False,
    )

    assert result.returncode == 79, result.stderr
    assert "tag not found" in result.stderr, result.stderr
    assert (project_dir / "grimoire.toml").read_bytes() == config
    assert (project_dir / "grimoire.lock").read_bytes() == lock


def test_hand_edit_then_lock_keeps_other_pins(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-003: a hand-added entry resolves and nothing else does."""
    kept = _skill(unique_repo, "kept")
    fresh = _skill(unique_repo, "fresh")
    write_config(project_dir, skills={"kept": kept.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")
    write_config(project_dir, skills={"kept": kept.fq, "fresh": fresh.fq})

    runner.run("lock")

    pins = _pins(project_dir)
    assert pins["fresh"].endswith(fresh.digest)
    assert pins["kept"].endswith(kept.digest)


def test_remove_then_lock_keeps_pins(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-004: re-locking after `grim remove` keeps the remaining pins, a diamond's too."""
    kept = _skill(unique_repo, "kept")
    gone = _skill(unique_repo, "gone")
    target = _skill(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.2", target.digest)
    left = _dependent(unique_repo, "left", [("skill", "target", "./target:1")])
    right = _dependent(unique_repo, "right", [("skill", "target", "./target:1.2")])
    write_config(
        project_dir,
        skills={
            "kept": kept.fq,
            "gone": gone.fq,
            "left": left.fq,
            "right": right.fq,
        },
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")
    moved = _move_tag(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.2", moved.digest)

    runner.run("remove", "skill", "gone")
    runner.run("lock")

    pins = _pins(project_dir)
    assert set(pins) == {"kept", "left", "right", "target"}
    assert pins["kept"].endswith(kept.digest)
    assert pins["target"] == f"{REGISTRY_HOST}/{unique_repo}/target:1@{target.digest}"


@pytest.mark.parametrize("holders", [2, 3])
def test_remove_of_the_diamond_winner_keeps_the_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str, holders: int
) -> None:
    """RV-1 (W-11, S-004): the holder whose tag won leaves, and the pin stays.

    Each surviving tag moves to its own digest, so a relock that resolved them
    would move the pin, or with three holders exit 78. The acceptance test
    earns its place by reading the rewritten pin back through a real lock file.
    """
    target = _skill(unique_repo, "target")
    survivors = ["1.2", "1.3"][: holders - 1]
    skills = {
        "winner": _dependent(
            unique_repo, "winner", [("skill", "target", "./target:1")]
        ).fq
    }
    for tag in survivors:
        retag(f"{unique_repo}/target", tag, target.digest)
        name = f"holder-{tag.replace('.', '-')}"
        skills[name] = _dependent(
            unique_repo, name, [("skill", "target", f"./target:{tag}")]
        ).fq
    write_config(project_dir, skills=skills)
    runner = grim_at(project_dir)
    runner.run("lock")
    for tag in survivors:
        _move_tag(unique_repo, "target", tag=tag, body=f"moved to {tag}")

    removed = runner.run("remove", "skill", "winner")
    locked = runner.run("lock")

    kept = f"{REGISTRY_HOST}/{unique_repo}/target:1@{target.digest}"
    assert _pins(project_dir)["target"] == kept
    assert removed.stderr.strip() == "", removed.stderr
    assert locked.stderr.strip() == "", locked.stderr


def _direct_over_two_tags(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[object, PublishedArtifact]:
    """`target:2` declared over `pone` asking for `:1` and `qtwo` for `:2`, one digest."""
    target = _skill(unique_repo, "target", tag="2")
    retag(f"{unique_repo}/target", "1", target.digest)
    pone = _dependent(unique_repo, "pone", [("skill", "target", "./target:1")])
    qtwo = _dependent(unique_repo, "qtwo", [("skill", "target", "./target:2")])
    write_config(
        project_dir,
        skills={"target": target.fq, "pone": pone.fq, "qtwo": qtwo.fq},
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    return runner, target


def test_remove_of_a_direct_key_keeps_a_pin_both_edges_agree_on(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The remove deleted the row, so `grim lock` moved the digest.

    The edge at `:1` was never resolved, so the remove leaves the lock stale. It
    changes no row, so the hash stays the one those rows were written for. The
    pin names the edge at `:2`, so it answers every recorded edge, and it stays
    where it was though both tags moved.
    """
    runner, target = _direct_over_two_tags(grim_at, project_dir, unique_repo)
    moved = _move_tag(unique_repo, "target", tag="2", body="v2")
    retag(f"{unique_repo}/target", "1", moved.digest)
    proof = _lock(project_dir)["metadata"]["declaration_hash"]

    removed = runner.run("remove", "skill", "target")
    assert _lock(project_dir)["metadata"]["declaration_hash"] == proof
    assert "run `grim lock`" in removed.stderr
    runner.run("lock")

    pin = f"{REGISTRY_HOST}/{unique_repo}/target:2@{target.digest}"
    assert _pins(project_dir)["target"] == pin
    assert _lock(project_dir)["metadata"]["declaration_hash"] != ""


def _direct_over_a_conflict(
    grim_at, project_dir: Path, unique_repo: str
) -> tuple[object, PublishedArtifact, PublishedArtifact]:
    """`x:1` declared over `p` asking for `:1` and `q` for `:2`, two digests."""
    one = _skill(unique_repo, "x", tag="1", body="ONE")
    two = make_artifact(f"{unique_repo}/x", "skill", _files("x", "TWO"), tag="2")
    assert one.digest != two.digest
    p = _dependent(unique_repo, "p", [("skill", "x", "./x:1")])
    q = _dependent(unique_repo, "q", [("skill", "x", "./x:2")])
    write_config(project_dir, skills={"x": one.fq, "p": p.fq, "q": q.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    return runner, one, two


def test_two_removes_write_no_false_pair(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Two plain removes wrote `x:2` beside the digest of `x:1`."""
    runner, one, two = _direct_over_a_conflict(grim_at, project_dir, unique_repo)

    runner.run("remove", "skill", "x")
    runner.run("remove", "skill", "p")

    assert _pins(project_dir)["x"] == f"{REGISTRY_HOST}/{unique_repo}/x:1@{one.digest}"
    runner.run("lock")
    assert _pins(project_dir)["x"] == f"{REGISTRY_HOST}/{unique_repo}/x:2@{two.digest}"


def test_remove_of_a_direct_key_over_a_conflict_keeps_the_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The remove hides a real conflict, and `grim lock` carries where a fresh resolve exits 78.

    The pin names the edge `p` recorded, so on the stale hash it answers every
    recorded edge, offline as online. The owner accepted this exit-0 case.
    """
    runner, one, _ = _direct_over_a_conflict(grim_at, project_dir, unique_repo)
    removed = runner.run("remove", "skill", "x")
    kept = f"{REGISTRY_HOST}/{unique_repo}/x:1@{one.digest}"
    assert _pins(project_dir)["x"] == kept

    offline = runner.run("--offline", "lock", check=False)
    assert offline.returncode == 0, offline.stderr
    assert _pins(project_dir)["x"] == kept
    locked = runner.run("lock", check=False)

    assert locked.returncode == 0, locked.stderr
    assert _pins(project_dir)["x"] == kept
    # The remove's note names the check `grim lock` does not make.
    assert "then `grim update -- x`" in removed.stderr, removed.stderr
    checked = runner.run("update", "--", "x", check=False)
    assert checked.returncode == 78, checked.stderr
    assert _pins(project_dir)["x"] == kept


def test_a_global_remove_note_names_the_global_commands(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A global remove run inside a project names `grim --global` in its advice.

    Found by review: the note named plain `grim lock` and `grim update -- x`,
    which act on the project in the working directory, not the global lock.
    """
    one = _skill(unique_repo, "x", tag="1", body="ONE")
    make_artifact(f"{unique_repo}/x", "skill", _files("x", "TWO"), tag="2")
    p = _dependent(unique_repo, "p", [("skill", "x", "./x:1")])
    q = _dependent(unique_repo, "q", [("skill", "x", "./x:2")])
    write_config(grim_home, skills={"x": one.fq, "p": p.fq, "q": q.fq})
    write_config(project_dir, skills={})
    runner = grim_at(project_dir)
    runner.run("--global", "lock")

    removed = runner.run("--global", "remove", "skill", "x")

    assert "run `grim --global lock`, which keeps the pin" in removed.stderr, removed.stderr
    assert "then `grim --global update -- x`" in removed.stderr, removed.stderr
    assert "`grim lock`" not in removed.stderr, removed.stderr
    assert "`grim update" not in removed.stderr, removed.stderr


def test_hand_deleting_a_direct_line_over_a_conflict_carries(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The one exception a fresh resolve disagrees with, decided in the design record.

    A stale hash cannot say why it is stale, and an honest diamond must survive a
    hand edit elsewhere, so `grim lock` carries where a fresh resolve exits 78.
    """
    runner, one, _ = _direct_over_a_conflict(grim_at, project_dir, unique_repo)
    config = tomllib.loads((project_dir / "grimoire.toml").read_text())
    del config["skills"]["x"]
    write_config(project_dir, skills=config["skills"])

    runner.run("lock")

    assert _pins(project_dir)["x"] == f"{REGISTRY_HOST}/{unique_repo}/x:1@{one.digest}"


def test_a_no_edit_relock_keeps_a_cross_repository_diamond(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """With both tags moved, a no-edit `grim lock` moved the digest.

    The lock's own bytes are the assertion: no `[[bundle]]` or `generated_at` churn.
    """
    files = _files("cross")
    left_side = make_artifact(f"{unique_repo}/aa/cross", "skill", files, tag="1")
    right_side = make_artifact(f"{unique_repo}/zz/cross", "skill", files, tag="1")
    assert left_side.digest == right_side.digest
    left = _dependent(unique_repo, "left", [("skill", "cross", left_side.fq)])
    right = _dependent(unique_repo, "right", [("skill", "cross", right_side.fq)])
    write_config(project_dir, skills={"left": left.fq, "right": right.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    before = (project_dir / "grimoire.lock").read_bytes()
    for side in ("aa", "zz"):
        retag(f"{unique_repo}/{side}/cross", "keep", left_side.digest)
        make_artifact(
            f"{unique_repo}/{side}/cross", "skill", _files("cross", "v2"), tag="1"
        )

    runner.run("lock")

    assert (project_dir / "grimoire.lock").read_bytes() == before


def test_remove_then_lock_keeps_a_nested_bundle_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """W-1: `grim remove` of an unrelated skill keeps a nested bundle's pin.

    Only the nested row pins `inner`, and it has to reach the next `grim lock`
    through a real lock file: the moved tag proves the pin was carried.
    """
    member = _skill(unique_repo, "member")
    inner = make_bundle(
        f"{unique_repo}/inner", [("skill", "member", member.fq)], tag="1"
    )
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"other": other.fq}, bundles={"outer": outer.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    retag(f"{unique_repo}/inner", "keep", inner.digest)
    moved = make_bundle(
        f"{unique_repo}/inner", [("skill", "other", other.fq)], tag="1"
    )
    assert moved.digest != inner.digest

    runner.run("remove", "skill", "other")
    runner.run("why", "skill", "member")
    runner.run("lock")

    assert _pins(project_dir, "bundle")["inner"].endswith(inner.digest)
    assert set(_pins(project_dir)) == {"member"}


def test_remove_of_an_overriding_bundle_leaves_the_lock_stale(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """W-2: an override's members must not pass for the include it hid.

    `outer` nests `inner:1`, the direct `inner:2` overrode it. Offline, nothing
    says what `inner:1` holds, so the lock stays stale until `grim lock` reads it.
    """
    old_member = _skill(unique_repo, "old-member")
    new_member = _skill(unique_repo, "new-member")
    repo = f"{unique_repo}/inner"
    old_inner = make_bundle(repo, [("skill", "old-member", old_member.fq)], tag="1")
    new_inner = make_bundle(repo, [("skill", "new-member", new_member.fq)], tag="2")
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", old_inner.fq)], tag="1")
    write_config(project_dir, bundles={"inner": new_inner.fq, "outer": outer.fq})
    runner = grim_at(project_dir)
    runner.run("lock")

    removed = runner.run("remove", "bundle", "inner")
    assert "grim lock" in removed.stderr, removed.stderr

    refused = runner.run("install", check=False)
    assert refused.returncode == 65, f"a stale lock must refuse: {refused.stderr}"

    runner.run("lock")

    assert _pins(project_dir, "bundle")["inner"].endswith(old_inner.digest)
    assert set(_pins(project_dir)) == {"old-member"}
    runner.run("install")
    assert (project_dir / ".claude/skills/old-member/SKILL.md").is_file()


def test_bundle_add_keeps_member_pins(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-005: adding a bundle that shares a member keeps that member's pin."""
    shared = _skill(unique_repo, "shared")
    extra = _skill(unique_repo, "extra")
    held = make_bundle(f"{unique_repo}/held", [("skill", "shared", shared.fq)], tag="1")
    write_config(project_dir, bundles={"held": held.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "shared")
    added = make_bundle(
        f"{unique_repo}/added",
        [("skill", "shared", shared.fq), ("skill", "extra", extra.fq)],
        tag="1",
    )

    result = _add(runner, added, "added", kind="bundle")

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["extra"].endswith(extra.digest)
    assert pins["shared"].endswith(shared.digest)


def test_tag_edit_moves_only_that_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-007: editing a tag moves that entry and leaves a moved neighbour."""
    edited = _skill(unique_repo, "edited")
    target = _skill(unique_repo, "edited", tag="2", body="v2")
    kept = _skill(unique_repo, "kept")
    write_config(project_dir, skills={"edited": edited.fq, "kept": kept.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")
    write_config(project_dir, skills={"edited": target.fq, "kept": kept.fq})

    runner.run("lock")

    pins = _pins(project_dir)
    assert pins["edited"].endswith(target.digest)
    assert pins["kept"].endswith(kept.digest)


def test_repository_change_moves_only_that_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-008: pointing an entry at another repository moves that entry alone."""
    edited = _skill(unique_repo, "edited")
    relocated = make_artifact(
        f"{unique_repo}/elsewhere/edited", "skill", _files("edited", "v2"), tag="1"
    )
    kept = _skill(unique_repo, "kept")
    write_config(project_dir, skills={"edited": edited.fq, "kept": kept.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "kept")
    write_config(project_dir, skills={"edited": relocated.fq, "kept": kept.fq})

    runner.run("lock")

    pins = _pins(project_dir)
    assert pins["edited"].endswith(relocated.digest)
    assert f"{unique_repo}/elsewhere/edited" in pins["edited"]
    assert pins["kept"].endswith(kept.digest)


def test_digest_declared_and_frozen_members_stay(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-009: a digest-declared entry and a `--pin`-frozen member never move.

    Not even under `grim update`, the one command that moves floating pins.
    """
    declared = _skill(unique_repo, "declared")
    frozen = _skill(unique_repo, "frozen")
    pack = make_bundle(
        f"{unique_repo}/pack", [("skill", "frozen", frozen.pinned)], tag="1"
    )
    write_config(
        project_dir, skills={"declared": declared.pinned}, bundles={"pack": pack.fq}
    )
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "declared")
    _move_tag(unique_repo, "frozen")

    runner.run("lock")

    pins = _pins(project_dir)
    assert pins["declared"].endswith(declared.digest)
    assert pins["frozen"].endswith(frozen.digest)

    runner.run("update")

    pins = _pins(project_dir)
    assert pins["declared"].endswith(declared.digest)
    assert pins["frozen"].endswith(frozen.digest)


def test_unpin_keeps_the_digest_until_update(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-010: dropping a declared digest keeps it until `grim update <name>`."""
    pinned = _skill(unique_repo, "unpinned")
    write_config(project_dir, skills={"unpinned": f"{pinned.fq}@{pinned.digest}"})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _move_tag(unique_repo, "unpinned")
    write_config(project_dir, skills={"unpinned": pinned.fq})

    runner.run("lock")

    assert _pins(project_dir)["unpinned"].endswith(pinned.digest)

    runner.run("update", "unpinned")

    assert _pins(project_dir)["unpinned"].endswith(moved.digest)


def test_update_named_moves_a_shared_reference(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-011: two bindings of one reference move independently.

    The named binding must resolve for itself rather than reuse the digest its
    unnamed twin holds for the same reference.
    """
    shared = _skill(unique_repo, "shared")
    write_config(project_dir, skills={"named": shared.fq, "twin": shared.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _move_tag(unique_repo, "shared")

    runner.run("update", "named")

    pins = _pins(project_dir)
    assert pins["named"].endswith(moved.digest)
    assert pins["twin"].endswith(shared.digest)


def test_clean_pre_change_lock_migrates(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-012: a clean pre-change lock gains its tags with no tag resolve."""
    leaf = _skill(unique_repo, "leaf")
    parent = _dependent(unique_repo, "parent", [("skill", "leaf", "./leaf:1")])
    solo = _skill(unique_repo, "solo")
    write_config(project_dir, skills={"parent": parent.fq, "solo": solo.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _rewrite_as_pre_change(project_dir)
    # With no tag cache, a tag resolve here exits 69; a cached tag would answer
    # with the same digests and hide it.
    shutil.rmtree(grim_home / "tags")

    result = runner.run("lock", "--offline", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir) == {
        "leaf": f"{REGISTRY_HOST}/{unique_repo}/leaf:1@{leaf.digest}",
        "parent": f"{REGISTRY_HOST}/{unique_repo}/parent:1@{parent.digest}",
        "solo": f"{REGISTRY_HOST}/{unique_repo}/solo:1@{solo.digest}",
    }
    metadata = _lock(project_dir)["metadata"]
    assert metadata["generated_at"] == _PRE_CHANGE_GENERATED_AT
    assert metadata["lock_version"] == 1
    assert "leaf" not in result.stderr, "a clean lock migrates without a warning"


@pytest.mark.parametrize("command", ["lock", "add", "update"])
def test_pre_change_pin_keeps_its_pin_through_every_relock(
    grim_at, project_dir: Path, registry: str, unique_repo: str, command: str
) -> None:
    """A clean lock keeps a transitive pin through every relocking command.

    The hash proves the declaration and `parent` is read at its pin, so the edge its
    row records is the request that resolved `leaf`. A resolve would show the move.
    """
    leaf = _skill(unique_repo, "leaf")
    parent = _dependent(unique_repo, "parent", [("skill", "leaf", "./leaf:1")])
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"parent": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _rewrite_as_pre_change(project_dir)
    _move_tag(unique_repo, "leaf")

    if command == "add":
        result = _add(runner, other, "other")
    elif command == "update":
        result = runner.run("update", "parent", check=False)
    else:
        result = runner.run("lock", check=False)

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["leaf"] == f"{REGISTRY_HOST}/{unique_repo}/leaf:1@{leaf.digest}"
    assert pins["parent"].endswith(parent.digest)
    assert "no tag recorded" not in result.stderr, result.stderr
    assert _lock(project_dir)["metadata"]["lock_version"] == 1


def test_pre_change_pin_a_moved_parent_disagrees_with_resolves_and_warns(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A moved parent says nothing of what resolved the pin, so its edge moves it."""
    _skill(unique_repo, "leaf")
    parent = _dependent(unique_repo, "parent", [("skill", "leaf", "./leaf:1")])
    write_config(project_dir, skills={"parent": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _rewrite_as_pre_change(project_dir, edges=False)
    wanted = _skill(unique_repo, "leaf", tag="2", body="v2")
    retag(f"{unique_repo}/parent", "keep", parent.digest)
    moved = make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        _files("parent", "v2"),
        [("skill", "leaf", "./leaf:2")],
    )

    result = runner.run("update", "parent", check=False)

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["leaf"].endswith(wanted.digest)
    assert pins["parent"].endswith(moved.digest)
    assert "skill 'leaf'" in result.stderr, "the warning must name the node"
    assert "no tag recorded" in result.stderr, "the warning must name the cause"


def test_drifted_pre_change_lock_resolves_once_and_warns(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-013: a pre-change lock that drifted resolves every pin once, and says so.

    The second move proves "once": the tags that first write stamped carry.
    """
    moving = _skill(unique_repo, "moving")
    steady = _skill(unique_repo, "steady")
    fresh = _skill(unique_repo, "fresh")
    write_config(project_dir, skills={"moving": moving.fq, "steady": steady.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _rewrite_as_pre_change(project_dir)
    write_config(
        project_dir,
        skills={"moving": moving.fq, "steady": steady.fq, "fresh": fresh.fq},
    )
    moved = _move_tag(unique_repo, "moving")

    result = runner.run("lock")

    pins = _pins(project_dir)
    assert pins["moving"].endswith(moved.digest)
    assert pins["steady"].endswith(steady.digest)
    assert pins["fresh"].endswith(fresh.digest)
    assert "moving" in result.stderr and "steady" in result.stderr, result.stderr
    assert "declaration" in result.stderr, "the warning must name the cause"
    assert "skill 'moving' moved, skill 'steady' kept its digest" in result.stderr, (
        "the warning must say which pins moved"
    )
    assert _lock(project_dir)["metadata"]["lock_version"] == 1

    _move_tag(unique_repo, "moving", body="v3")
    runner.run("lock")

    assert _pins(project_dir)["moving"].endswith(moved.digest)


def _pre_change_part(
    grim_at, project_dir: Path, unique_repo: str, *, edges: bool = False
) -> tuple[GrimRunner, Path, PublishedArtifact, PublishedArtifact]:
    """Lock one part as a pre-change build would, then move the tag under its pin.

    `edges=False` also drops `requires`, a shape no grim writes beside a transitive row.
    """
    leaf = _skill(unique_repo, "leaf")
    parent = _dependent(unique_repo, "parent", [("skill", "leaf", "./leaf:1")])
    manifest = project_dir / "marketplace.toml"
    manifest.write_text(f"[plugins.team]\ninclude = [{json.dumps(parent.fq)}]\n")
    runner = grim_at(project_dir)
    runner.run("update", "--marketplace", str(manifest))
    lock = project_dir / "marketplace.lock"
    text = _ADVISORY_TAG.sub(r"\1\2", lock.read_text())
    lock.write_text(text if edges else _REQUIRES_BLOCK.sub("\n", text))
    assert ("requires" in lock.read_text()) == edges
    _move_tag(unique_repo, "leaf")
    return runner, manifest, leaf, parent


def test_marketplace_fresh_part_keeps_a_pin_its_edge_records(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A fresh part proves its include, so a named update keeps the pin below it."""
    runner, manifest, leaf, _ = _pre_change_part(
        grim_at, project_dir, unique_repo, edges=True
    )

    result = runner.run("update", "--marketplace", str(manifest), "team:parent")

    lock = (project_dir / "marketplace.lock").read_text()
    assert f"/leaf:1@{leaf.digest}" in lock, lock
    assert "no tag recorded" not in result.stderr, result.stderr


def test_marketplace_warning_names_the_part(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C-013: in `marketplace.lock` the warning says which part resolved."""
    runner, manifest, leaf, parent = _pre_change_part(
        grim_at, project_dir, unique_repo
    )
    extra = _skill(unique_repo, "extra")
    includes = json.dumps([parent.fq, extra.fq])
    manifest.write_text(f"[plugins.team]\ninclude = {includes}\n")

    result = runner.run(
        "export", "plugin", "--client", "claude", "-o", "dist", "--force", check=False
    )

    assert result.returncode == 0, result.stderr
    assert f"/leaf:1@{leaf.digest}" not in (project_dir / "marketplace.lock").read_text()
    assert "plugin 'team'" in result.stderr, result.stderr
    assert "skill 'leaf'" in result.stderr, result.stderr
    assert "no tag recorded" in result.stderr, "the warning must name the cause"


def test_diamond_records_the_winning_tag(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-014: agreeing edges record their winner, and a moved tag keeps it."""
    target = _skill(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.2", target.digest)
    left = _dependent(unique_repo, "left", [("skill", "target", "./target:1")])
    right = _dependent(unique_repo, "right", [("skill", "target", "./target:1.2")])
    write_config(project_dir, skills={"left": left.fq, "right": right.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    winner = f"{REGISTRY_HOST}/{unique_repo}/target:1@{target.digest}"
    assert _pins(project_dir)["target"] == winner

    moved = _move_tag(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.2", moved.digest)
    runner.run("lock")

    assert _pins(project_dir)["target"] == winner


def test_digest_edge_leaves_the_diamond_pin_tagless(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-014 error case: a digest-bearing edge wins the diamond with no tag."""
    target = _skill(unique_repo, "target")
    left = _dependent(unique_repo, "left", [("skill", "target", "./target:1")])
    right = _dependent(unique_repo, "right", [("skill", "target", target.pinned)])
    write_config(project_dir, skills={"left": left.fq, "right": right.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    assert _pins(project_dir)["target"] == target.pinned


def test_cross_repository_diamond_keeps_the_first_read_id(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-014 error case: across repositories the first-read id stays.

    The later edge sits one level deeper, so it arrives after `target` is read,
    and its repository sorts first, so a pin taken from the smallest id would
    land on it. Two edges of one wave settle before the read and prove nothing.
    """
    files = _files("target")
    first = make_artifact(f"{unique_repo}/zz/target", "skill", files, tag="1")
    later = make_artifact(f"{unique_repo}/aa/target", "skill", files, tag="1")
    assert first.digest == later.digest
    left = _dependent(unique_repo, "left", [("skill", "target", first.fq)])
    _dependent(unique_repo, "middle", [("skill", "target", later.fq)])
    right = _dependent(unique_repo, "right", [("skill", "middle", "./middle:1")])
    write_config(project_dir, skills={"left": left.fq, "right": right.fq})
    runner = grim_at(project_dir)

    runner.run("lock")

    expected = f"{REGISTRY_HOST}/{unique_repo}/zz/target:1@{first.digest}"
    assert _pins(project_dir)["target"] == expected


def test_moved_tag_does_not_fake_a_conflict(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-015: a carried pin that loses a diamond re-resolves instead of exiting 78."""
    _skill(unique_repo, "target")
    first = _dependent(unique_repo, "first", [("skill", "target", "./target:1")])
    write_config(project_dir, skills={"first": first.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _move_tag(unique_repo, "target")
    retag(f"{unique_repo}/target", "1.3", moved.digest)
    second = _dependent(unique_repo, "second", [("skill", "target", "./target:1.3")])

    result = _add(runner, second, "second")

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["target"].endswith(moved.digest)


def test_real_tag_disagreement_still_exits_78(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-015 error case: edges that truly disagree still refuse."""
    _skill(unique_repo, "target")
    first = _dependent(unique_repo, "first", [("skill", "target", "./target:1")])
    write_config(project_dir, skills={"first": first.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = (project_dir / "grimoire.lock").read_bytes()
    _skill(unique_repo, "target", tag="2", body="v2")
    second = _dependent(unique_repo, "second", [("skill", "target", "./target:2")])

    result = _add(runner, second, "second")

    assert result.returncode == 78, result.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == lock


def test_lock_repacks_a_changed_path_source(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-016: a path source has no tag to carry, so `grim lock` re-packs it."""
    source = project_dir / "skills" / "local" / "SKILL.md"
    source.parent.mkdir(parents=True)
    source.write_text("---\nname: local\ndescription: d\n---\n# one\n")
    (project_dir / "grimoire.toml").write_text('[skills]\nlocal = "./skills/local"\n')
    runner = grim_at(project_dir)
    runner.run("lock")
    before = _lock(project_dir)["skill"]
    source.write_text("---\nname: local\ndescription: d\n---\n# two\n")
    assert runner.run("install", check=False).returncode == 65

    runner.run("lock")

    assert _lock(project_dir)["skill"] != before
    runner.run("install")
    assert "# two" in _installed(project_dir, "local")


def test_lock_repairs_stale_dependencies(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-017: a lock missing edges gains them at the same pins."""
    leaf = _skill(unique_repo, "leaf")
    parent = _dependent(unique_repo, "parent", [("skill", "leaf", "./leaf:1")])
    write_config(project_dir, skills={"parent": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock_path = project_dir / "grimoire.lock"
    lock_path.write_text(_REQUIRES_BLOCK.sub("\n", lock_path.read_text()))
    assert "requires" not in lock_path.read_text()
    assert runner.run("install", check=False).returncode == 65

    runner.run("lock")

    assert 'id = "./leaf:1"' in lock_path.read_text()
    pins = _pins(project_dir)
    assert pins["parent"].endswith(parent.digest)
    assert pins["leaf"].endswith(leaf.digest)
    runner.run("install")
    assert "leaf v1" in _installed(project_dir, "leaf")


@pytest.mark.parametrize("path", ["add-over-the-key", "undeclare-the-key"])
def test_shadowed_edge_does_not_answer(
    grim_at, project_dir: Path, registry: str, unique_repo: str, path: str
) -> None:
    """S-032: an edge a direct key shadowed was never resolved, so it vouches for nothing."""
    old = _skill(unique_repo, "target")
    new = _skill(unique_repo, "target", tag="2", body="v2")
    parent = _dependent(unique_repo, "parent", [("skill", "target", "./target:2")])
    write_config(project_dir, skills={"parent": parent.fq, "target": old.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    assert _pins(project_dir)["target"].endswith(old.digest)

    if path == "add-over-the-key":
        result = _add(runner, new, "target")
        assert result.returncode == 0, result.stderr
    else:
        write_config(project_dir, skills={"parent": parent.fq})
        runner.run("lock")

    assert _pins(project_dir)["target"].endswith(new.digest)
    lock_text = (project_dir / "grimoire.lock").read_text()
    assert f":2@{old.digest}" not in lock_text, (
        "tag 2 must not pair with the old digest"
    )


def test_add_target_held_as_a_dependency_keeps_its_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-033: an add at the reference a held pin answers keeps it, as `grim lock` would.

    The binary is the only place that shows what reaches the user: the kept
    pin, an empty `moved`, and the stderr note naming the command that moves it.
    """
    held = _skill(unique_repo, "target")
    parent = _dependent(unique_repo, "parent", [("skill", "target", "./target:1")])
    write_config(project_dir, skills={"parent": parent.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    moved = _move_tag(unique_repo, "target")

    result = runner.run(
        "add", "--kind", "skill", "--name", "target", moved.fq, format="json", check=False
    )

    assert result.returncode == 0, result.stderr
    pins = _pins(project_dir)
    assert pins["target"].endswith(held.digest)
    assert pins["parent"].endswith(parent.digest)
    assert json.loads(result.stdout)["moved"] == []
    assert "`grim update -- target` moves it to what its tag names now, or says why it cannot" in result.stderr
    assert "Declare a reference that exists if that tag is gone" in result.stderr, result.stderr


def test_add_target_held_as_a_dependency_at_another_tag_moves_it_alone(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-033: promoting a dependency at a tag its pin never held moves that key alone.

    A neighbour on an unmoved tag cannot tell a kept pin from a re-resolve, so
    this needs a moved tag; the move itself is listed in `moved`.
    """
    _skill(unique_repo, "target")
    other = _skill(unique_repo, "target", tag="2", body="v2")
    parent = _dependent(unique_repo, "parent", [("skill", "target", "./target:1")])
    solo = _skill(unique_repo, "solo")
    write_config(project_dir, skills={"parent": parent.fq, "solo": solo.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _move_tag(unique_repo, "solo")

    report = runner.json("add", "--kind", "skill", "--name", "target", other.fq)

    pins = _pins(project_dir)
    assert pins["target"].endswith(other.digest)
    assert pins["solo"].endswith(solo.digest)
    assert [(m["kind"], m["name"]) for m in report["moved"]] == [("skill", "target")]


def test_lock_with_a_gone_manifest_names_update(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-034: a carried pin whose manifest is gone refuses and points at `grim update`."""
    repo = f"{unique_repo}/vanished"
    pinned = make_artifact(repo, "skill", _files("vanished", "v1"), tag="1")
    write_config(project_dir, skills={"vanished": pinned.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    # No `keep` tag: moving the only tag is what drops the manifest.
    make_artifact(repo, "skill", _files("vanished", "v2"), tag="1")
    with pytest.raises(urllib.error.HTTPError):
        tag_digest(repo, pinned.digest)
    _evict_from_store(grim_home, pinned.digest)
    lock = (project_dir / "grimoire.lock").read_bytes()

    result = runner.run("lock", check=False)

    assert result.returncode == 79, result.stderr
    assert pinned.digest in result.stderr, result.stderr
    assert "grim update -- vanished" in result.stderr, result.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == lock


def test_offline_lock_takes_a_carried_bundle_from_its_row(
    grim_at, grim_home: Path, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """W-10: offline, the lock's `[[bundle]]` row stands in for a manifest the store lacks.

    A fresh checkout has no store and `grim install` keeps no bundle manifest, so
    a clean lock must relock offline. Only the real cache layer raises the miss.
    """
    member = _skill(unique_repo, "member")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "member", member.fq)], tag="1")
    write_config(project_dir, bundles={"pack": pack.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    lock = (project_dir / "grimoire.lock").read_bytes()

    warm = runner.run("lock", "--offline")
    assert "carried from the lock unverified" not in warm.stderr, warm.stderr

    _evict_from_store(grim_home, pack.digest)

    # A selected bundle has no pin to keep: the cached tag answers, the store does not.
    for names in ((), ("pack",)):
        selected = runner.run("update", "--offline", *names, check=False)
        assert selected.returncode == 69, selected.stderr
        assert (project_dir / "grimoire.lock").read_bytes() == lock

    carried = runner.run("lock", "--offline")
    assert "carried from the lock unverified" in carried.stderr, carried.stderr
    assert (project_dir / "grimoire.lock").read_bytes() == lock

    shutil.rmtree(grim_home / "blobs")
    shutil.rmtree(grim_home / "tags")
    runner.run("lock", "--offline")
    assert (project_dir / "grimoire.lock").read_bytes() == lock

    runner.run("install")
    runner.run("lock", "--offline")
    assert (project_dir / "grimoire.lock").read_bytes() == lock


def test_remove_on_a_stale_lock_keeps_it_stale(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-037: `grim remove` must not launder a hand edit into a fresh-looking lock."""
    edited = _skill(unique_repo, "edited")
    target = _skill(unique_repo, "edited", tag="2", body="v2")
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"edited": edited.fq, "other": other.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    write_config(project_dir, skills={"edited": target.fq, "other": other.fq})

    runner.run("remove", "skill", "other")

    refused = runner.run("install", check=False)
    assert refused.returncode == 65, f"a stale lock must refuse: {refused.stderr}"
    assert _pins(project_dir)["edited"].endswith(edited.digest)

    runner.run("lock")

    assert _pins(project_dir)["edited"].endswith(target.digest)
    runner.run("install")
    assert "edited v2" in _installed(project_dir, "edited")


def _edit_then_hand_undo(
    shape: str, grim_at, project_dir: Path, unique_repo: str
) -> tuple[object, str]:
    """Lock, run one edit that changes a row, restore `grimoire.toml` by hand.

    Returns the runner and the skill whose row the edit took.
    """
    runner = grim_at(project_dir)
    config = project_dir / "grimoire.toml"
    if shape == "fresh-diamond":
        x = _skill(unique_repo, "x", tag="1", body="one")
        two = _skill(unique_repo, "x", tag="2", body="two")
        q = _dependent(unique_repo, "q", [("skill", "x", two.fq)])
        write_config(project_dir, skills={"x": x.fq, "q": q.fq})
        runner.run("lock")
        saved = config.read_text()
        runner.run("remove", "skill", "x")
        taken = "x"
    elif shape == "stale-bundle":
        # A lock with no member lists takes the offline path that evicts a removed
        # bundle's members by their recorded provenance. A moved request makes it
        # stale: a line typed with no row could require `m`, which keeps its row.
        m, b = _skill(unique_repo, "m"), _skill(unique_repo, "b")
        b2 = _skill(unique_repo, "b", tag="2", body="v2")
        pk1 = make_bundle(f"{unique_repo}/pk", [("skill", "m", m.fq)], tag="1")
        write_config(project_dir, skills={"b": b.fq}, bundles={"pk": pk1.fq})
        runner.run("lock")
        saved = config.read_text()
        _drop_bundle_rows(project_dir)
        write_config(project_dir, skills={"b": b2.fq}, bundles={"pk": pk1.fq})
        runner.run("remove", "bundle", "pk")
        taken = "m"
    else:
        a1 = _skill(unique_repo, "a", tag="1")
        a2 = _skill(unique_repo, "a", tag="2", body="v2")
        y = _skill(unique_repo, "y")
        write_config(project_dir, skills={"a": a1.fq, "y": y.fq})
        runner.run("lock")
        saved = config.read_text()
        write_config(project_dir, skills={"a": a2.fq, "y": y.fq})
        verb = "uninstall" if shape == "stale-uninstall" else "remove"
        runner.run(verb, "skill", "y")
        taken = "y"
    assert taken not in _pins(project_dir), "the fixture takes a row"
    config.write_text(saved)
    return runner, taken


@pytest.mark.parametrize(
    "shape", ["stale-uninstall", "stale-remove", "stale-bundle", "fresh-diamond"]
)
def test_a_hand_undo_proves_no_lock_an_edit_took_a_row_from(
    grim_at, project_dir: Path, registry: str, unique_repo: str, shape: str
) -> None:
    """An edit that leaves a lock unproven and takes a row ends its hash.

    Found by review: the edit kept the old hash, so restoring `grimoire.toml`
    by hand proved a lock without the row, and `grim install` exited 0 with the
    declared skill missing. 0.15.1 exits 65 on the stale shapes.
    """
    runner, taken = _edit_then_hand_undo(shape, grim_at, project_dir, unique_repo)

    refused = runner.run("install", check=False)

    assert refused.returncode == 65, f"{shape}: {refused.stderr}"
    runner.run("lock")
    assert taken in _pins(project_dir)
    runner.run("install")


def _beside_a_pack_with_no_member_list(
    project_dir: Path, unique_repo: str, *, declared: tuple[str, ...]
) -> None:
    """`x` is `pack`'s member, `y` is not, and the lock holds no list for `pack`."""
    member, other = _skill(unique_repo, "x"), _skill(unique_repo, "y")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", member.fq)], tag="1")
    published = {"x": member.fq, "y": other.fq}
    skills = {name: published[name] for name in declared}
    write_config(project_dir, skills=skills, bundles={"pack": pack.fq})


def test_a_remove_keeps_a_member_of_a_bundle_with_no_member_list_for_grim_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A direct key the bundle also provides keeps its row and its pin.

    Found by review: the remove dropped the row and restamped, so the next
    `grim lock` resolved it again and moved its pin. The store shows the bundle
    provides it, so the row stays under the bundle's stamp and the lock stays
    proven: `grim install` exits 0, as on 0.15.1, and `grim lock` keeps the pin.
    """
    _beside_a_pack_with_no_member_list(project_dir, unique_repo, declared=("x", "y"))
    runner = grim_at(project_dir)
    runner.run("lock")
    _drop_bundle_rows(project_dir)
    pin = _pins(project_dir)["x"]

    edit = runner.run("remove", "skill", "x")

    assert "no member list" not in edit.stderr, edit.stderr
    assert "x" in _pins(project_dir)
    installed = runner.run("install", check=False)
    assert installed.returncode == 0, installed.stderr
    _move_tag(unique_repo, "x")
    runner.run("lock")
    assert _pins(project_dir)["x"] == pin
    runner.run("install")


def test_an_uninstall_keeps_the_files_of_a_member_with_a_kept_row(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The store shows the bundle provides the key, so its files stay with its row.

    Found by review: the row stayed for `grim lock` while its files went, which
    no other command repairs until the next `grim install`. The row stays under
    the bundle's stamp, so the lock stays proven and `grim install` exits 0.
    """
    _beside_a_pack_with_no_member_list(project_dir, unique_repo, declared=("x", "y"))
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    _drop_bundle_rows(project_dir)

    edit = runner.run("uninstall", "skill", "x")

    assert "no member list" not in edit.stderr, edit.stderr
    assert "x" in _pins(project_dir)
    assert (project_dir / ".claude/skills/x/SKILL.md").is_file()
    installed = runner.run("install", check=False)
    assert installed.returncode == 0, installed.stderr


def test_a_bundle_remove_on_a_grim_0_3_lock_keeps_a_member_another_bundle_provides(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`bz` also provides `m`, which grim 0.3 stamped with `ba` alone.

    Found by review: the remove kept `m`'s row with no stamp, so `grim install`
    exited 65 where 0.15.1 exits 0, and the advised `grim lock` moved `m`'s pin.
    The store shows `bz` provides `m`, so the row stays under `bz`'s stamp.
    """
    m, y = _skill(unique_repo, "m"), _skill(unique_repo, "y")
    ba = make_bundle(f"{unique_repo}/ba", [("skill", "m", m.fq)], tag="1")
    bz = make_bundle(f"{unique_repo}/bz", [("skill", "m", m.fq)], tag="1")
    write_config(project_dir, skills={"y": y.fq}, bundles={"ba": ba.fq, "bz": bz.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    _rewrite_as_grim_0_3(project_dir)

    edit = runner.run("remove", "bundle", "ba")

    assert "no member list" not in edit.stderr, edit.stderr
    row = next(r for r in _lock(project_dir)["skill"] if r["name"] == "m")
    assert row["bundle"] == f"{REGISTRY_HOST}/{unique_repo}/bz", row
    installed = runner.run("install", check=False)
    assert installed.returncode == 0, installed.stderr
    _move_tag(unique_repo, "m")
    relocked = runner.run("lock")
    assert "no tag recorded" not in relocked.stderr, relocked.stderr
    assert _pins(project_dir)["m"] == f"{REGISTRY_HOST}/{unique_repo}/m:1@{m.digest}"


def test_an_uninstall_of_a_member_with_no_direct_line_changes_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A bundle-only member stays, as it does on a lock with a member list.

    0.15.1 exits 0 here too. It deletes the files and takes the row, and the
    next `grim lock` and `grim install` bring both back.
    """
    _beside_a_pack_with_no_member_list(project_dir, unique_repo, declared=("y",))
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    _drop_bundle_rows(project_dir)

    edit = runner.run("uninstall", "skill", "x", format="json")

    assert json.loads(edit.stdout)["status"] == "kept-by-bundle"
    assert (project_dir / ".claude/skills/x/SKILL.md").is_file()
    runner.run("install")


def test_a_remove_of_a_key_the_stored_list_lacks_keeps_the_lock_proven(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The bundle's list in the store shows it does not provide the key.

    Found by review: every remove beside a bundle with no member list left the
    lock stale, a key the bundle does not provide included, where 0.15.1 exits 0.
    """
    _beside_a_pack_with_no_member_list(project_dir, unique_repo, declared=("y",))
    runner = grim_at(project_dir)
    runner.run("lock")
    _drop_bundle_rows(project_dir)

    edit = runner.run("remove", "skill", "y")

    assert "no member list" not in edit.stderr, edit.stderr
    assert "y" not in _pins(project_dir)
    runner.run("install")


def _cold_runner(grim_binary: Path, tmp_path: Path, project_dir: Path) -> GrimRunner:
    """The project on a machine whose store never resolved anything."""
    cold_home = tmp_path / "cold-home"
    cold_home.mkdir()
    return GrimRunner(grim_binary, cold_home, cwd=project_dir)


@pytest.mark.parametrize("verb", ["remove", "uninstall"])
def test_a_cold_store_edit_beside_a_bundle_with_no_member_list_exits_install_0(
    grim_at,
    grim_binary: Path,
    project_dir: Path,
    tmp_path: Path,
    registry: str,
    unique_repo: str,
    verb: str,
) -> None:
    """Nothing offline shows `pack` provides `y`, so the edit is 0.15.1's.

    A fresh clone reads the lock on a machine that never resolved `pack`. The
    row drops and the lock restamps, so `grim install` exits 0 as on 0.15.1.
    Found by review: the row stayed and `grim install` exited 65, a frozen
    exit code changed for an artifact the bundle does not provide. A note names
    the bundle and what the drop costs if it does provide the artifact.
    """
    _beside_a_pack_with_no_member_list(project_dir, unique_repo, declared=("y",))
    grim_at(project_dir).run("lock")
    _drop_bundle_rows(project_dir)
    runner = _cold_runner(grim_binary, tmp_path, project_dir)

    edit = runner.run(verb, "skill", "y")

    assert "bundle 'pack' has no member list" in edit.stderr, edit.stderr
    assert "resolves it afresh" in edit.stderr, edit.stderr
    assert "y" not in _pins(project_dir)
    installed = runner.run("install", check=False)
    assert installed.returncode == 0, installed.stderr


def test_a_cold_store_bundle_remove_evicts_a_member_as_0_15_1_does(
    grim_at,
    grim_binary: Path,
    project_dir: Path,
    tmp_path: Path,
    registry: str,
    unique_repo: str,
) -> None:
    """Nothing offline shows `bz` provides `ba`'s member, so it leaves with `ba`.

    Found by review: `m` stayed for a `grim lock` because `bz` might provide it,
    and `grim install` exited 65 where 0.15.1 exits 0. A note names `bz`.
    """
    m, z = _skill(unique_repo, "m"), _skill(unique_repo, "z")
    ba = make_bundle(f"{unique_repo}/ba", [("skill", "m", m.fq)], tag="1")
    bz = make_bundle(f"{unique_repo}/bz", [("skill", "z", z.fq)], tag="1")
    write_config(project_dir, bundles={"ba": ba.fq, "bz": bz.fq})
    grim_at(project_dir).run("lock")
    _drop_bundle_rows(project_dir)
    runner = _cold_runner(grim_binary, tmp_path, project_dir)

    edit = runner.run("remove", "bundle", "ba")

    assert "bundle 'bz' has no member list" in edit.stderr, edit.stderr
    assert set(_pins(project_dir)) == {"z"}
    installed = runner.run("install", check=False)
    assert installed.returncode == 0, installed.stderr


def test_a_remove_keeps_the_pin_a_line_typed_since_the_lock_may_require(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A line typed since the last `grim lock` has edges only a relock reads.

    Found by review: on a lock with no dependency data the remove read `p` as
    requiring nothing, so it dropped `m`, and the next `grim lock` moved `m`'s
    pin. The row stays and the lock stays stale, as `grim install` found it.
    """
    m = _skill(unique_repo, "m")
    p = _dependent(unique_repo, "p", [("skill", "m", m.fq)])
    write_config(project_dir, skills={"m": m.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    pin = _pins(project_dir)["m"]
    write_config(project_dir, skills={"m": m.fq, "p": p.fq})

    edit = runner.run("remove", "skill", "m")

    assert "skill 'p'" in edit.stderr, edit.stderr
    assert runner.run("install", check=False).returncode == 65
    _move_tag(unique_repo, "m")
    runner.run("lock")
    assert _pins(project_dir)["m"] == pin


def test_a_ghost_typed_into_the_declaration_stays_gone_after_its_uninstall(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A row nothing reaches leaves with the line typed over it.

    Found by review: the row stayed under the hash that proves the edited
    declaration, so the next `grim install` brought back the files `grim
    uninstall` had just deleted.
    """
    kept, ghost = _skill(unique_repo, "x"), _skill(unique_repo, "g")
    write_config(project_dir, skills={"x": kept.fq, "g": ghost.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")
    with_ghost = (project_dir / "grimoire.lock").read_text()
    write_config(project_dir, skills={"x": kept.fq})
    runner.run("lock")
    proof = tomllib.loads((project_dir / "grimoire.lock").read_text())
    (project_dir / "grimoire.lock").write_text(
        re.sub(
            r'declaration_hash = ".*"',
            f'declaration_hash = "{proof["metadata"]["declaration_hash"]}"',
            with_ghost,
        )
    )
    write_config(project_dir, skills={"x": kept.fq, "g": ghost.fq})

    runner.run("uninstall", "skill", "g")

    assert "g" not in _pins(project_dir)
    runner.run("install")
    assert not (project_dir / ".claude/skills/g/SKILL.md").exists()


def test_an_edit_ending_the_proof_stamps_a_pre_change_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An edit that ends the proof stamps first: it is the last holder of the proof.

    Found by review: `y` has no tag and a moved one, so with the proof gone
    `grim lock` moved a pin the source says stays.
    """
    bystander = _skill(unique_repo, "y", tag="1")
    target = _skill(unique_repo, "x", tag="1")
    retag(f"{unique_repo}/x", "2", target.digest)
    p = _dependent(unique_repo, "p", [("skill", "x", "./x:1")])
    q = _dependent(unique_repo, "q", [("skill", "x", "./x:2")])
    skills = {"x": target.fq, "p": p.fq, "q": q.fq, "y": bystander.fq}
    write_config(project_dir, skills=skills)
    runner = grim_at(project_dir)
    runner.run("lock")
    _rewrite_as_pre_change(project_dir)
    _move_tag(unique_repo, "y")
    proof = _lock(project_dir)["metadata"]["declaration_hash"]

    runner.run("remove", "skill", "x")

    assert _lock(project_dir)["metadata"]["declaration_hash"] not in ("", proof)
    relocked = runner.run("lock", check=False)
    assert relocked.returncode == 0, relocked.stderr
    assert _pins(project_dir)["y"].endswith(bystander.digest), relocked.stderr
    assert "re-resolved" not in relocked.stderr, relocked.stderr


def test_stale_hash_pre_change_lock(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An edit that leaves the hash stale ends the proof the same way, and stamps too."""
    bystander = _skill(unique_repo, "y", tag="1")
    declared = _skill(unique_repo, "x", tag="1", body="one")
    required = _skill(unique_repo, "x", tag="2", body="two")
    q = _dependent(unique_repo, "q", [("skill", "x", "./x:2")])
    write_config(project_dir, skills={"x": declared.fq, "q": q.fq, "y": bystander.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    _rewrite_as_pre_change(project_dir)
    _move_tag(unique_repo, "y")

    runner.run("remove", "skill", "x")

    assert _lock(project_dir)["metadata"]["declaration_hash"] != ""
    assert "x" not in _pins(project_dir)
    relocked = runner.run("lock", check=False)
    assert relocked.returncode == 0, relocked.stderr
    pins = _pins(project_dir)
    assert pins["y"].endswith(bystander.digest), relocked.stderr
    assert pins["x"].endswith(required.digest)


def test_a_remove_keeps_an_unrelated_diamond_pin(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A remove that leaves the lock unproven costs an unrelated diamond nothing.

    Found by review: the remove of `z` ended the proof for every key, so with one
    tag of the untouched `x` diamond moved, `grim lock` exited 78. `x` names a
    recorded edge, and on a stale lock that holds every recorded edge.
    """
    kept = _skill(unique_repo, "x", tag="1")
    retag(f"{unique_repo}/x", "2", kept.digest)
    p = _dependent(unique_repo, "p", [("skill", "x", "./x:1")])
    q = _dependent(unique_repo, "q", [("skill", "x", "./x:2")])
    z = _skill(unique_repo, "z", tag="1")
    retag(f"{unique_repo}/z", "2", z.digest)
    r = _dependent(unique_repo, "r", [("skill", "z", "./z:1")])
    s = _dependent(unique_repo, "s", [("skill", "z", "./z:2")])
    write_config(project_dir, skills={"p": p.fq, "q": q.fq, "z": z.fq, "r": r.fq, "s": s.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("remove", "skill", "z")
    _move_tag(unique_repo, "x", tag="2")

    relocked = runner.run("lock", check=False)

    assert relocked.returncode == 0, relocked.stderr
    pins = _pins(project_dir)
    assert pins["x"].endswith(kept.digest), relocked.stderr
    assert pins["z"].endswith(z.digest), relocked.stderr


def test_a_stale_pin_naming_no_recorded_edge_moves_with_a_warning(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """On a stale lock a pin naming none of its recorded edges vouches for none.

    Found by review: `x` kept `x:1` beside the surviving edge `x:2`, and after an
    unrelated hand edit `grim lock` moved it with stderr empty.
    """
    kept = _skill(unique_repo, "x", tag="1")
    retag(f"{unique_repo}/x", "2", kept.digest)
    p = _dependent(unique_repo, "p", [("skill", "x", "./x:1")])
    q = _dependent(unique_repo, "q", [("skill", "x", "./x:2")])
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"p": p.fq, "q": q.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("remove", "skill", "p")
    moved = _move_tag(unique_repo, "x", tag="2")
    write_config(project_dir, skills={"q": q.fq, "other": other.fq})

    relocked = runner.run("lock", check=False)

    assert relocked.returncode == 0, relocked.stderr
    assert _pins(project_dir)["x"].endswith(moved.digest)
    assert "moved skill 'x'" in relocked.stderr, relocked.stderr
    assert "skill 'other'" not in relocked.stderr, "a declared key is not named"
