# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim export` reconciles a stale plugin without moving what it did not change."""

from __future__ import annotations

import json
import re
import subprocess
import urllib.request
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, make_dependent
from src.registry import REGISTRY_BASE, PublishedArtifact, retag, tag_digest
from src.runner import GrimRunner

_KINDS = ("skill", "rule", "agent", "mcp")
# Strips `:tag` from `pinned` only: a registry port is followed by `/`, never
# by `@sha256:`.
_ADVISORY_TAG = re.compile(r'(?m)^(pinned = "[^"@]*):[^/"@]+(@sha256:)')
# Strips the digest a part's stamp records, which grim 0.15.1's re-resolving writes
# leave out.
_RECORDED_DIGEST = re.compile(r'(?m)^(bundle_tag = "[^"@]*)@sha256:[0-9a-f]+"')


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    d = tmp_path / "work"
    d.mkdir()
    return d


def _files(name: str, body: str = "") -> dict[str, str]:
    return {
        f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"
    }


def _skill(unique_repo: str, name: str, body: str = "v1") -> PublishedArtifact:
    return make_artifact(f"{unique_repo}/{name}", "skill", _files(name, body), tag="1")


def _move_tag(unique_repo: str, name: str, body: str = "v2") -> PublishedArtifact:
    """Publish new bytes under `:1`; `keep` stops zot dropping the old manifest."""
    repo = f"{unique_repo}/{name}"
    old = tag_digest(repo, "1")
    retag(repo, "keep", old)
    moved = make_artifact(repo, "skill", _files(name, body), tag="1")
    assert moved.digest != old, "a moved tag must change the digest"
    return moved


def _write_marketplace(work: Path, includes: list[str]) -> None:
    (work / "marketplace.toml").write_text(
        f"[plugins.team]\ninclude = {json.dumps(includes)}\n"
    )


def _export(runner: GrimRunner, *args: str) -> subprocess.CompletedProcess[str]:
    return runner.run(
        "export", "plugin", "--client", "claude", "-o", "dist", *args, check=False
    )


def _rows(work: Path) -> dict[str, dict]:
    """`name -> entry` for the `team` part of the marketplace lock."""
    lock = tomllib.loads((work / "marketplace.lock").read_text())
    return {
        entry["name"]: entry
        for kind in _KINDS
        for entry in lock.get(kind, [])
        if entry.get("plugin") == "team"
    }


def _pins(work: Path) -> dict[str, str]:
    """`name -> pinned` for the `team` part of the marketplace lock."""
    return {name: row["pinned"] for name, row in _rows(work).items()}


def _forget_recorded_digests(work: Path) -> None:
    """Leave the stamps as a lock grim 0.15.1 wrote holds them."""
    lock = work / "marketplace.lock"
    lock.write_text(_RECORDED_DIGEST.sub(r'\1"', lock.read_text()))


def test_export_keeps_unchanged_includes(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """S-029: adding an include resolves it and leaves the includes already pinned."""
    kept = _skill(unique_repo, "kept")
    fresh = _skill(unique_repo, "fresh")
    runner = grim_at(work)
    _write_marketplace(work, [kept.fq])
    first = _export(runner)
    assert first.returncode == 0, first.stderr
    _move_tag(unique_repo, "kept")
    _write_marketplace(work, [kept.fq, fresh.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    pins = _pins(work)
    assert pins["fresh"].endswith(fresh.digest)
    assert pins["kept"].endswith(kept.digest)


def test_export_resolves_only_the_new_include(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """S-029: moved tags reach neither the rows of a stale plugin nor its export."""
    first = _skill(unique_repo, "first")
    second = _skill(unique_repo, "second")
    fresh = _skill(unique_repo, "fresh")
    runner = grim_at(work)
    _write_marketplace(work, [first.fq, second.fq])
    exported = _export(runner)
    assert exported.returncode == 0, exported.stderr
    before = _rows(work)
    _move_tag(unique_repo, "first")
    _move_tag(unique_repo, "second")
    _write_marketplace(work, [first.fq, second.fq, fresh.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert rows.pop("fresh")["pinned"].endswith(fresh.digest)
    assert rows == before
    tree = work / "dist" / "team.claude" / "skills"
    assert "# first v1" in (tree / "first" / "SKILL.md").read_text()
    assert "# fresh v1" in (tree / "fresh" / "SKILL.md").read_text()


def test_tagless_part_resolves_an_unchanged_include_once_and_warns(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """C-013, C-031: S-029 needs a recorded tag, and a 0.15.1 part has none.

    The include list before the edit is gone, so a tagless pin cannot be told
    from one the edit retargeted. It resolves once and says so, and the write
    records the tag that lets the next edit keep it.
    """
    kept = _skill(unique_repo, "kept")
    fresh = _skill(unique_repo, "fresh")
    later = _skill(unique_repo, "later")
    runner = grim_at(work)
    _write_marketplace(work, [kept.fq])
    exported = _export(runner)
    assert exported.returncode == 0, exported.stderr
    lock = work / "marketplace.lock"
    tagless = _ADVISORY_TAG.sub(r"\1\2", lock.read_text())
    lock.write_text(tagless)
    moved = _move_tag(unique_repo, "kept")

    unedited = _export(runner, "--force")

    assert unedited.returncode == 0, unedited.stderr
    assert lock.read_text() == tagless, "a fresh plugin resolves and rewrites nothing"
    assert "re-resolved" not in unedited.stderr

    _write_marketplace(work, [kept.fq, fresh.fq])
    edited = _export(runner, "--force")

    assert edited.returncode == 0, edited.stderr
    assert _pins(work)["kept"].endswith(moved.digest)
    assert "plugin 'team'" in edited.stderr and "skill 'kept'" in edited.stderr

    _move_tag(unique_repo, "kept", "v3")
    _write_marketplace(work, [kept.fq, fresh.fq, later.fq])
    again = _export(runner, "--force")

    assert again.returncode == 0, again.stderr
    assert _pins(work)["kept"].endswith(moved.digest)
    assert "re-resolved" not in again.stderr


def _exported_pack(
    grim_at, work: Path, unique_repo: str
) -> tuple[GrimRunner, PublishedArtifact, PublishedArtifact]:
    """Export `team` with one bundle include; returns the runner, `pack` and `held`."""
    held = _skill(unique_repo, "held")
    leaving = _skill(unique_repo, "leaving")
    pack = make_bundle(
        f"{unique_repo}/pack",
        [("skill", "held", held.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq])
    first = _export(runner)
    assert first.returncode == 0, first.stderr
    assert set(_pins(work)) == {"held", "leaving"}
    return runner, pack, held


def _roll_pack(unique_repo: str, pack: PublishedArtifact, held: PublishedArtifact):
    """Move `held`, then move `pack` onto a manifest listing `held` and `arriving`."""
    moved = _move_tag(unique_repo, "held")
    arriving = _skill(unique_repo, "arriving")
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("skill", "held", held.fq), ("skill", "arriving", arriving.fq)],
        tag="1",
    )
    return moved, arriving


def test_bundle_include_follows_its_tag(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """S-036: a part that records no digest for a bundle follows its tag.

    A lock grim 0.15.1 wrote holds none, so only the members the bundle still
    lists can keep what they hold.
    """
    runner, pack, held = _exported_pack(grim_at, work, unique_repo)
    _forget_recorded_digests(work)
    _, arriving = _roll_pack(unique_repo, pack, held)
    extra = _skill(unique_repo, "extra")
    _write_marketplace(work, [pack.fq, extra.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    pins = _pins(work)
    assert set(pins) == {"held", "arriving", "extra"}
    assert pins["arriving"].endswith(arriving.digest)
    assert pins["extra"].endswith(extra.digest)
    assert pins["held"].endswith(held.digest)
    assert "plugin 'team' follows the tag of bundle 'pack'" in result.stderr


def test_bundle_include_keeps_its_recorded_digest(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """A part keeps its bundle's pin in `bundle_tag`, so an edit beside it moves nothing."""
    runner, pack, held = _exported_pack(grim_at, work, unique_repo)
    stamped = {name: row["bundle_tag"] for name, row in _rows(work).items()}
    assert stamped == {"held": f"1@{pack.digest}", "leaving": f"1@{pack.digest}"}
    before = _pins(work)
    _roll_pack(unique_repo, pack, held)
    extra = _skill(unique_repo, "extra")
    _write_marketplace(work, [pack.fq, extra.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    pins = _pins(work)
    assert pins.pop("extra").endswith(extra.digest)
    assert pins == before
    assert "follows the tag" not in result.stderr
    assert _rows(work)["held"]["bundle_tag"] == f"1@{pack.digest}"


def test_member_update_keeps_a_bundle_and_a_whole_update_moves_it(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    runner, pack, held = _exported_pack(grim_at, work, unique_repo)
    extra = _skill(unique_repo, "extra")
    _write_marketplace(work, [pack.fq, extra.fq])
    runner.run("update", "--marketplace", "marketplace.toml")
    before = _pins(work)
    _roll_pack(unique_repo, pack, held)

    member = runner.run("update", "--marketplace", "marketplace.toml", "team:extra")

    assert _pins(work) == before
    assert "follows the tag" not in member.stderr

    runner.run("update", "--marketplace", "marketplace.toml", "team")

    moved = tag_digest(f"{unique_repo}/pack", "1")
    assert set(_pins(work)) == {"held", "arriving", "extra"}
    assert _rows(work)["held"]["bundle_tag"] == f"1@{moved}"


@pytest.mark.parametrize("change", ["added member", "bundle bytes only"])
def test_a_bundle_pick_moves_the_bundle_and_records_its_digest_on_every_row(
    grim_at, work: Path, registry: str, unique_repo: str, change: str
) -> None:
    """A `<plugin>:<bundle>` pick moves that bundle, where grim 0.15.1 exited 79.

    Each row below it records the digest it moved to. Rows the move left unchanged
    kept the old digest, beside the added member's new one or on their own.
    """
    runner, pack, held = _exported_pack(grim_at, work, unique_repo)
    leaving = tag_digest(f"{unique_repo}/leaving", "1")
    members = [
        ("skill", "held", held.fq),
        ("skill", "leaving", f"{registry}/{unique_repo}/leaving:1"),
    ]
    below = {"held", "leaving"}
    if change == "added member":
        members.append(("skill", "arriving", _skill(unique_repo, "arriving").fq))
        below.add("arriving")
    else:
        members.reverse()
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    moved = make_bundle(f"{unique_repo}/pack", members, tag="1")
    assert moved.digest != pack.digest, "the tag must move"

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:pack", check=False
    )

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert set(rows) == below
    assert {name: rows[name]["bundle_tag"] for name in below} == dict.fromkeys(
        below, f"1@{moved.digest}"
    )
    assert rows["leaving"]["pinned"].endswith(leaving)


def test_a_second_write_changes_no_byte(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    runner, pack, _held = _exported_pack(grim_at, work, unique_repo)
    extra = _skill(unique_repo, "extra")
    _write_marketplace(work, [pack.fq, extra.fq])
    runner.run("update", "--marketplace", "marketplace.toml")
    lock = work / "marketplace.lock"
    before = lock.read_bytes()

    runner.run("update", "--marketplace", "marketplace.toml", "team:extra")

    assert lock.read_bytes() == before


def _strings(document: object) -> list[str]:
    """Every key and string value of a JSON document."""
    if isinstance(document, dict):
        return [s for k, v in document.items() for s in (k, *_strings(v))]
    if isinstance(document, list):
        return [s for item in document for s in _strings(item)]
    return [document] if isinstance(document, str) else []


def test_no_json_output_echoes_a_recorded_digest(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """`bundle_tag` may read `<tag>@<digest>`; every report keeps its value form."""
    runner, pack, _held = _exported_pack(grim_at, work, unique_repo)
    (work / "marketplace.toml").write_text(
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        + (work / "marketplace.toml").read_text()
    )
    stamp = f"1@{pack.digest}"
    assert stamp in (work / "marketplace.lock").read_text()
    reports = {
        "member update": runner.json(
            "update", "--marketplace", "marketplace.toml", "team:held"
        ),
        "whole update": runner.json("update", "--marketplace", "marketplace.toml"),
        "export plugin": runner.json(
            "export", "plugin", "--client", "claude", "-o", "dist", "--force"
        ),
        "export marketplace": runner.json("export", "marketplace"),
    }

    for name, report in reports.items():
        strings = _strings(report)
        assert not [s for s in strings if stamp in s], name
        assert not {"bundle", "bundle_tag", "bundles"} & set(strings), name


def test_tagless_bundle_member_warns_naming_the_part(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """S-036 error case: a tagless member has no recorded edge, so it resolves and warns."""
    runner, pack, held = _exported_pack(grim_at, work, unique_repo)
    lock = work / "marketplace.lock"
    lock.write_text(_ADVISORY_TAG.sub(r"\1\2", lock.read_text()))
    _forget_recorded_digests(work)
    moved, _ = _roll_pack(unique_repo, pack, held)
    extra = _skill(unique_repo, "extra")
    _write_marketplace(work, [pack.fq, extra.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    assert _pins(work)["held"].endswith(moved.digest)
    assert "held" in result.stderr and "team" in result.stderr, result.stderr


_OFFLINE_RUNS = {
    "export plugin": (
        "export",
        "plugin",
        "--plugin",
        "team",
        "--client",
        "claude",
        "-o",
        "dist",
        "--force",
    ),
    "export marketplace": ("export", "marketplace"),
    "member update": ("update", "--marketplace", "marketplace.toml", "team:extra"),
}


@pytest.mark.parametrize("run", list(_OFFLINE_RUNS))
def test_an_offline_relock_stops_at_a_recorded_bundle_this_machine_never_read(
    tmp_path: Path, grim_binary: Path, registry: str, unique_repo: str, run: str
) -> None:
    """A part's recorded bundle digest the store lacks exits 81 offline, lock untouched.

    The tag cache here names the bundle's newer digest, whose manifest and members
    the store holds: following it would add `arriving` and move the recorded pin.
    """
    held = _skill(unique_repo, "held")
    extra = _skill(unique_repo, "extra")
    fresh = _skill(unique_repo, "fresh")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    header = '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'

    def manifest(directory: Path, plugin: str, includes: list[str]) -> None:
        directory.mkdir(exist_ok=True)
        (directory / "marketplace.toml").write_text(
            f"{header}[plugins.{plugin}]\ninclude = {json.dumps(includes)}\n"
        )

    origin = tmp_path / "origin"
    manifest(origin, "team", [pack.fq, extra.fq])
    writer = GrimRunner(grim_binary, tmp_path / "writer" / "grim-home", cwd=origin)
    writer.run("update", "--marketplace", "marketplace.toml")
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    arriving = _skill(unique_repo, "arriving")
    members = [("skill", "held", held.fq), ("skill", "arriving", arriving.fq)]
    make_bundle(f"{unique_repo}/pack", members, tag="1")

    home = tmp_path / "here" / "grim-home"
    other = tmp_path / "other"
    manifest(other, "other", [pack.fq, extra.fq, fresh.fq])
    _export(GrimRunner(grim_binary, home, cwd=other), "--plugin", "other", "--force")

    consumer = tmp_path / "consumer"
    stale = run != "member update"
    manifest(consumer, "team", [pack.fq, extra.fq, *([fresh.fq] if stale else [])])
    lock = consumer / "marketplace.lock"
    lock.write_bytes((origin / "marketplace.lock").read_bytes())
    before = lock.read_bytes()

    result = GrimRunner(grim_binary, home, cwd=consumer).run(
        "--offline", *_OFFLINE_RUNS[run], check=False
    )

    assert result.returncode == 81, result.stderr
    assert lock.read_bytes() == before


@pytest.mark.parametrize("nested", [False, True], ids=["conflict-only", "outer-nests"])
@pytest.mark.parametrize("pick", ["team:m", "team:y", "team:x"])
def test_a_pick_refuses_a_moved_bundle_list_that_conflicts_with_a_direct_include(
    grim_at, work: Path, registry: str, unique_repo: str, pick: str, nested: bool
) -> None:
    """`inner:1` moves onto a list naming `x:1`, and the plugin includes `x:2` itself.

    The part reads as grim 0.15.1 writes it: tagless pins, plain stamps, and no digest for
    either bundle. A pick selecting neither bundle still meets the conflict a whole update
    meets, so it exits 78 naming `x` and leaves the part as it was. It exited 0.
    """
    x1 = _skill(unique_repo, "x")
    x2 = make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    m = _skill(unique_repo, "m")
    y = _skill(unique_repo, "y")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "m", m.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [outer.fq, inner.fq, x2.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    lock = work / "marketplace.lock"
    written = _RECORDED_DIGEST.sub(r'\1"', _ADVISORY_TAG.sub(r"\1\2", lock.read_text()))
    lock.write_text(written)
    retag(f"{unique_repo}/inner", "keep", inner.digest)
    make_bundle(
        f"{unique_repo}/inner", [("skill", "m", m.fq), ("skill", "x", x1.fq)], tag="1"
    )
    if nested:
        retag(f"{unique_repo}/outer", "keep", outer.digest)
        make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")

    whole = runner.run(
        "update", "--marketplace", "marketplace.toml", "team", check=False
    )
    assert whole.returncode == 78, whole.stderr
    assert lock.read_text() == written

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", pick, check=False
    )

    assert result.returncode == 78, result.stderr
    assert "skill 'x'" in result.stderr, result.stderr
    assert lock.read_text() == written


def test_a_pick_of_an_absent_name_exits_79_before_a_conflict_with_no_part(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """With no `marketplace.lock` the selector is checked first, as in 0.15.1.

    `pack:1` lists `x:1` and the plugin includes `x:2` itself, a conflict a whole update
    refuses with 78. A pick of a name nothing holds still exits 79, and writes no lock.
    """
    x1 = _skill(unique_repo, "x")
    make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", x1.fq)], tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, f"{x1.fq[:-1]}2"])

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:absent", check=False
    )

    assert result.returncode == 79, result.stderr
    assert not (work / "marketplace.lock").exists()


def _stamp_tags(row: dict) -> set[str]:
    """Every `<tag>[@<digest>]` stamp a row carries."""
    tags = {stamp["tag"] for stamp in row.get("bundles", [])}
    if "bundle_tag" in row:
        tags.add(row["bundle_tag"])
    return tags


def _stamps_of(row: dict, bundle: str) -> set[str]:
    """The `<tag>[@<digest>]` stamps of `row` that name the bundle repository `bundle`."""
    tags = {
        stamp["tag"]
        for stamp in row.get("bundles", [])
        if stamp["repo"].endswith(f"/{bundle}")
    }
    if row.get("bundle_tag") and row.get("bundle", "").endswith(f"/{bundle}"):
        tags.add(row["bundle_tag"])
    return tags


def _delete(repo: str, reference: str) -> None:
    """Delete a manifest by digest, or a tag by name."""
    request = urllib.request.Request(
        f"{REGISTRY_BASE}/v2/{repo}/manifests/{reference}", method="DELETE"
    )
    with urllib.request.urlopen(request):
        pass


def _as_c5e3824_wrote(work: Path) -> None:
    """Leave the part as c5e3824 wrote it: no digest in any stamp, no tag in any pin."""
    lock = work / "marketplace.lock"
    text = _ADVISORY_TAG.sub(r"\1\2", lock.read_text())
    lock.write_text(_RECORDED_DIGEST.sub(r'\1"', text))


def test_an_export_keeps_a_recorded_nested_bundle_below_a_parent_that_records_none(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """`inner` records its digest, `outer` none, and `outer` then nests `inner`.

    The export that drops the `inner` include reads `inner` through `outer`. A part
    keeps a recorded bundle at its digest whichever parent reaches it, so `x` stays and
    `z`, which `inner:1` lists now, does not arrive. The export read `inner`'s tag and
    replaced `x` by `z`.
    """
    x = _skill(unique_repo, "x")
    y = _skill(unique_repo, "y")
    z = _skill(unique_repo, "z")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "x", x.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [outer.fq, inner.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    lock = work / "marketplace.lock"
    lock.write_text(lock.read_text().replace(f"@{outer.digest}", ""))
    retag(f"{unique_repo}/inner", "keep", inner.digest)
    make_bundle(f"{unique_repo}/inner", [("skill", "z", z.fq)], tag="1")
    retag(f"{unique_repo}/outer", "keep", outer.digest)
    make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    _write_marketplace(work, [outer.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert set(rows) == {"x"}, set(rows)
    assert f"1@{inner.digest}" in _stamp_tags(rows["x"]), rows["x"]


def _mk_f1(grim_at, work: Path, unique_repo: str, *, roll: bool = True):
    """`team` = [`pack:1` (held:1, leaving), `pk2:1` (y, b), `extra`], `y:1` requiring `held:1`.

    Locked by the current grim, then left as c5e3824 wrote it. With `roll`, `pack:1`
    rolls onto `held:2` and `y:1` onto a manifest requiring `held:2`.
    """
    held1 = _skill(unique_repo, "held")
    held2 = make_artifact(f"{unique_repo}/held", "skill", _files("held", "v2"), tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", _files("y"), [("skill", "held", held1.fq)]
    )
    members = [("skill", "held", held1.fq), ("skill", "leaving", leaving.fq)]
    pack = make_bundle(f"{unique_repo}/pack", members, tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, pk2.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    if not roll:
        return runner, held1
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    members = [("skill", "held", held2.fq), ("skill", "leaving", leaving.fq)]
    make_bundle(f"{unique_repo}/pack", members, tag="1")
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y",
        "1",
        "skill",
        _files("y", "v2"),
        [("skill", "held", held2.fq)],
    )
    return runner, held1


def test_a_pick_refuses_a_dependent_beside_a_newer_version_it_requires(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """The pick would keep `y` at its pin beside `held:2`, which `y` does not require.

    `y:1`'s tag and `pack`'s moved together, and the part records neither. A pick of
    `held` or `pack` that took `held:2` exited 0 and left a part `grim export` refuses
    with 65. Both refuse with 78 now and write nothing; a pick of `extra` moves no
    row `y` requires and still passes.
    """
    runner, _held1 = _mk_f1(grim_at, work, unique_repo)
    lock = work / "marketplace.lock"
    written = lock.read_text()

    for pick in ("team:held", "team:pack"):
        result = runner.run(
            "update", "--marketplace", "marketplace.toml", pick, check=False
        )

        assert result.returncode == 78, (pick, result.stderr)
        assert "skill 'y'" in result.stderr and "requires" in result.stderr, (
            result.stderr
        )
        assert lock.read_text() == written

    extra = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:extra", check=False
    )
    assert extra.returncode == 0, extra.stderr


def test_a_pick_reads_an_unrecorded_include_republished_as_a_skill_as_a_skill(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """`pack:1` held `x`, and the part records no digest for it; it is a skill now.

    The pick of `extra` read the include as the bundle the part's stamp names and
    exited 65. The include reads its tag for its kind, so the pick passes, and a pick
    of `x`, which left with the bundle, exits 79.
    """
    x = _skill(unique_repo, "x")
    extra = _skill(unique_repo, "extra")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", x.fq)], tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_artifact(f"{unique_repo}/pack", "skill", _files("pack"), tag="1")

    picked = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:extra", check=False
    )
    assert picked.returncode == 0, picked.stderr

    lock = work / "marketplace.lock"
    before = lock.read_text()
    gone = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:x", check=False
    )
    assert gone.returncode == 79, gone.stderr
    assert lock.read_text() == before


@pytest.mark.parametrize("written", ["recorded", "forgotten"])
def test_a_bundle_pick_of_a_bundle_republished_as_a_skill_drops_its_old_members(
    grim_at, work: Path, registry: str, unique_repo: str, written: str
) -> None:
    """`pack:1` held `x` and `leaving`; it is a skill now, and a pick of `pack` drops them."""
    x = _skill(unique_repo, "x")
    leaving = _skill(unique_repo, "leaving")
    extra = _skill(unique_repo, "extra")
    members = [("skill", "x", x.fq), ("skill", "leaving", leaving.fq)]
    pack = make_bundle(f"{unique_repo}/pack", members, tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    if written == "forgotten":
        _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_artifact(f"{unique_repo}/pack", "skill", _files("pack"), tag="1")

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:pack", check=False
    )

    assert result.returncode == 0, result.stderr
    assert set(_rows(work)) == {"extra", "pack"}, set(_rows(work))


def test_an_offline_pick_of_a_moved_recorded_bundle_needs_its_recorded_list(
    tmp_path: Path, grim_binary: Path, registry: str, unique_repo: str
) -> None:
    """Preservation control, green on the base: nothing here is new.

    The part records `pack` at its first digest; the tag moved, and this machine holds
    the second digest only. A pick of `pack` needs the first list to know the rows the
    bundle let go, and the store lacks it: 81, as grim 0.15.1 gives. A whole update
    needs no old list and passes.
    """
    held = _skill(unique_repo, "held")
    extra = _skill(unique_repo, "extra")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    header = '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'

    def manifest(directory: Path, plugin: str, includes: list[str]) -> None:
        directory.mkdir(exist_ok=True)
        (directory / "marketplace.toml").write_text(
            f"{header}[plugins.{plugin}]\ninclude = {json.dumps(includes)}\n"
        )

    origin = tmp_path / "origin"
    manifest(origin, "team", [pack.fq, extra.fq])
    writer = GrimRunner(grim_binary, tmp_path / "writer" / "grim-home", cwd=origin)
    writer.run("update", "--marketplace", "marketplace.toml")
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    arriving = _skill(unique_repo, "arriving")
    members = [("skill", "held", held.fq), ("skill", "arriving", arriving.fq)]
    make_bundle(f"{unique_repo}/pack", members, tag="1")

    home = tmp_path / "here" / "grim-home"
    other = tmp_path / "other"
    manifest(other, "other", [pack.fq, extra.fq])
    _export(GrimRunner(grim_binary, home, cwd=other), "--plugin", "other", "--force")
    consumer = tmp_path / "consumer"
    manifest(consumer, "team", [pack.fq, extra.fq])
    lock = consumer / "marketplace.lock"
    lock.write_bytes((origin / "marketplace.lock").read_bytes())
    before = lock.read_bytes()
    reader = GrimRunner(grim_binary, home, cwd=consumer)

    pick = reader.run(
        "--offline",
        "update",
        "--marketplace",
        "marketplace.toml",
        "team:pack",
        check=False,
    )
    assert pick.returncode == 81, pick.stderr
    assert lock.read_bytes() == before

    whole = reader.run(
        "--offline", "update", "--marketplace", "marketplace.toml", "team", check=False
    )
    assert whole.returncode == 0, whole.stderr


def test_a_part_conflict_names_the_plugin_include(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """`pk2` lists `y` but not `held`; `y:1` requires `held:1` and `pack` rolls onto `held:2`.

    The part's 78 told the reader to declare `held` in `grimoire.toml`, a file a
    marketplace repository does not have. It names the include and the manifest.
    """
    held1 = _skill(unique_repo, "held")
    held2 = make_artifact(f"{unique_repo}/held", "skill", _files("held", "v2"), tag="2")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", _files("y"), [("skill", "held", held1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held1.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, pk2.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(f"{unique_repo}/pack", [("skill", "held", held2.fq)], tag="1")
    lock = work / "marketplace.lock"
    written = lock.read_text()

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:pack", check=False
    )

    assert result.returncode == 78, result.stderr
    assert "include skill held directly in plugin 'team' of" in result.stderr, (
        result.stderr
    )
    assert "marketplace.toml" in result.stderr and "grimoire.toml" not in result.stderr
    assert "a bundle that lists another version still conflicts" in result.stderr
    assert lock.read_text() == written


def test_a_first_write_bundle_conflict_names_the_bundles_to_drop(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """Two bundles of one plugin list `x` at two versions, with no part to hold yet.

    The first write's 78 told the reader to declare `x` in `grimoire.toml`, a file a
    marketplace repository does not have; a later text told the reader to include `x`
    in the plugin, which a bundle that lists the other version still defeats. With both
    sides bundles no include settles it: the text names the bundles, and dropping one
    from the manifest lets the update through.
    """
    x1 = _skill(unique_repo, "x", "v1")
    x2 = make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    p1 = make_bundle(f"{unique_repo}/p1", [("skill", "x", x1.fq)], tag="1")
    p2 = make_bundle(f"{unique_repo}/p2", [("skill", "x", x2.fq)], tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [p1.fq, p2.fq])

    result = runner.run("update", "--marketplace", "marketplace.toml", check=False)

    assert result.returncode == 78, result.stderr
    assert (
        "drop one of these bundles from plugin 'team', or include versions of them "
        "that agree" in result.stderr
    ), result.stderr
    assert "directly in plugin" not in result.stderr, result.stderr
    assert "grimoire.toml" not in result.stderr, result.stderr
    assert not (work / "marketplace.lock").exists()

    _write_marketplace(work, [p1.fq])
    dropped = runner.run("update", "--marketplace", "marketplace.toml", check=False)
    assert dropped.returncode == 0, dropped.stderr


@pytest.mark.parametrize("depth", [2, 3])
def test_a_pick_refuses_a_dependent_beside_another_version_of_a_bundle_its_bundle_requirement_nests(
    grim_at, work: Path, registry: str, unique_repo: str, depth: int
) -> None:
    """`y:1` requires `outer:1`, which nests `req:1` now, and the pick moves `req` to `req:2`.

    The part is as c5e3824 wrote it, when `outer:1` listed `x:1` flat. `outer:1` nests
    `req:1` since, at `depth` 3 through `mid:1`; `outer:2` nests `req:2`; `y:1` moves onto a
    manifest requiring `outer:2`; `pack` nests `req:2`. A pick of `pack` or `req` took `x:2`
    beside `y` at its pin and exited 0. It refuses with 78 now, naming `req:1` through
    `outer:1`, and writes nothing; a pick of `extra` passes.
    """
    x1 = _skill(unique_repo, "x")
    x2 = make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    outer1 = make_bundle(f"{unique_repo}/outer", [("skill", "x", x1.fq)], tag="1")
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", _files("y"), [("bundle", "outer", outer1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, pk2.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/outer", "keep", outer1.digest)
    if depth == 3:
        mid1 = make_bundle(f"{unique_repo}/mid", [("bundle", "req", req1.fq)], tag="1")
        mid2 = make_bundle(f"{unique_repo}/mid", [("bundle", "req", req2.fq)], tag="2")
        make_bundle(f"{unique_repo}/outer", [("bundle", "mid", mid1.fq)], tag="1")
        outer2 = make_bundle(
            f"{unique_repo}/outer", [("bundle", "mid", mid2.fq)], tag="2"
        )
    else:
        make_bundle(f"{unique_repo}/outer", [("bundle", "req", req1.fq)], tag="1")
        outer2 = make_bundle(
            f"{unique_repo}/outer", [("bundle", "req", req2.fq)], tag="2"
        )
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y",
        "1",
        "skill",
        _files("y", "v2"),
        [("bundle", "outer", outer2.fq)],
    )
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    members = [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)]
    make_bundle(f"{unique_repo}/pack", members, tag="1")
    lock = work / "marketplace.lock"
    written = lock.read_text()

    for pick in ("team:pack", "team:req"):
        result = runner.run(
            "update", "--marketplace", "marketplace.toml", pick, check=False
        )

        assert result.returncode == 78, (pick, result.stderr)
        assert "skill 'y'" in result.stderr, result.stderr
        assert "requires req:1 through outer:1" in result.stderr, result.stderr
        assert "bundle 'req' to req:2" in result.stderr, result.stderr
        assert lock.read_text() == written
    extra_pick = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:extra", check=False
    )
    assert extra_pick.returncode == 0, extra_pick.stderr


@pytest.mark.parametrize("stale", [True, False], ids=["req-moves", "req-answers"])
def test_a_pick_judges_every_binding_a_nested_list_names_one_bundle_under(
    grim_at, work: Path, registry: str, unique_repo: str, stale: bool
) -> None:
    """`outer:1` lists one bundle as `a` and as `req`, and the pick moves `req`.

    The part is as c5e3824 wrote it, when `outer:1` listed `x:1` flat. With `stale`,
    `outer:1` lists `req:1` under both bindings since, and `y:1` moves onto a manifest
    requiring `outer:2`: a pick of `pack` or `req` took `x:2` beside `y` at its pin and
    exited 0, as only `a`, which sorts first, was judged. It refuses with 78 now and
    writes nothing. Where both bindings name the `req:2` the pick moves to, it passes.
    """
    x1 = _skill(unique_repo, "x")
    x2 = make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    outer1 = make_bundle(f"{unique_repo}/outer", [("skill", "x", x1.fq)], tag="1")
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", _files("y"), [("bundle", "outer", outer1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, pk2.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/outer", "keep", outer1.digest)
    listed = req1 if stale else req2
    twice = [("bundle", "a", listed.fq), ("bundle", "req", listed.fq)]
    make_bundle(f"{unique_repo}/outer", twice, tag="1")
    if stale:
        both = [("bundle", "a", req2.fq), ("bundle", "req", req2.fq)]
        outer2 = make_bundle(f"{unique_repo}/outer", both, tag="2")
        retag(f"{unique_repo}/y", "keep", y.digest)
        make_dependent(
            f"{unique_repo}/y",
            "1",
            "skill",
            _files("y", "v2"),
            [("bundle", "outer", outer2.fq)],
        )
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    members = [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)]
    make_bundle(f"{unique_repo}/pack", members, tag="1")
    lock = work / "marketplace.lock"
    written = lock.read_text(encoding="utf-8")

    for pick in ("team:pack", "team:req"):
        lock.write_text(written, encoding="utf-8", newline="\n")
        result = runner.run(
            "update", "--marketplace", "marketplace.toml", pick, check=False
        )

        if not stale:
            assert result.returncode == 0, (pick, result.stderr)
            continue
        assert result.returncode == 78, (pick, result.stderr)
        assert "requires req:1 through outer:1" in result.stderr, result.stderr
        assert "bundle 'req' to req:2" in result.stderr, result.stderr
        assert lock.read_text(encoding="utf-8") == written


# A requires row, as c5e3824 writes it and grim 0.15.1 never did.
_REQUIRES_ROWS = re.compile(
    r"(?m)^\[\[(?:skill|rule|agent|mcp)\.requires\]\]\n(?:[^\n\[][^\n]*\n)*\n?"
)


def _as_0_15_1_wrote(work: Path) -> None:
    """Leave the part as grim 0.15.1 wrote it: no digest, no tag and no requires row."""
    _as_c5e3824_wrote(work)
    lock = work / "marketplace.lock"
    lock.write_text(_REQUIRES_ROWS.sub("", lock.read_text()))


def _gone_requirement(
    grim_at, work: Path, unique_repo: str, shape: str
) -> tuple[GrimRunner, str]:
    """MK-F1 with only the `held:1` tag deleted from the registry; its manifest stays by digest."""
    runner, held1 = _mk_f1(grim_at, work, unique_repo)
    if shape == "0.15.1":
        _as_0_15_1_wrote(work)
    retag(f"{unique_repo}/held", "keep-first", held1.digest)
    _delete(f"{unique_repo}/held", "1")
    return runner, (work / "marketplace.lock").read_text()


@pytest.mark.parametrize(
    ("shape", "expected"), [("c5e3824", 78), ("0.15.1", 0)], ids=["c5e3824", "0.15.1"]
)
@pytest.mark.parametrize("pick", ["team:held", "team:pack"])
def test_a_gone_requirement_tag_refuses_a_pick_only_on_recorded_evidence(
    grim_at,
    work: Path,
    registry: str,
    unique_repo: str,
    shape: str,
    expected: int,
    pick: str,
) -> None:
    """`y` requires `held:1`, the pick moves `held` to `held:2`, and the `held:1` tag is gone.

    A part c5e3824 wrote records `y`'s requires row, which shows the pin the pick moves
    answered that edge: 78 with no tag read, the part untouched. A part grim 0.15.1 wrote
    records no edge, so only the tag could prove the mismatch; a tag that fails to read
    refuses nothing, and the pick exits as it did before this change.
    """
    runner, written = _gone_requirement(grim_at, work, unique_repo, shape)

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", pick, check=False
    )

    assert result.returncode == expected, result.stderr
    if expected:
        assert "requires" in result.stderr, result.stderr
        assert (work / "marketplace.lock").read_text() == written


@pytest.mark.parametrize("pick", ["team:pack", "team:req"])
def test_a_gone_bundle_requirement_tag_refuses_a_pick_on_the_parts_stamps(
    grim_at, work: Path, registry: str, unique_repo: str, pick: str
) -> None:
    """`y:1` requires the bundle `req:1`, the pick moves `req` to `req:2`, and `req:1` is gone.

    A part c5e3824 wrote stamps `x` through `req:1` at the pin the pick moves, so the pick
    refuses with 78 and the part is untouched. It exited 0, as no tag could be read to
    prove the mismatch. A part grim 0.15.1 wrote stamps none, so a gone tag refuses nothing
    there, and the unit tests cover that shape.
    """
    x1 = _skill(unique_repo, "x")
    x2 = make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", _files("y"), [("bundle", "req", req1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    runner = grim_at(work)
    _write_marketplace(work, [pack.fq, pk2.fq, extra.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    members = [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)]
    make_bundle(f"{unique_repo}/pack", members, tag="1")
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y",
        "1",
        "skill",
        _files("y", "v2"),
        [("bundle", "req", req2.fq)],
    )
    retag(f"{unique_repo}/req", "keep-first", req1.digest)
    _delete(f"{unique_repo}/req", "1")
    lock = work / "marketplace.lock"
    written = lock.read_text()

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", pick, check=False
    )

    assert result.returncode == 78, result.stderr
    assert "requires req:1" in result.stderr, result.stderr
    assert "bundle 'req' to req:2" in result.stderr, result.stderr
    assert lock.read_text() == written


@pytest.mark.parametrize(
    ("shape", "expected"), [("c5e3824", 78), ("0.15.1", 0)], ids=["c5e3824", "0.15.1"]
)
def test_an_offline_requirement_tag_miss_refuses_a_pick_only_on_recorded_evidence(
    tmp_path: Path,
    grim_at,
    grim_binary: Path,
    work: Path,
    registry: str,
    unique_repo: str,
    shape: str,
    expected: int,
) -> None:
    """The cache-miss twin of the gone tag: this machine never cached `held:1`.

    A helper's whole update warms a home with the rolled tags, `held:1` not among them.
    """
    _mk_f1(grim_at, work, unique_repo)
    if shape == "0.15.1":
        _as_0_15_1_wrote(work)
    helper = tmp_path / "helper"
    helper.mkdir()
    (helper / "marketplace.toml").write_text((work / "marketplace.toml").read_text())
    warm = GrimRunner(grim_binary, tmp_path / "warm" / "grim-home", cwd=helper)
    assert warm.run("update", "--marketplace", "marketplace.toml").returncode == 0
    written = (work / "marketplace.lock").read_text()
    reader = GrimRunner(grim_binary, tmp_path / "warm" / "grim-home", cwd=work)

    result = reader.run(
        "--offline",
        "update",
        "--marketplace",
        "marketplace.toml",
        "team:held",
        check=False,
    )

    assert result.returncode == expected, result.stderr
    if expected:
        assert (work / "marketplace.lock").read_text() == written


@pytest.mark.parametrize("written", ["c5e3824", "0.15.1"])
def test_a_pick_holds_the_requirement_of_a_row_it_holds(
    grim_at, work: Path, registry: str, unique_repo: str, written: str
) -> None:
    """`pack` drops `held`, and `y`, which the pick holds, still requires it.

    `y` was republished without a requires layer, so the pick keeps its old row. That row
    requires `held`, so `held` stays at its pin with its tag and stamps, as the previous
    part holds it; the pick refuses nothing and the part keeps a requirement its dependent
    needs. A part grim 0.15.1 wrote records no `requires` row, so the requirement shows
    only in `y`'s manifest at the pin the part holds.
    """
    runner, _held1 = _mk_f1(grim_at, work, unique_repo)
    if written == "0.15.1":
        _as_0_15_1_wrote(work)
    before = _rows(work)
    leaving = _skill(unique_repo, "leaving")
    retag(f"{unique_repo}/pack", "keep-second", tag_digest(f"{unique_repo}/pack", "1"))
    make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    retag(f"{unique_repo}/y", "keep-third", tag_digest(f"{unique_repo}/y", "1"))
    make_artifact(f"{unique_repo}/y", "skill", _files("y", "v3"), tag="1")

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:pack", check=False
    )

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert {"held", "y"} <= set(after), set(after)
    assert after["held"] == before["held"], after["held"]
    assert after["y"] == before["y"], after["y"]


@pytest.mark.parametrize(
    ("online", "exit_code"), [(True, 79), (False, 81)], ids=["online", "offline"]
)
def test_an_unreadable_recorded_nested_bundle_stops_the_export(
    tmp_path: Path,
    grim_binary: Path,
    registry: str,
    unique_repo: str,
    online: bool,
    exit_code: int,
) -> None:
    """`inner` records its digest and `outer`, which nests it now, records none.

    The export reads `inner` at its recorded digest, so a reader that cannot get that
    manifest has no list to read: online the registry dropped it and the pin is gone, 79;
    offline the store lacks it, 81. Neither follows the tag, and the part is untouched.
    """
    x = _skill(unique_repo, "x")
    y = _skill(unique_repo, "y")
    z = _skill(unique_repo, "z")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "x", x.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    header = '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'

    def manifest(directory: Path, includes: list[str]) -> None:
        directory.mkdir(exist_ok=True)
        (directory / "marketplace.toml").write_text(
            f"{header}[plugins.team]\ninclude = {json.dumps(includes)}\n"
        )

    origin = tmp_path / "origin"
    manifest(origin, [outer.fq, inner.fq])
    writer = GrimRunner(grim_binary, tmp_path / "writer" / "grim-home", cwd=origin)
    writer.run("update", "--marketplace", "marketplace.toml")
    forgotten = (
        (origin / "marketplace.lock").read_text().replace(f"@{outer.digest}", "")
    )
    make_bundle(f"{unique_repo}/inner", [("skill", "z", z.fq)], tag="1")
    retag(f"{unique_repo}/outer", "keep", outer.digest)
    members = [("skill", "y", y.fq), ("bundle", "inner", inner.fq)]
    make_bundle(f"{unique_repo}/outer", members, tag="1")

    home = tmp_path / "reader" / "grim-home"
    if not online:
        warm = tmp_path / "warm"
        manifest(warm, [outer.fq])
        warmer = GrimRunner(grim_binary, home, cwd=warm)
        assert warmer.run("update", "--marketplace", "marketplace.toml").returncode == 0
    consumer = tmp_path / "consumer"
    manifest(consumer, [outer.fq])
    lock = consumer / "marketplace.lock"
    lock.write_text(forgotten)
    before = lock.read_bytes()

    result = GrimRunner(grim_binary, home, cwd=consumer).run(
        *([] if online else ["--offline"]),
        "export",
        "plugin",
        "--client",
        "claude",
        "-o",
        "dist",
        "--force",
        check=False,
    )

    assert result.returncode == exit_code, result.stderr
    assert lock.read_bytes() == before


def test_a_pick_reads_a_recorded_nested_bundle_below_an_unrecorded_parent_at_its_recorded_list(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """`outer` nests `inner`; the part records `inner`'s digest and `outer`'s none.

    `inner:1` moved onto a list naming `x:1` while the plugin includes `x:2` itself. A whole
    update selects `inner`, reads the moved list and exits 78. A pick of `m` selects
    neither bundle: `inner` keeps its recorded list, so the conflict never arises and `x`
    stays at `x:2`. The pick read the moved list and exited 78.
    """
    x1 = _skill(unique_repo, "x")
    x2 = make_artifact(f"{unique_repo}/x", "skill", _files("x", "v2"), tag="2")
    m = _skill(unique_repo, "m")
    y = _skill(unique_repo, "y")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "m", m.fq)], tag="1")
    members = [("skill", "y", y.fq), ("bundle", "inner", inner.fq)]
    outer = make_bundle(f"{unique_repo}/outer", members, tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [outer.fq, x2.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    lock = work / "marketplace.lock"
    lock.write_text(lock.read_text().replace(f"@{outer.digest}", ""))
    written = lock.read_text()
    retag(f"{unique_repo}/inner", "keep", inner.digest)
    make_bundle(
        f"{unique_repo}/inner", [("skill", "m", m.fq), ("skill", "x", x1.fq)], tag="1"
    )

    whole = runner.run(
        "update", "--marketplace", "marketplace.toml", "team", check=False
    )
    assert whole.returncode == 78, whole.stderr
    assert lock.read_text() == written

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:m", check=False
    )

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert set(rows) == {"m", "x", "y"}, set(rows)
    assert rows["x"]["pinned"].endswith(x2.digest), rows["x"]
    assert f"1@{inner.digest}" in _stamp_tags(rows["m"]), rows["m"]


def test_a_released_stamp_carried_bundle_follows_the_project_route(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """Preservation control, green on the base: the part follows the project route.

    The part holds `inner:1` at its first digest, and `outer` lists `inner:1` while
    `bridge` lists `inner:2`; both tags serve one new manifest. A project's carried
    `[[bundle]]` pin meeting a second tag it does not answer is released and both tags
    resolve afresh; the part's stamp meets the diamond the same way, so the rows follow
    the new list and every stamp records the new digest.
    """
    x = _skill(unique_repo, "x")
    y = _skill(unique_repo, "y")
    w = _skill(unique_repo, "w")
    z = _skill(unique_repo, "z")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    bridge = make_bundle(f"{unique_repo}/bridge", [("skill", "w", w.fq)], tag="1")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "x", x.fq)], tag="1")
    runner = grim_at(work)
    _write_marketplace(work, [outer.fq, bridge.fq, inner.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    _as_c5e3824_wrote(work)
    pick = runner.run("update", "--marketplace", "marketplace.toml", "team:inner")
    assert pick.returncode == 0, pick.stderr
    for name, old in (("outer", outer), ("bridge", bridge), ("inner", inner)):
        retag(f"{unique_repo}/{name}", "keep", old.digest)
    make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    second = inner.fq.rsplit(":", 1)[0] + ":2"
    make_bundle(f"{unique_repo}/bridge", [("bundle", "inner", second)], tag="1")
    moved = make_bundle(f"{unique_repo}/inner", [("skill", "z", z.fq)], tag="1")
    make_bundle(f"{unique_repo}/inner", [("skill", "z", z.fq)], tag="2")
    _write_marketplace(work, [outer.fq, bridge.fq])

    result = _export(runner, "--force")

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert set(rows) == {"z"}, set(rows)
    stamps = _stamps_of(rows["z"], "inner")
    assert stamps and stamps <= {f"1@{moved.digest}", f"2@{moved.digest}"}, rows["z"]


@pytest.mark.parametrize("written", ["c5e3824", "0.15.1"])
@pytest.mark.parametrize("agree", [True, False], ids=["tag-agrees", "tag-stays"])
def test_a_pick_of_a_row_a_direct_include_shadows_refuses_no_dependent(
    grim_at, work: Path, registry: str, unique_repo: str, written: str, agree: bool
) -> None:
    """`team` = [`y:1`, `held:2`], and `y:1` requires `held:1`: the include shadows the edge.

    `held:2` is republished, and `held:1` onto the same digest when `agree`. The pick of
    `held` moves the include, which the graph accepts; `y` stays beside it as the
    declaration put it. The check read the shadowed edge as one the old pin answered and
    refused with 78 without reading the tag.
    """
    held1 = _skill(unique_repo, "held")
    held2 = make_artifact(f"{unique_repo}/held", "skill", _files("held", "v2"), tag="2")
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", _files("y"), [("skill", "held", held1.fq)]
    )
    runner = grim_at(work)
    _write_marketplace(work, [y.fq, held2.fq])
    assert runner.run("update", "--marketplace", "marketplace.toml").returncode == 0
    if written == "0.15.1":
        _as_0_15_1_wrote(work)
    else:
        _as_c5e3824_wrote(work)
    retag(f"{unique_repo}/held", "keep", held2.digest)
    moved = make_artifact(f"{unique_repo}/held", "skill", _files("held", "v3"), tag="2")
    if agree:
        retag(f"{unique_repo}/held", "1", moved.digest)
    before = _rows(work)

    result = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:held", check=False
    )

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert after["held"]["pinned"].endswith(moved.digest), after["held"]
    assert after["y"] == before["y"], after["y"]


def test_a_restored_requirement_records_one_digest_for_the_bundle_that_dropped_it(
    grim_at, work: Path, registry: str, unique_repo: str
) -> None:
    """A pick of `pack` records its digest; `pack` then drops `held`, which `y` still requires.

    The second pick restores `held` for `y` with the stamp the first pick wrote, while
    `leaving` takes the new digest: two digests for one pair, which the next read drops.
    The restored row keeps its stamp without the digest this run replaced.
    """
    runner, _held1 = _mk_f1(grim_at, work, unique_repo, roll=False)
    first = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:pack", check=False
    )
    assert first.returncode == 0, first.stderr
    assert any("@" in tag for tag in _stamps_of(_rows(work)["held"], "pack"))
    leaving = _skill(unique_repo, "leaving")
    retag(f"{unique_repo}/pack", "keep-second", tag_digest(f"{unique_repo}/pack", "1"))
    make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    retag(f"{unique_repo}/y", "keep-third", tag_digest(f"{unique_repo}/y", "1"))
    make_artifact(f"{unique_repo}/y", "skill", _files("y", "v3"), tag="1")

    second = runner.run(
        "update", "--marketplace", "marketplace.toml", "team:pack", check=False
    )

    assert second.returncode == 0, second.stderr
    rows = _rows(work)
    assert "held" in rows, set(rows)
    digests = {
        tag
        for row in rows.values()
        for tag in _stamps_of(row, "pack")
        if "@" in tag
    }
    assert len(digests) == 1, {name: _stamps_of(row, "pack") for name, row in rows.items()}
