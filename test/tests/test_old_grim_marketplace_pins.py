# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""How grim 0.15.1 and the current grim share a `marketplace.lock`.

A part keeps a bundle's pin in the existing stamp, `bundle_tag = "<tag>@<digest>"`,
with no new field and `lock_version` still 1. 0.15.1 holds that string opaque: it
reads it on every path, a member update writes it back, and a write that
re-resolves replaces it with the plain tag. With no digest to honour, a current
write that selects the bundle follows its tag once, says so, and records the digest
it found; a member update keeps every row outside its selector as the part holds it.

The old binary comes from `GRIM_OLD_BIN`, a build of tag `v0.15.1`, and the module
skips when it is unset. `GRIM_COMMAND` is the current grim. Each side keeps its own
`GRIM_HOME`, so the old grim reaches a pin through the registry rather than a store
the current grim filled.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, make_dependent
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import PublishedArtifact, retag
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

_MANIFEST = "marketplace.toml"
_FOLLOWS = "plugin 'team' follows the tag of bundle 'pack'"


@dataclass(slots=True)
class _Published:
    """`pack:1` at (`held:1`, `leaving`); a roll moves it to (`held:2`, `arriving`)."""

    repo: str
    pack: PublishedArtifact
    held: PublishedArtifact
    held2: PublishedArtifact
    leaving: PublishedArtifact
    arriving: PublishedArtifact
    extra: PublishedArtifact
    fresh: PublishedArtifact

    def includes(self) -> list[str]:
        return [self.pack.fq, self.extra.fq]

    def roll(self) -> PublishedArtifact:
        """Move `pack:1`; `keep` stops zot dropping the manifest it leaves."""
        retag(f"{self.repo}/pack", "keep", self.pack.digest)
        members = [
            ("skill", "held", self.held2.fq),
            ("skill", "arriving", self.arriving.fq),
        ]
        moved = make_bundle(f"{self.repo}/pack", members, tag="1")
        assert moved.digest != self.pack.digest, "a moved tag must change the digest"
        return moved


def _skill(repo: str, name: str, body: str = "v1", tag: str = "1") -> PublishedArtifact:
    return make_artifact(
        f"{repo}/{name}",
        "skill",
        {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {body}\n"},
        tag=tag,
    )


def _publish(repo: str) -> _Published:
    held = _skill(repo, "held")
    leaving = _skill(repo, "leaving")
    return _Published(
        repo=repo,
        pack=make_bundle(
            f"{repo}/pack",
            [("skill", "held", held.fq), ("skill", "leaving", leaving.fq)],
            tag="1",
        ),
        held=held,
        held2=_skill(repo, "held", "v2", tag="2"),
        leaving=leaving,
        arriving=_skill(repo, "arriving"),
        extra=_skill(repo, "extra"),
        fresh=_skill(repo, "fresh"),
    )


def _write_manifest(work: Path, includes: list[str]) -> None:
    (work / _MANIFEST).write_text(
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f"[plugins.team]\ninclude = {json.dumps(includes)}\n",
        encoding="utf-8",
    )


def _lock_text(work: Path) -> str:
    return (work / "marketplace.lock").read_text(encoding="utf-8")


def _rows(work: Path) -> dict[str, dict]:
    return {
        row["name"]: row for row in tomllib.loads(_lock_text(work)).get("skill", [])
    }


def _stamps(work: Path) -> dict[str, str | None]:
    """`name -> bundle_tag` of the rows a bundle stamped."""
    rows = _rows(work)
    return {name: rows[name].get("bundle_tag") for name in rows if name != "extra"}


def _digests(work: Path) -> dict[str, str]:
    return {name: row["pinned"].split("@")[1] for name, row in _rows(work).items()}


def _update(runner: GrimRunner, *selectors: str):
    return runner.run("update", "--marketplace", _MANIFEST, *selectors, check=False)


def _export_plugin(runner: GrimRunner):
    return runner.run(
        "export",
        "plugin",
        "--plugin",
        "team",
        "--client",
        "claude",
        "-o",
        "dist",
        "--force",
        check=False,
    )


def _held_body(work: Path) -> str:
    bodies = (work / "dist").rglob("held/SKILL.md")
    return next(bodies).read_text(encoding="utf-8").split()[-1]


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    directory = tmp_path / "work"
    directory.mkdir()
    return directory


@pytest.fixture()
def old(old_bin: Path, tmp_path: Path, work: Path) -> GrimRunner:
    """The old grim in the marketplace repository, with its own cold `GRIM_HOME`."""
    grim_home = tmp_path / "old" / "grim-home"
    grim_home.mkdir(parents=True)
    return GrimRunner(old_bin, grim_home, cwd=work)


@pytest.fixture()
def current(grim_at, work: Path, registry: str) -> GrimRunner:
    return grim_at(work)


@pytest.fixture()
def published(unique_repo: str, work: Path) -> _Published:
    state = _publish(unique_repo)
    _write_manifest(work, state.includes())
    return state


def _locked_by_current(current: GrimRunner, work: Path, state: _Published) -> str:
    """The first write; returns the stamp it recorded on `held` and `leaving`."""
    assert _update(current).returncode == 0
    stamp = f"1@{state.pack.digest}"
    assert _stamps(work) == {"held": stamp, "leaving": stamp}
    assert tomllib.loads(_lock_text(work))["metadata"]["lock_version"] == 1
    return stamp


def _locked_by_old(old: GrimRunner, work: Path) -> None:
    """The first write, as 0.15.1 leaves it: plain stamps and tagless pins."""
    assert _update(old).returncode == 0
    assert _stamps(work) == {"held": "1", "leaving": "1"}
    assert all(":1@" not in row["pinned"] for row in _rows(work).values())


@pytest.mark.parametrize("command", ["export plugin", "export marketplace"])
def test_old_grim_reads_a_recorded_digest_and_leaves_the_lock_alone(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    command: str,
) -> None:
    _locked_by_current(current, work, published)
    published.roll()
    before = _lock_text(work)

    if command == "export plugin":
        result = _export_plugin(old)
    else:
        result = old.run("export", "marketplace", check=False)

    assert result.returncode == 0, result.stderr
    assert _lock_text(work) == before
    if command == "export plugin":
        assert _held_body(work) == "v1", "0.15.1 exports the pinned bytes"


def test_old_grim_member_update_writes_the_digest_back_for_the_current_grim_to_keep(
    old: GrimRunner, current: GrimRunner, work: Path, published: _Published
) -> None:
    stamp = _locked_by_current(current, work, published)
    digests = _digests(work)
    published.roll()

    by_old = _update(old, "team:extra")

    assert by_old.returncode == 0, by_old.stderr
    assert _stamps(work) == {"held": stamp, "leaving": stamp}
    assert _digests(work) == digests
    assert all(":1@" not in row["pinned"] for row in _rows(work).values())

    by_current = _update(current, "team:extra")

    assert by_current.returncode == 0, by_current.stderr
    assert "follows the tag" not in by_current.stderr
    assert _stamps(work) == {"held": stamp, "leaving": stamp}
    assert _digests(work) == digests


def test_old_grim_re_resolving_a_member_keeps_the_stamps_it_did_not_touch(
    old: GrimRunner, current: GrimRunner, work: Path, published: _Published
) -> None:
    """Each member update keeps the rows outside its selector as the part holds them."""
    stamp = _locked_by_current(current, work, published)
    digests = _digests(work)

    by_old = _update(old, "team:held")

    assert by_old.returncode == 0, by_old.stderr
    assert _stamps(work)["leaving"] == stamp
    assert _digests(work) == digests
    left = _rows(work)

    by_current = _update(current, "team:extra")

    assert by_current.returncode == 0, by_current.stderr
    after = _rows(work)
    assert {n: after[n] for n in ("held", "leaving")} == {
        n: left[n] for n in ("held", "leaving")
    }


@pytest.mark.parametrize("write", ["whole update", "stale export"])
def test_old_grim_re_resolving_write_drops_the_digest_and_the_next_whole_write_records_one(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    write: str,
) -> None:
    _locked_by_current(current, work, published)
    moved = published.roll()
    if write == "whole update":
        result = _update(old)
    else:
        _write_manifest(work, [*published.includes(), published.fresh.fq])
        result = _export_plugin(old)

    assert result.returncode == 0, result.stderr
    assert _stamps(work) | {"fresh": "1"} == {
        "held": "1",
        "arriving": "1",
        "fresh": "1",
    }

    before = _rows(work)
    by_member = _update(current, "team:extra")

    assert by_member.returncode == 0, by_member.stderr
    after = _rows(work)
    assert {n: after[n] for n in after if n != "extra"} == {
        n: before[n] for n in before if n != "extra"
    }

    by_plugin = _update(current, "team")

    assert by_plugin.returncode == 0, by_plugin.stderr
    assert "follows the tag" not in by_plugin.stderr, (
        "an update warns of no tag it read"
    )
    stamp = f"1@{moved.digest}"
    assert _stamps(work).get("held") == stamp
    assert _stamps(work).get("arriving") == stamp


@pytest.mark.parametrize("write", ["stale export", "whole update"])
def test_the_first_write_over_an_old_lock_follows_the_tag_and_records_it(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    write: str,
) -> None:
    _locked_by_old(old, work)
    moved = published.roll()

    if write == "whole update":
        result = _update(current)
    else:
        _write_manifest(work, [*published.includes(), published.fresh.fq])
        result = _export_plugin(current)

    assert result.returncode == 0, result.stderr
    stamp = f"1@{moved.digest}"
    assert _stamps(work).get("held") == stamp
    assert _stamps(work).get("arriving") == stamp
    assert "leaving" not in _rows(work)
    assert _digests(work)["held"] == published.held2.digest
    assert (_FOLLOWS in result.stderr) == (write != "whole update"), result.stderr


def test_a_member_update_over_an_old_lock_keeps_every_row_outside_its_selector(
    old: GrimRunner, current: GrimRunner, work: Path, published: _Published
) -> None:
    """The bundle's tag moved, yet the pick names `extra` alone.

    Following the tag wrote `held:2` beside `held:1`'s digest, a reference that never
    resolved it, dropped `leaving` and added `arriving`. The old grim's own member
    update keeps those rows, and so does the current one, run after run.
    """
    _locked_by_old(old, work)
    before = _rows(work)
    published.roll()

    result = _update(current, "team:extra")

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert {name: after[name] for name in after if name != "extra"} == {
        name: before[name] for name in before if name != "extra"
    }
    assert after["extra"]["pinned"].endswith(f"/extra:1@{published.extra.digest}")
    assert _FOLLOWS not in result.stderr, result.stderr
    written = _lock_text(work)

    again = _update(current, "team:extra")

    assert again.returncode == 0, again.stderr
    assert _lock_text(work) == written


def test_a_member_update_writes_no_false_pair_over_a_member_the_old_grim_moved(
    old: GrimRunner, current: GrimRunner, work: Path, published: _Published
) -> None:
    """The old grim moved `held` to `held:2` under a stamp still recording `pack:1`.

    That stamp's bundle lists `held:1`, so reading the pin through it wrote
    `held:1` beside `held:2`'s digest. The row stays as the old grim left it, with
    no tag, and the run names the command that records one.
    """
    _locked_by_current(current, work, published)
    published.roll()
    assert _update(old, "team:held").returncode == 0
    left = _rows(work)
    assert left["held"]["pinned"].endswith(f"/held@{published.held2.digest}")

    result = _update(current, "team:extra")

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert after["held"] == left["held"]
    assert after["leaving"] == left["leaving"]
    assert "plugin 'team' kept skill 'held'" in result.stderr, result.stderr
    assert "update --marketplace" in result.stderr, result.stderr


def test_a_member_pick_of_a_row_the_moved_bundle_let_go_matches_nothing(
    old: GrimRunner, current: GrimRunner, work: Path, published: _Published
) -> None:
    """The bundle's moved tag no longer lists `leaving`, so a pick naming it exits 79.

    The current grim dropped the named row at exit 0 and reported nothing about it.
    Both grims leave the lock as it was.
    """
    _locked_by_old(old, work)
    published.roll()
    before = _lock_text(work)

    for runner in (old, current):
        result = _update(runner, "team:leaving")

        assert result.returncode == 79, result.stderr
        assert "team:leaving" in result.stderr, result.stderr
        assert _lock_text(work) == before


def test_an_offline_export_over_an_old_part_ships_no_include_deleted_by_hand(
    old_bin: Path, current: GrimRunner, tmp_path: Path, unique_repo: str
) -> None:
    """An include deleted by hand, then an offline export of the part 0.15.1 wrote.

    0.15.1's `update --marketplace` records no edge and stores manifests without
    their layers, and an export never fetches a rule, so the store keeps lacking the
    rule `p`'s requires layer. Found by review: the current offline export kept `b`
    for `p`, shipped it, and wrote the part under the plugin's own hash, so every
    later export shipped it too. It now drops `b`, and both grims read the part as
    fresh. 0.15.1 cannot run that offline export and exits 81.
    """
    a = _skill(unique_repo, "a")
    b = _skill(unique_repo, "b")
    p = make_dependent(
        f"{unique_repo}/p", "1", "rule", {"p.md": "# p\n"}, [("skill", "a", a.fq)]
    )

    def sequence(binary: Path, name: str) -> tuple[int, GrimRunner, GrimRunner]:
        work, home = tmp_path / name / "work", tmp_path / name / "grim-home"
        work.mkdir(parents=True)
        home.mkdir()
        _write_manifest(work, [a.fq, b.fq, p.fq])
        writer = GrimRunner(old_bin, home, cwd=work)
        assert _update(writer).returncode == 0
        runner = GrimRunner(binary, home, cwd=work)
        assert _export_plugin(runner).returncode == 0
        _write_manifest(work, [a.fq, p.fq])
        args = ("export", "plugin", "--plugin", "team", "--client", "claude")
        offline = runner.run(*args, "-o", "dist", "--force", "--offline", check=False)
        return offline.returncode, runner, writer

    old_exit, _, _ = sequence(old_bin, "old")
    current_exit, runner, reader = sequence(current.binary, "current")
    work = runner.cwd
    assert work is not None

    def shipped() -> set[str]:
        return {path.parent.name for path in (work / "dist").rglob("SKILL.md")}

    assert (old_exit, current_exit) == (81, 0)
    assert set(_rows(work)) == {"a"}, _lock_text(work)
    assert shipped() == {"a"}
    written = _lock_text(work)
    assert _export_plugin(runner).returncode == 0
    assert shipped() == {"a"}
    assert _lock_text(work) == written, "the part reads as fresh"
    assert _update(reader, "team:a").returncode == 0, "0.15.1 reads it as fresh too"


def test_a_member_update_writes_no_false_pair_below_a_bundle_its_member_requires(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
) -> None:
    """`p` requires `pack`, whose tag moves onto a list naming `held:2`.

    The part the old grim wrote records no digest for `pack`, so a pick of `p` reads
    `pack` at its tag, and nothing there attests `held`'s tagless pin. `held` follows
    the list, and no row pairs a tag with a digest that tag never served (CWE-345).
    """
    files = {"p/SKILL.md": "---\nname: p\ndescription: d\n---\n# p\n"}
    p = make_dependent(
        f"{unique_repo}/p", "1", "skill", files, [("bundle", "pack", published.pack.fq)]
    )
    _write_manifest(work, [*published.includes(), p.fq])
    assert _update(old).returncode == 0
    assert _rows(work)["held"]["pinned"].endswith(f"/held@{published.held.digest}")
    published.roll()

    result = _update(current, "team:p")

    assert result.returncode == 0, result.stderr
    held = _rows(work)["held"]["pinned"]
    assert held.endswith(f"/held:2@{published.held2.digest}"), (
        held,
        published.held.digest,
    )


@pytest.mark.parametrize("pick", ["team:pack", "team:pk2"])
def test_a_bundle_pick_over_an_old_part_refuses_a_conflict_with_a_sibling_bundle(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
    pick: str,
) -> None:
    """`pack` and `pk2` both list `held:1`, and the old grim's part records neither digest.

    `pack`'s tag moves onto `held:2`. A pick of either bundle left the other unread and
    settled the conflict at exit 0, writing `held` under one stamp while `pk2:1` still
    names `held:1`. Both grims refuse it with 78 and leave the part as it was, and so
    they do a pick of `extra`. Both exports keep the part as the old grim wrote it.
    """
    b = _skill(unique_repo, "b")
    members = [("skill", "held", published.held.fq), ("skill", "b", b.fq)]
    pk2 = make_bundle(f"{unique_repo}/pk2", members, tag="1")
    _write_manifest(work, [*published.includes(), pk2.fq])
    assert _update(old).returncode == 0
    published.roll()
    written = _lock_text(work)

    for runner in (old, current):
        result = _update(runner, pick)

        assert result.returncode == 78, result.stderr
        assert "skill 'held'" in result.stderr, result.stderr
        assert _lock_text(work) == written

    for runner in (old, current):
        by_member = _update(runner, "team:extra")

        assert by_member.returncode == 78, by_member.stderr
        assert _lock_text(work) == written

    exported = _export_plugin(current)

    assert exported.returncode == 0, exported.stderr
    assert _held_body(work) == "v1"
    marketplace = current.run(
        "export", "marketplace", "--marketplace", _MANIFEST, "-o", "distm", check=False
    )
    assert marketplace.returncode == 0, marketplace.stderr
    assert _lock_text(work) == written


def test_a_pick_over_an_old_part_refuses_a_sibling_whose_member_requires_the_row(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
) -> None:
    """`pk2` lists `y` and `b` but no `held`, and `y`'s manifest requires `held:1`.

    The old grim's part records no bundle digest and none of `y`'s edges. `pack`'s tag
    moves onto `held:2`. A pick of `pack` or of `held` left `pk2` unread and wrote
    `held:2` at exit 0, though `y` still requires `held:1`. Both picks now refuse the
    conflict with 78, as a whole update does, and leave the part as it was. The old grim
    matches no bundle selector and exits 79.
    """
    b = _skill(unique_repo, "b")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    requires = [("skill", "held", published.held.fq)]
    y = make_dependent(f"{unique_repo}/y", "1", "skill", files, requires)
    members = [("skill", "y", y.fq), ("skill", "b", b.fq)]
    pk2 = make_bundle(f"{unique_repo}/pk2", members, tag="1")
    _write_manifest(work, [*published.includes(), pk2.fq])
    assert _update(old).returncode == 0
    published.roll()
    written = _lock_text(work)

    assert _update(old, "team:pack").returncode == 79
    assert _lock_text(work) == written
    for pick in ("team:pack", "team:held"):
        result = _update(current, pick)

        assert result.returncode == 78, (pick, result.stderr)
        assert "skill 'held'" in result.stderr, result.stderr
        assert _lock_text(work) == written


def test_a_member_pick_beside_a_conflicting_sibling_refuses_the_conflict_first(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
) -> None:
    """`pack`'s tag moves onto `held:2` and drops `leaving`; `pk2` still lists `held:1`.

    Both grims meet the conflict between the bundles before the missing name, so a pick
    of `leaving` exits 78 and writes nothing. The current grim left `pk2` unread and
    exited 79 for the name instead.
    """
    b = _skill(unique_repo, "b")
    members = [("skill", "held", published.held.fq), ("skill", "b", b.fq)]
    pk2 = make_bundle(f"{unique_repo}/pk2", members, tag="1")
    _write_manifest(work, [*published.includes(), pk2.fq])
    assert _update(old).returncode == 0
    published.roll()
    written = _lock_text(work)

    for runner in (old, current):
        result = _update(runner, "team:leaving")

        assert result.returncode == 78, result.stderr
        assert "skill 'held'" in result.stderr, result.stderr
        assert _lock_text(work) == written


@pytest.mark.parametrize("nested", [False, True], ids=["conflict-only", "outer-nests"])
@pytest.mark.parametrize("pick", ["team:m", "team:y", "team:x"])
def test_a_pick_refuses_a_moved_bundle_list_that_conflicts_with_a_direct_include(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    unique_repo: str,
    pick: str,
    nested: bool,
) -> None:
    """`inner:1` moves onto a list naming `x:1` while the plugin includes `x:2` itself.

    The old grim's part records no digest for `inner`, and a pick of `m`, `y` or `x`
    selects neither bundle. A whole update and both grims' picks meet the conflict
    with 78 naming `x`; the current grim's pick exited 0 and left the conflict in the
    part. With `outer` also moved to nest `inner`, 0.15.1 refuses the nested bundle
    outright (65), so only the current grim is held to 78 there.
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    m = _skill(unique_repo, "m")
    y = _skill(unique_repo, "y")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "m", m.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    _write_manifest(work, [outer.fq, inner.fq, x2.fq])
    assert _update(old).returncode == 0
    retag(f"{unique_repo}/inner", "keep", inner.digest)
    make_bundle(
        f"{unique_repo}/inner", [("skill", "m", m.fq), ("skill", "x", x1.fq)], tag="1"
    )
    if nested:
        retag(f"{unique_repo}/outer", "keep", outer.digest)
        make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    written = _lock_text(work)

    runners = (current,) if nested else (old, current)
    for runner in runners:
        for selectors in ((), (pick,)):
            result = _update(runner, *selectors)

            assert result.returncode == 78, (selectors, result.stderr)
            assert "skill 'x'" in result.stderr, result.stderr
            assert _lock_text(work) == written


def test_the_old_grim_member_update_keeps_what_the_first_current_write_moves(
    old: GrimRunner, work: Path, published: _Published
) -> None:
    """The old grim's member update keeps every row outside its selector."""
    _locked_by_old(old, work)
    digests = _digests(work)
    published.roll()

    result = _update(old, "team:extra")

    assert result.returncode == 0, result.stderr
    assert _digests(work) == digests
    assert _stamps(work) == {"held": "1", "leaving": "1"}


def test_a_current_export_leaves_a_fresh_old_lock_alone(
    old: GrimRunner, current: GrimRunner, work: Path, published: _Published
) -> None:
    _locked_by_old(old, work)
    published.roll()
    before = _lock_text(work)

    result = _export_plugin(current)

    assert result.returncode == 0, result.stderr
    assert _lock_text(work) == before


def test_old_grim_reads_the_stamp_of_a_nested_only_bundle(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """A bundle no row carries stamps the rows below it, which 0.15.1 never wrote.

    0.15.1 refuses a nested bundle on any resolve, with or without a digest, so only
    its read path meets this form.
    """
    x = _skill(unique_repo, "x")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "x", x.fq)], tag="1")
    nests = make_bundle(
        f"{unique_repo}/nests", [("bundle", "inner", inner.fq)], tag="1"
    )
    _write_manifest(work, [nests.fq])
    assert _update(current).returncode == 0
    assert {stamp["tag"] for stamp in _rows(work)["x"]["bundles"]} == {
        f"1@{inner.digest}",
        f"1@{nests.digest}",
    }
    before = _lock_text(work)

    exported = _export_plugin(old)

    assert exported.returncode == 0, exported.stderr
    assert _lock_text(work) == before


def test_old_grim_writes_back_stamps_on_a_declared_row_and_in_the_bundles_array(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    x = _skill(unique_repo, "x")
    direct = _skill(unique_repo, "direct")
    extra = _skill(unique_repo, "extra")
    left = make_bundle(f"{unique_repo}/left", [("skill", "x", x.fq)], tag="1")
    right = make_bundle(f"{unique_repo}/right", [("skill", "x", x.fq)], tag="1")
    lists = make_bundle(
        f"{unique_repo}/lists", [("skill", "direct", direct.fq)], tag="1"
    )
    _write_manifest(work, [left.fq, right.fq, lists.fq, direct.fq, extra.fq])
    assert _update(current).returncode == 0
    both = {f"1@{left.digest}", f"1@{right.digest}"}
    declared = f"1@{lists.digest}"
    rows = _rows(work)
    assert rows["direct"]["bundle_tag"] == declared
    assert {stamp["tag"] for stamp in rows["x"]["bundles"]} == both

    updated = _update(old, "team:extra")

    assert updated.returncode == 0, updated.stderr
    rows = _rows(work)
    assert rows["direct"]["bundle_tag"] == declared
    assert {stamp["tag"] for stamp in rows["x"]["bundles"]} == both


_STAMP_FIELDS = ("bundle", "bundle_tag", "bundles")


def _stamp_fields(row: dict) -> dict:
    return {field: row.get(field) for field in _STAMP_FIELDS}


@pytest.mark.parametrize("pick", ["x", "y"])
def test_a_member_pick_records_no_digest_for_a_bundle_it_did_not_select(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str, pick: str
) -> None:
    """`pack` lists only `x`, which the plugin also includes, so no row carries its stamp.

    The old grim's part records no digest for `pack`. A pick of `x`, or of `y` after
    `pack`'s tag moves onto a list adding it, recorded `pack`'s digest on the row it
    took, though it neither selected `pack` nor found it recorded, and later picks
    carried that pin. Both grims now stamp that row alike and keep every other row.
    """
    x = _skill(unique_repo, "x")
    extra = _skill(unique_repo, "extra")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", x.fq)], tag="1")
    _write_manifest(work, [pack.fq, x.fq, extra.fq])
    assert _update(old).returncode == 0
    picked = x
    if pick == "y":
        picked = _skill(unique_repo, "y")
        make_bundle(
            f"{unique_repo}/pack",
            [("skill", "x", x.fq), ("skill", "y", picked.fq)],
            tag="1",
        )
    written = _lock_text(work)
    before = _rows(work)

    by_old = _update(old, f"team:{pick}")
    assert by_old.returncode == 0, by_old.stderr
    as_old = _rows(work)
    (work / "marketplace.lock").write_text(written, encoding="utf-8")

    result = _update(current, f"team:{pick}")

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert "@" not in "".join(str(_stamp_fields(row)) for row in after.values())
    assert _stamp_fields(after[pick]) == _stamp_fields(as_old[pick])
    assert as_old[pick]["pinned"] == picked.pinned, as_old[pick]
    assert after[pick]["pinned"] == f"{picked.fq}@{picked.digest}", after[pick]
    assert {n: after[n] for n in before if n != pick} == {
        n: before[n] for n in before if n != pick
    }


def test_a_bundle_pick_records_its_digest_on_a_member_the_plugin_also_includes(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`pack` lists only `x`, which the plugin also includes, so no row carries its stamp.

    A pick of `pack` over the old grim's part held `x` outside the bundle and restored
    the row whole, so the saved part recorded no digest for the bundle it selected.
    `x` now keeps its pin and gains `pack`'s stamp at the digest the pick read, and
    every other row stays as the old grim wrote it.
    """
    x = _skill(unique_repo, "x")
    extra = _skill(unique_repo, "extra")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", x.fq)], tag="1")
    _write_manifest(work, [pack.fq, x.fq, extra.fq])
    assert _update(old).returncode == 0
    before = _rows(work)

    result = _update(current, "team:pack")

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert after["x"]["pinned"] == before["x"]["pinned"]
    assert after["x"].get("bundle_tag") == f"1@{pack.digest}", after["x"]
    assert after["x"]["bundle"].endswith("/pack"), after["x"]
    assert after["extra"] == before["extra"]
    written = _lock_text(work)
    assert _export_plugin(old).returncode == 0, "0.15.1 reads the part"
    assert _lock_text(work) == written


def test_a_bundle_pick_records_its_digest_below_a_bundle_it_newly_nests(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """The old grim stamped `y` with `outer` and `x` with `inner`, both plainly.

    `outer`'s tag then moves onto a list nesting `inner` alone. A pick of `outer` let
    `inner` go for its plain stamp before it saw `inner` nested, so `outer`'s stamp
    walk stopped there and `x` recorded no digest for the bundle the pick selected.
    `x` now records `outer`'s at the digest the pick read and keeps its pin's digest.
    `inner`'s stamp stays plain: the pick names `outer`, and a row `inner` dropped
    would keep the plain stamp beside the digest.
    """
    x = _skill(unique_repo, "x")
    y = _skill(unique_repo, "y")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "x", x.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    _write_manifest(work, [outer.fq, inner.fq])
    assert _update(old).returncode == 0
    retag(f"{unique_repo}/outer", "keep", outer.digest)
    nests = make_bundle(
        f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1"
    )

    result = _update(current, "team:outer")

    assert result.returncode == 0, result.stderr
    assert "y" not in _rows(work), "the moved list no longer provides y"
    row = _rows(work)["x"]
    assert row["pinned"].endswith(f"@{x.digest}"), row
    stamps = {s["repo"].rsplit("/", 1)[1]: s["tag"] for s in row.get("bundles", [])}
    assert stamps == {
        "inner": "1",
        "outer": f"1@{nests.digest}",
    }, row
    written = _lock_text(work)
    assert _export_plugin(old).returncode == 0, "0.15.1 reads the part"
    assert _lock_text(work) == written


def test_an_export_keeps_a_nested_bundle_pin_recorded_below_an_old_parent(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`inner` records its digest, `outer` none, and `outer` then nests `inner`.

    The old grim wrote both stamps plainly and a pick of `inner` recorded its digest on
    `x`. `outer` later nests `inner` and `inner` lists `z`; the manifest drops the
    `inner` include. The export read `inner`'s tag and replaced `x` by `z`, though the
    part holds `inner` at the digest it recorded.
    """
    x = _skill(unique_repo, "x")
    y = _skill(unique_repo, "y")
    z = _skill(unique_repo, "z")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "x", x.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "y", y.fq)], tag="1")
    _write_manifest(work, [outer.fq, inner.fq])
    assert _update(old).returncode == 0
    assert _update(current, "team:inner").returncode == 0
    assert _rows(work)["x"].get("bundle_tag") == f"1@{inner.digest}"
    retag(f"{unique_repo}/inner", "keep", inner.digest)
    make_bundle(f"{unique_repo}/inner", [("skill", "z", z.fq)], tag="1")
    retag(f"{unique_repo}/outer", "keep", outer.digest)
    make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    _write_manifest(work, [outer.fq])

    result = _export_plugin(current)

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert set(rows) == {"x"}, set(rows)
    assert f"1@{inner.digest}" in {
        *(s["tag"] for s in rows["x"].get("bundles", [])),
        rows["x"].get("bundle_tag"),
    }, rows["x"]


def _requiring_sibling(
    published: _Published, work: Path, unique_repo: str
) -> PublishedArtifact:
    """Add `pk2` (`y`, `b`), whose `y:1` requires `held:1`; return `y:1`."""
    b = _skill(unique_repo, "b")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    requires = [("skill", "held", published.held.fq)]
    y = make_dependent(f"{unique_repo}/y", "1", "skill", files, requires)
    members = [("skill", "y", y.fq), ("skill", "b", b.fq)]
    pk2 = make_bundle(f"{unique_repo}/pk2", members, tag="1")
    _write_manifest(work, [*published.includes(), pk2.fq])
    return y


def test_a_pick_over_an_old_part_refuses_a_dependent_beside_a_newer_version_it_requires(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
) -> None:
    """`y:1` moves onto a manifest requiring `held:2` with `pack`; the part holds `y` old.

    The old grim's part records no edge of `y` and no tag in any pin. A pick of `held`
    or `pack` takes `held:2` beside `y` at its pin, which requires `held:1`: exit 0 and a
    part `grim export` refuses with 65. Both picks refuse with 78 now and write nothing,
    and a pick of `extra`, which moves no row `y` requires, passes.
    """
    y = _requiring_sibling(published, work, unique_repo)
    assert _update(old).returncode == 0
    published.roll()
    retag(f"{unique_repo}/y", "keep", y.digest)
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y v2\n"}
    requires = [("skill", "held", published.held2.fq)]
    make_dependent(f"{unique_repo}/y", "1", "skill", files, requires)
    written = _lock_text(work)

    for pick in ("team:held", "team:pack"):
        result = _update(current, pick)

        assert result.returncode == 78, (pick, result.stderr)
        assert "skill 'y'" in result.stderr and "requires" in result.stderr, (
            result.stderr
        )
        assert _lock_text(work) == written

    extra = _update(current, "team:extra")
    assert extra.returncode == 0, extra.stderr


def test_a_pick_over_an_old_part_refuses_a_dependent_beside_another_version_of_a_bundle_it_requires(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`y:1` requires the bundle `req:1`; `pack` then nests `req:2` and `y:1` requires it.

    The old grim's part holds `pack` flat and `y` at its pin, and records no row of `req`:
    0.15.1 follows no `requires` layer. A pick of `pack` took `req:2` with its `x:2` beside
    `y` at its pin, which requires `req:1`: exit 0 and a part that pairs the two versions.
    It refuses with 78 now as it does for a skill, names the bundle and writes nothing, and
    a pick of `extra`, which moves no bundle `y` requires, passes. 0.15.1 itself refuses a
    nested bundle outright (65).
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(f"{unique_repo}/y", "1", "skill", files, [("bundle", "req", req1.fq)])
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [pack.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    assert set(_rows(work)) == {"b", "extra", "leaving", "y"}, set(_rows(work))
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(f"{unique_repo}/y", "1", "skill", files, [("bundle", "req", req2.fq)])
    written = _lock_text(work)

    assert _update(old, "team:pack").returncode == 65
    result = _update(current, "team:pack")

    assert result.returncode == 78, result.stderr
    assert "skill 'y'" in result.stderr and "bundle 'req'" in result.stderr, result.stderr
    assert "requires" in result.stderr and "req:2" in result.stderr, result.stderr
    assert _lock_text(work) == written
    by_member = _update(current, "team:extra")
    assert by_member.returncode == 0, by_member.stderr


def test_a_pick_over_an_old_part_refuses_a_dependent_requiring_the_moved_bundle_under_another_binding(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`y:1` requires the bundle `req:1` as `other`; `pack` then nests `req:2` as `req`.

    The shape of the requirement above with the two bindings apart. The held edge looked
    for a bundle named `other`, found none in the pick's list and refused nothing, so a
    pick of `pack` or `req` exited 0 and wrote `x:2` beside `y`. The edge is matched by
    repository now: 78, naming the pick's bundle, nothing written.
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "other", req1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [pack.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "other", req2.fq)]
    )
    written = _lock_text(work)

    assert _update(old, "team:pack").returncode == 65
    for pick in ("team:pack", "team:req"):
        result = _update(current, pick)

        assert result.returncode == 78, (pick, result.stderr)
        assert "skill 'y'" in result.stderr, result.stderr
        assert "requires req:1" in result.stderr, result.stderr
        assert "bundle 'req' to req:2" in result.stderr, result.stderr
        assert _lock_text(work) == written
    by_member = _update(current, "team:extra")
    assert by_member.returncode == 0, by_member.stderr


def test_a_pick_over_an_old_part_shows_no_moved_bundle_where_the_requirement_tag_was_republished(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`y:1` requires the bundle `req:1`; `pack` then nests `req:2`, which `req:1` names too.

    grim 0.15.1 follows no `requires`, so its part stamps no row through `req:1`. The held
    edge reads `req:1` as the digest the pick moves to, and nothing in the part shows the
    bundle stood elsewhere: the pick exits 0 and keeps `y` at its pin, as it did before. A
    part that holds `x:1` through `req:1` refuses the same pick with 78.
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "req", req1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [pack.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    assert "x" not in _rows(work)
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "req", req2.fq)]
    )
    retag(f"{unique_repo}/req", "keep", req1.digest)
    retag(f"{unique_repo}/req", "1", req2.digest)
    written = _lock_text(work)
    held = _rows(work)["y"]

    assert _update(old, "team:pack").returncode == 65
    for pick in ("team:pack", "team:req"):
        (work / "marketplace.lock").write_text(written, encoding="utf-8", newline="\n")
        result = _update(current, pick)

        assert result.returncode == 0, (pick, result.stderr)
        rows = _rows(work)
        assert rows["y"]["pinned"].endswith(held["pinned"].rsplit("@", 1)[1]), rows["y"]


def test_a_pick_over_an_old_part_shows_no_moved_row_below_a_held_bundle_requirement(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`y:1` requires the bundle `outer:1`, which lists `x:1`; `pack` then lists `x:2`.

    grim 0.15.1 follows no `requires`, so its part holds no `x`. The pick of `pack` brings
    `x:2` as a new row, and nothing in the part shows it moved one: the pick exits 0 and
    keeps `y` at its pin, as it did before the rows of a held requirement were judged. A
    part that holds `x:1` refuses the same pick with 78.
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    outer1 = make_bundle(f"{unique_repo}/outer", [("skill", "x", x1.fq)], tag="1")
    outer2 = make_bundle(f"{unique_repo}/outer", [("skill", "x", x2.fq)], tag="2")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "outer", outer1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [pack.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    assert "x" not in _rows(work)
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("skill", "x", x2.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "outer", outer2.fq)]
    )
    held = _rows(work)["y"]

    assert _update(old, "team:pack").returncode == 79
    result = _update(current, "team:pack")

    assert result.returncode == 0, result.stderr
    rows = _rows(work)
    assert rows["y"]["pinned"].endswith(held["pinned"].rsplit("@", 1)[1]), rows["y"]
    assert rows["x"]["pinned"].endswith(x2.digest), rows["x"]


def test_a_pick_over_an_old_part_refuses_a_dependent_beside_another_version_of_a_bundle_its_bundle_requirement_nests(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`y:1` requires the bundle `outer:1`, which lists `x:1` flat when the old grim writes.

    `outer:1` nests `req:1` afterwards, `outer:2` nests `req:2`, `y:1` moves onto a manifest
    requiring `outer:2`, and `pack` nests `req:2`. The old grim's part holds `y` at its pin and
    records no row of `req` or `outer`. A pick of `pack` or `req` took `req:2` with its `x:2`
    beside `y` at its pin, which requires `outer:1`, which nests `req:1`: exit 0. It refuses with
    78 now, names the path through `outer:1` and writes nothing; a pick of `extra`, which moves
    no bundle `y` requires, passes. 0.15.1 itself refuses a nested bundle outright (65).
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    outer1 = make_bundle(f"{unique_repo}/outer", [("skill", "x", x1.fq)], tag="1")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "outer", outer1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [pack.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    assert set(_rows(work)) == {"b", "extra", "leaving", "y"}, set(_rows(work))
    retag(f"{unique_repo}/outer", "keep", outer1.digest)
    make_bundle(f"{unique_repo}/outer", [("bundle", "req", req1.fq)], tag="1")
    outer2 = make_bundle(f"{unique_repo}/outer", [("bundle", "req", req2.fq)], tag="2")
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "outer", outer2.fq)]
    )
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    written = _lock_text(work)

    assert _update(old, "team:pack").returncode == 65
    for pick in ("team:pack", "team:req"):
        result = _update(current, pick)

        assert result.returncode == 78, (pick, result.stderr)
        assert "skill 'y'" in result.stderr, result.stderr
        assert "requires req:1 through outer:1" in result.stderr, result.stderr
        assert "bundle 'req' to req:2" in result.stderr, result.stderr
        assert _lock_text(work) == written
    by_member = _update(current, "team:extra")
    assert by_member.returncode == 0, by_member.stderr


def test_a_pick_over_an_old_part_judges_every_binding_a_nested_list_names_one_bundle_under(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`outer:1` lists `req:1` as `a` and as `req`; the pick moves `req` to `req:2`.

    The shape of the nested requirement above, with `outer:1` republished to name one
    bundle under two bindings. Only `a`, which sorts first and which the pick does not
    move, was judged, so a pick of `pack` or `req` exited 0 and wrote `x:2` beside `y`.
    Each binding is judged now: 78, naming the path through `outer:1`, nothing written.
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    outer1 = make_bundle(f"{unique_repo}/outer", [("skill", "x", x1.fq)], tag="1")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "outer", outer1.fq)]
    )
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "leaving", leaving.fq)], tag="1")
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [pack.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    retag(f"{unique_repo}/outer", "keep", outer1.digest)
    make_bundle(
        f"{unique_repo}/outer",
        [("bundle", "a", req1.fq), ("bundle", "req", req1.fq)],
        tag="1",
    )
    outer2 = make_bundle(
        f"{unique_repo}/outer",
        [("bundle", "a", req2.fq), ("bundle", "req", req2.fq)],
        tag="2",
    )
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(
        f"{unique_repo}/y", "1", "skill", files, [("bundle", "outer", outer2.fq)]
    )
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(
        f"{unique_repo}/pack",
        [("bundle", "req", req2.fq), ("skill", "leaving", leaving.fq)],
        tag="1",
    )
    written = _lock_text(work)

    assert _update(old, "team:pack").returncode == 65
    for pick in ("team:pack", "team:req"):
        result = _update(current, pick)

        assert result.returncode == 78, (pick, result.stderr)
        assert "requires req:1 through outer:1" in result.stderr, result.stderr
        assert "bundle 'req' to req:2" in result.stderr, result.stderr
        assert _lock_text(work) == written
    by_member = _update(current, "team:extra")
    assert by_member.returncode == 0, by_member.stderr


def test_a_pick_over_an_old_part_refuses_a_dependent_beside_another_version_of_a_bundle_a_picked_skill_requires(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """`y:1` requires the bundle `req:1`; `p:1` and then `y:1` require `req:2`.

    The old grim's part holds `y` at its pin and records no row of `req`. A pick of the
    skill `p` takes `req:2` with its `x:2` through `p`'s `requires`, beside `y` at its pin,
    which requires `req:1`: exit 0 and a part that pairs the two versions. It refuses with
    78 now, names the skill and the bundle and writes nothing, and a pick of `extra`, which
    reaches no bundle `y` requires, passes.
    """
    x1 = _skill(unique_repo, "x")
    x2 = _skill(unique_repo, "x", "v2", tag="2")
    p = _skill(unique_repo, "p")
    b = _skill(unique_repo, "b")
    extra = _skill(unique_repo, "extra")
    req1 = make_bundle(f"{unique_repo}/req", [("skill", "x", x1.fq)], tag="1")
    req2 = make_bundle(f"{unique_repo}/req", [("skill", "x", x2.fq)], tag="2")
    y_files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(f"{unique_repo}/y", "1", "skill", y_files, [("bundle", "req", req1.fq)])
    pk2 = make_bundle(
        f"{unique_repo}/pk2", [("skill", "y", y.fq), ("skill", "b", b.fq)], tag="1"
    )
    _write_manifest(work, [p.fq, pk2.fq, extra.fq])
    assert _update(old).returncode == 0
    assert set(_rows(work)) == {"b", "extra", "p", "y"}, set(_rows(work))
    retag(f"{unique_repo}/p", "keep", p.digest)
    retag(f"{unique_repo}/y", "keep", y.digest)
    p_files = {"p/SKILL.md": "---\nname: p\ndescription: d\n---\n# p\n"}
    make_dependent(f"{unique_repo}/p", "1", "skill", p_files, [("bundle", "req", req2.fq)])
    make_dependent(f"{unique_repo}/y", "1", "skill", y_files, [("bundle", "req", req2.fq)])
    written = _lock_text(work)

    result = _update(current, "team:p")

    assert result.returncode == 78, result.stderr
    assert "skill 'y'" in result.stderr and "bundle 'req'" in result.stderr, result.stderr
    assert "requires" in result.stderr and "req:2" in result.stderr, result.stderr
    assert _lock_text(work) == written
    by_member = _update(current, "team:extra")
    assert by_member.returncode == 0, by_member.stderr


def test_a_pick_over_an_old_part_holds_the_requirement_of_a_row_it_holds(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
) -> None:
    """`pack` drops `held`; `y`, which the old grim wrote with no edge, still requires it.

    `y` was republished without a requires layer, so the pick keeps its old row. The part
    records no edge of `y`, so its manifest at the pinned digest shows the requirement,
    and `held` stays at its pin as the part holds it.
    """
    y = _requiring_sibling(published, work, unique_repo)
    assert _update(old).returncode == 0
    before = _rows(work)
    retag(f"{unique_repo}/pack", "keep-second", published.pack.digest)
    make_bundle(
        f"{unique_repo}/pack", [("skill", "leaving", published.leaving.fq)], tag="1"
    )
    retag(f"{unique_repo}/y", "keep-third", y.digest)
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y v3\n"}
    make_artifact(f"{unique_repo}/y", "skill", files, tag="1")

    result = _update(current, "team:pack")

    assert result.returncode == 0, result.stderr
    after = _rows(work)
    assert {"held", "y"} <= set(after), set(after)
    assert after["held"] == before["held"], after["held"]
    assert after["y"] == before["y"], after["y"]


def test_a_pick_over_an_old_part_reads_an_include_republished_as_a_skill(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    published: _Published,
    unique_repo: str,
) -> None:
    """`pack:1` held `held` and `leaving`; it is a skill now.

    The old grim's part records no digest for it. A pick of `extra` read the include as
    the bundle the part's stamp names and exited 65; the include reads its tag for its
    kind, so the pick passes. A pick of `held`, which left with the bundle, exits 79,
    and a pick of `pack` drops the rows the bundle let go.
    """
    assert _update(old).returncode == 0
    retag(f"{unique_repo}/pack", "keep", published.pack.digest)
    files = {"pack/SKILL.md": "---\nname: pack\ndescription: d\n---\n# pack\n"}
    make_artifact(f"{unique_repo}/pack", "skill", files, tag="1")

    picked = _update(current, "team:extra")
    assert picked.returncode == 0, picked.stderr

    written = _lock_text(work)
    gone = _update(current, "team:held")
    assert gone.returncode == 79, gone.stderr
    assert _lock_text(work) == written

    whole = _update(current, "team:pack")
    assert whole.returncode == 0, whole.stderr
    assert set(_rows(work)) == {"extra", "pack"}, set(_rows(work))
