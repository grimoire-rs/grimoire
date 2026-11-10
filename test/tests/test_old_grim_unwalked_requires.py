# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Relocking a lock grim 0.15.1 wrote beside a dependent whose edges it never read.

grim 0.15.1 reads no `requires` layer, so its lock records a bundle's member list
and none of a dependent's edges. `x:1` requires a `held` tag that `pack:1` does not
list. The current grim reads `x` at the digest the lock records, and its edge onto
`held` shows nothing the lock's writer walked. A relock settles `held` as a fresh
resolve does: the conflict refuses with 78, or the pin follows its tag. Read as a
walked edge, it kept `held:1`'s digest under `x`'s tag at exit 0.

The old binary comes from `GRIM_OLD_BIN`, a build of tag `v0.15.1`, and the module
skips when it is unset. `GRIM_COMMAND` is the current grim. Each side keeps its own
`GRIM_HOME`, so the old grim reaches a pin through the registry.
"""

from __future__ import annotations

import json
import shutil
import urllib.request
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, make_dependent, write_config
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import REGISTRY_BASE, REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

_MANIFEST = "marketplace.toml"


def _skill(repo: str, name: str, body: str, tag: str = "1") -> PublishedArtifact:
    return make_artifact(
        f"{repo}/{name}",
        "skill",
        {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {body}\n"},
        tag=tag,
    )


def _dependent(
    repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}
    return make_dependent(f"{repo}/{name}", "1", "skill", files, requires)


@dataclass(slots=True)
class _Unwalked:
    """`pack:1` lists `held:1`; `x:1` requires `held:<other>`, another digest."""

    repo: str
    held: PublishedArtifact
    other: PublishedArtifact
    pack: PublishedArtifact
    x: PublishedArtifact

    def served(self) -> dict[str, str]:
        """`repo:tag -> digest` for every tag here: none of them ever moves."""
        held = f"{REGISTRY_HOST}/{self.repo}/held"
        return {
            f"{held}:1": self.held.digest,
            f"{held}:{self.other.tag}": self.other.digest,
        }


def _unwalked(repo: str, other: str) -> _Unwalked:
    held = _skill(repo, "held", "v1")
    theirs = _skill(repo, "held", f"v{other}", tag=other)
    pack = make_bundle(f"{repo}/pack", [("skill", "held", held.fq)], tag="1")
    x = _dependent(repo, "x", [("skill", "held", theirs.fq)])
    return _Unwalked(repo=repo, held=held, other=theirs, pack=pack, x=x)


def _rows(text: str) -> dict[str, dict]:
    return {row["name"]: row for row in tomllib.loads(text).get("skill", [])}


def _false_pairs(text: str, served: dict[str, str]) -> list[str]:
    """Each `pinned` whose tag never served its digest, among the tags `served` knows."""
    wrong = []
    for row in _rows(text).values():
        reference, _, digest = row["pinned"].partition("@")
        if reference in served and served[reference] != digest:
            wrong.append(row["pinned"])
    return wrong


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
    """The old grim in `work`, with its own cold `GRIM_HOME`."""
    grim_home = tmp_path / "old" / "grim-home"
    grim_home.mkdir(parents=True)
    return GrimRunner(old_bin, grim_home, cwd=work)


@pytest.fixture()
def current(grim_at, work: Path, registry: str) -> GrimRunner:
    return grim_at(work)


@pytest.mark.parametrize("other", ["0", "2"])
def test_grim_lock_refuses_a_conflict_the_old_lock_never_walked(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str, other: str
) -> None:
    """`grim lock` exits 78 where it kept `held:1`'s digest under `x`'s tag.

    0.15.1 writes `pack`'s member list and no edge of `x`, so nothing shows a
    resolve of `held:<other>` gave `held`'s pin. The remedy the refusal names,
    declaring `held` directly, then resolves the line rather than stamp it.
    """
    state = _unwalked(unique_repo, other)
    write_config(work, skills={"x": state.x.fq}, bundles={"pack": state.pack.fq})
    assert old.run("lock").returncode == 0
    lock = work / "grimoire.lock"
    written = lock.read_bytes()
    assert "requires" not in written.decode(), "0.15.1 records no edge"

    refused = current.run("lock", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "conflicting versions" in refused.stderr, refused.stderr
    assert "declare skill held directly" in refused.stderr, refused.stderr
    assert lock.read_bytes() == written

    added = current.run(
        "add", "--no-install", "--kind", "skill", state.other.fq, check=False
    )

    assert added.returncode == 0, added.stderr
    text = lock.read_text(encoding="utf-8")
    assert _rows(text)["held"]["pinned"] == f"{state.other.fq}@{state.other.digest}"
    assert _false_pairs(text, state.served()) == []


@pytest.mark.parametrize("move", ["both-tags", "pin-tag-joins"])
@pytest.mark.parametrize("other", ["0", "2"])
def test_an_add_at_the_lists_tag_pins_what_grim_lock_pins_over_the_typed_line(
    old: GrimRunner,
    current: GrimRunner,
    grim_binary: Path,
    tmp_path: Path,
    work: Path,
    unique_repo: str,
    other: str,
    move: str,
) -> None:
    """`grim add held:1` pins what `grim lock` pins with the same line typed by hand.

    `pack:1` lists `held:1`, and `x:1` requires `held:<other>`, an edge 0.15.1
    never read. Once both tags name one digest, `grim lock` releases `held`'s
    tagless pin and resolves the line. The add kept `held:1`'s old digest from
    `pack`'s recorded list instead, a pin no `grim lock` writes.
    """
    state = _unwalked(unique_repo, other)
    write_config(work, skills={"x": state.x.fq}, bundles={"pack": state.pack.fq})
    assert old.run("lock").returncode == 0
    repo = f"{unique_repo}/held"
    retag(repo, "keep", state.held.digest)
    if move == "both-tags":
        target = _skill(unique_repo, "held", "v9", tag="9").digest
        retag(repo, "1", target)
        retag(repo, other, target)
    else:
        target = state.other.digest
        retag(repo, "1", target)
    typed = tmp_path / "typed"
    shutil.copytree(work, typed)
    skills = {"x": state.x.fq, "held": state.held.fq}
    write_config(typed, skills=skills, bundles={"pack": state.pack.fq})
    home = tmp_path / "typed-side" / "grim-home"
    home.mkdir(parents=True)

    locked = GrimRunner(grim_binary, home, cwd=typed).run("lock", check=False)
    added = current.run(
        "add", "--no-install", "--kind", "skill", state.held.fq, check=False
    )

    assert locked.returncode == 0, locked.stderr
    assert added.returncode == 0, added.stderr
    by_hand = _rows((typed / "grimoire.lock").read_text(encoding="utf-8"))
    text = (work / "grimoire.lock").read_text(encoding="utf-8")
    assert _rows(text)["held"]["pinned"] == by_hand["held"]["pinned"]
    assert _rows(text)["held"]["pinned"] == f"{state.held.fq}@{target}"


def _cut_bundle_rows(work: Path) -> None:
    """Cut every `[[bundle]]` table, as a lock written before member lists."""
    lock = work / "grimoire.lock"
    kept, cutting = [], False
    for line in lock.read_text(encoding="utf-8").splitlines(keepends=True):
        if line.startswith("["):
            cutting = line.startswith(("[[bundle]]", "[[bundle."))
        if not cutting:
            kept.append(line)
    lock.write_text("".join(kept), encoding="utf-8")


def _delete_tag(repo: str, tag: str) -> None:
    """Delete `tag`, as a publisher withdraws one; a manifest another tag names stays."""
    url = f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}"
    request = urllib.request.Request(url, method="DELETE")
    with urllib.request.urlopen(request) as response:
        assert response.status == 202, response.status


def test_an_add_that_keeps_a_member_pin_names_the_update(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str
) -> None:
    """A line at a member's tag keeps the pin the bundle's edge stamps, and says so.

    With no member list, only the bundle's walk vouches for `held`'s tagless pin,
    and `held:1` is gone, so no tag read could have kept it. The report says
    `added` either way, so the note is the one place the kept pin is named.
    """
    held = _skill(unique_repo, "held", "v1")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    write_config(work, bundles={"pack": pack.fq})
    assert old.run("lock").returncode == 0
    _cut_bundle_rows(work)
    retag(f"{unique_repo}/held", "keep", held.digest)
    _delete_tag(f"{unique_repo}/held", "1")

    added = current.run("add", "--no-install", "--kind", "skill", held.fq, check=False)

    assert added.returncode == 0, added.stderr
    text = (work / "grimoire.lock").read_text(encoding="utf-8")
    assert _rows(text)["held"]["pinned"] == f"{held.fq}@{held.digest}"
    assert "kept the existing pin of skill 'held'" in added.stderr, added.stderr
    assert "`grim update -- held` moves it to what its tag names now" in added.stderr
    assert "or says why it cannot" in added.stderr, added.stderr
    assert "Declare a reference that exists if that tag is gone" in added.stderr


@pytest.mark.parametrize(
    "case", ["online", "offline", "listed-tag-gone", "asked-tag-gone"]
)
def test_an_add_at_the_lists_tag_keeps_the_pin_grim_lock_keeps(
    old: GrimRunner,
    current: GrimRunner,
    tmp_path: Path,
    work: Path,
    unique_repo: str,
    case: str,
) -> None:
    """`grim add held:1` keeps the pin `grim lock` keeps once `held:1` moved.

    `pack:1` lists `held:1`, and `x:1` requires `held:2`, an edge 0.15.1 never
    read, at the digest `held:1` named when 0.15.1 locked. `grim lock` resolves
    that edge onto the pin and keeps it. The add resolved only its line and moved
    the pin to the tag's new digest. With `held:1` gone the add still refuses with
    79, and with `held:2` gone it follows the line, as 0.15.1's add does.
    """
    repo = f"{unique_repo}/held"
    held = _skill(unique_repo, "held", "v1")
    retag(repo, "2", held.digest)
    asked = f"{REGISTRY_HOST}/{repo}:2"
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    x = _dependent(unique_repo, "x", [("skill", "held", asked)])
    write_config(work, skills={"x": x.fq}, bundles={"pack": pack.fq})
    assert old.run("lock").returncode == 0
    retag(repo, "keep", held.digest)
    moved = _skill(unique_repo, "held", "v9")
    if case == "listed-tag-gone":
        _delete_tag(repo, "1")
    if case == "asked-tag-gone":
        _delete_tag(repo, "2")
    first = tmp_path / "lock-first"
    shutil.copytree(work, first)
    old_dir = tmp_path / "old-add"
    shutil.copytree(work, old_dir)
    line = ("add", "--no-install", "--kind", "skill", held.fq)
    files = ("grimoire.toml", "grimoire.lock")
    before = {name: (work / name).read_bytes() for name in files}

    locked = GrimRunner(current.binary, current.grim_home, cwd=first).run(
        "lock", check=False
    )
    if case == "offline":
        warm = tmp_path / "warm-add"
        shutil.copytree(work, warm)
        GrimRunner(current.binary, current.grim_home, cwd=warm).run(*line)
        added = current.run("--offline", *line, check=False)
    else:
        added = current.run(*line, check=False)
    by_old = GrimRunner(old.binary, old.grim_home, cwd=old_dir).run(*line, check=False)

    if case == "listed-tag-gone":
        assert by_old.returncode == 79, by_old.stderr
        assert added.returncode == 79, added.stderr
        assert {name: (work / name).read_bytes() for name in files} == before
        return
    assert added.returncode == 0, added.stderr
    pinned = _rows((work / "grimoire.lock").read_text(encoding="utf-8"))["held"]
    if case == "asked-tag-gone":
        assert by_old.returncode == 0, by_old.stderr
        assert locked.returncode == 79, locked.stderr
        assert pinned["pinned"] == f"{held.fq}@{moved.digest}"
        return
    assert locked.returncode == 0, locked.stderr
    by_lock = _rows((first / "grimoire.lock").read_text(encoding="utf-8"))["held"]
    assert by_lock["pinned"] == f"{held.fq}@{held.digest}"
    assert pinned["pinned"] == by_lock["pinned"]


def _without_generated(text: str) -> str:
    """The lock text without the `generated_*` stamps, which differ per run."""
    return "".join(
        line
        for line in text.splitlines(keepends=True)
        if not line.startswith("generated")
    )


@pytest.mark.parametrize(
    ("case", "update_says"),
    [
        ("cycle-at-another-digest", "dependency cycle"),
        ("gone-direct", "tag not found"),
        ("gone-deep", "tag not found"),
    ],
)
def test_an_add_keeps_the_pin_where_only_its_moved_manifest_refuses(
    old: GrimRunner,
    current: GrimRunner,
    grim_binary: Path,
    tmp_path: Path,
    work: Path,
    unique_repo: str,
    case: str,
    update_says: str,
) -> None:
    """`grim add held:1` keeps the pin `grim lock` keeps, whatever the moved manifest asks.

    `pack:1` lists `held:1` and `x:1` requires `held:2`, at the digest 0.15.1
    locked. Then `held:1` moves to a manifest that asks `w:1`, which asks `held:7`
    (a cycle back at another digest), or that asks a tag which is gone, directly or
    under `w`. `grim lock` never reads that manifest and keeps the pin, before the
    line is typed and after. The add refused with 65 or 79 and wrote nothing. Now
    it keeps the pin and writes the lock that lock, typed line and lock write. The
    note says why `grim update -- held` cannot move it, and that command refuses.
    """
    repo = f"{unique_repo}/held"
    held = _skill(unique_repo, "held", "v1")
    retag(repo, "2", held.digest)
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    x = _dependent(unique_repo, "x", [("skill", "held", f"{REGISTRY_HOST}/{repo}:2")])
    write_config(work, skills={"x": x.fq}, bundles={"pack": pack.fq})
    assert old.run("lock").returncode == 0
    retag(repo, "keep", held.digest)
    gone = _skill(unique_repo, "gone", "g")
    _delete_tag(f"{unique_repo}/gone", "1")
    if case == "cycle-at-another-digest":
        _skill(unique_repo, "held", "v7", tag="7")
        w = _dependent(unique_repo, "w", [("skill", "held", f"{REGISTRY_HOST}/{repo}:7")])
        asks = [("skill", "w", w.fq)]
    elif case == "gone-direct":
        asks = [("skill", "gone", gone.fq)]
    else:
        w = _dependent(unique_repo, "w", [("skill", "gone", gone.fq)])
        asks = [("skill", "w", w.fq)]
    moved = _dependent(unique_repo, "held", asks)
    assert moved.digest != held.digest
    typed = tmp_path / "typed"
    shutil.copytree(work, typed)
    home = tmp_path / "typed-side" / "grim-home"
    home.mkdir(parents=True)
    by_hand = GrimRunner(grim_binary, home, cwd=typed)

    first = by_hand.run("lock", check=False)
    write_config(
        typed,
        skills={"x": x.fq, "held": held.fq},
        bundles={"pack": pack.fq},
    )
    locked = by_hand.run("lock", check=False)
    added = current.run(
        "add", "--no-install", "--kind", "skill", held.fq, check=False
    )

    assert (first.returncode, locked.returncode) == (0, 0), locked.stderr
    assert added.returncode == 0, added.stderr
    text = (work / "grimoire.lock").read_text(encoding="utf-8")
    assert _rows(text)["held"]["pinned"] == f"{held.fq}@{held.digest}"
    assert _without_generated(text) == _without_generated(
        (typed / "grimoire.lock").read_text(encoding="utf-8")
    )
    assert "kept the existing pin of skill 'held'" in added.stderr, added.stderr
    assert "does not resolve" in added.stderr, added.stderr
    assert "`grim update -- held` says why" in added.stderr, added.stderr
    assert "moves it to what its tag names now" not in added.stderr, added.stderr
    update = current.run("update", "--", "held", check=False)
    assert update.returncode in (65, 78, 79), update.stderr
    assert update_says in update.stderr, update.stderr
    after = (work / "grimoire.lock").read_text(encoding="utf-8")
    assert _rows(after)["held"]["pinned"] == f"{held.fq}@{held.digest}"


def test_an_add_keeps_its_exit_where_the_moved_manifest_reaches_an_unreachable_registry(
    current: GrimRunner,
    grim_binary: Path,
    old: GrimRunner,
    tmp_path: Path,
    work: Path,
    unique_repo: str,
) -> None:
    """Only a gone tag below the moved manifest lets `grim add` keep the pin.

    The setup of the test above, but the moved manifest asks a registry nothing
    listens on. The registry's absence is not a gone tag: the add exits 69, as
    `grim lock` over the typed line does (it reads that line's tag too) and as
    grim c5e3824 does, and writes nothing. Found by review: the add exited 0.
    """
    repo = f"{unique_repo}/held"
    held = _skill(unique_repo, "held", "v1")
    retag(repo, "2", held.digest)
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    x = _dependent(unique_repo, "x", [("skill", "held", f"{REGISTRY_HOST}/{repo}:2")])
    write_config(work, skills={"x": x.fq}, bundles={"pack": pack.fq})
    assert old.run("lock").returncode == 0
    retag(repo, "keep", held.digest)
    moved = _dependent(unique_repo, "held", [("skill", "bad", "localhost:1/acme/bad:1")])
    assert moved.digest != held.digest
    typed = tmp_path / "typed"
    shutil.copytree(work, typed)
    home = tmp_path / "typed-side" / "grim-home"
    home.mkdir(parents=True)
    by_hand = GrimRunner(grim_binary, home, cwd=typed)
    write_config(
        typed,
        skills={"x": x.fq, "held": held.fq},
        bundles={"pack": pack.fq},
    )
    before = (work / "grimoire.lock").read_text(encoding="utf-8")

    locked = by_hand.run("lock", check=False)
    added = current.run(
        "add", "--no-install", "--kind", "skill", held.fq, check=False
    )

    assert (locked.returncode, added.returncode) == (69, 69), added.stderr
    assert "registry unreachable" in added.stderr, added.stderr
    assert (work / "grimoire.lock").read_text(encoding="utf-8") == before


def _write_manifest(work: Path, includes: list[str]) -> None:
    (work / _MANIFEST).write_text(
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f"[plugins.team]\ninclude = {json.dumps(includes)}\n",
        encoding="utf-8",
    )


def _update(runner: GrimRunner, *selectors: str):
    return runner.run("update", "--marketplace", _MANIFEST, *selectors, check=False)


@pytest.mark.parametrize("pick", ["team:pack", "team:x"])
def test_a_pick_over_an_old_part_writes_no_tag_beside_a_digest_it_never_served(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str, pick: str
) -> None:
    """A pick over a part 0.15.1 wrote never stamps `held:0` beside `held:1`'s digest.

    Either pick reads `pack` and `x` and refuses their conflict with 78, as a whole
    update does, leaving the part as it was. Left unread beside a pick of `x`, `pack`
    lost its member to `held:0` at exit 0.
    """
    state = _unwalked(unique_repo, "0")
    extra = _skill(unique_repo, "extra", "e")
    _write_manifest(work, [state.pack.fq, state.x.fq, extra.fq])
    assert _update(old).returncode == 0
    part = work / "marketplace.lock"
    written = part.read_bytes()
    assert "requires" not in written.decode(), "0.15.1 records no edge"

    result = _update(current, pick)

    text = part.read_text(encoding="utf-8")
    assert _false_pairs(text, state.served()) == [], text
    assert result.returncode == 78, result.stderr
    assert "conflicting versions" in result.stderr, result.stderr
    assert part.read_bytes() == written


@pytest.mark.parametrize("pick", ["team:p", "team:pack"])
def test_a_pick_through_a_digest_an_old_part_never_walked_follows_the_tag(
    old: GrimRunner, current: GrimRunner, work: Path, unique_repo: str, pick: str
) -> None:
    """`p:1` requires `pack@P2`, the digest `pack:1` later moves to.

    0.15.1 never read that edge, so `pack`'s list at `P2` shows nothing the part
    saw: `held` resolves `held:2`, the tag the list names, and keeps no
    `held:1` digest under it.
    """
    held = _skill(unique_repo, "held", "v1")
    held2 = _skill(unique_repo, "held", "v2", tag="2")
    leaving = _skill(unique_repo, "leaving", "l")
    arriving = _skill(unique_repo, "arriving", "a")
    extra = _skill(unique_repo, "extra", "e")
    listed = [("skill", "held", held.fq), ("skill", "leaving", leaving.fq)]
    pack1 = make_bundle(f"{unique_repo}/pack", listed, tag="1")
    moved = [("skill", "held", held2.fq), ("skill", "arriving", arriving.fq)]
    pack2 = make_bundle(f"{unique_repo}/pack", moved, tag="2")
    by_digest = f"{REGISTRY_HOST}/{unique_repo}/pack@{pack2.digest}"
    p = _dependent(unique_repo, "p", [("bundle", "pack", by_digest)])
    _write_manifest(work, [pack1.fq, extra.fq, p.fq])
    assert _update(old).returncode == 0
    retag(f"{unique_repo}/pack", "keep", pack1.digest)
    retag(f"{unique_repo}/pack", "1", pack2.digest)

    result = _update(current, pick)

    assert result.returncode == 0, result.stderr
    text = (work / "marketplace.lock").read_text(encoding="utf-8")
    assert _rows(text)["held"]["pinned"] == f"{held2.fq}@{held2.digest}"
    base = f"{REGISTRY_HOST}/{unique_repo}/held"
    served = {f"{base}:1": held.digest, f"{base}:2": held2.digest}
    assert _false_pairs(text, served) == [], text


def _store_requires(
    current: GrimRunner, tmp_path: Path, dependent: PublishedArtifact
) -> None:
    """Another project's `grim add` on the shared home stores `dependent`'s requires layer."""
    other = tmp_path / "other"
    (other / ".claude").mkdir(parents=True)
    write_config(other)
    name = dependent.repo.rsplit("/", 1)[-1]
    GrimRunner(current.binary, current.grim_home, cwd=other).run(
        "add", "--no-install", "--name", name, dependent.fq
    )


def _pin(project: Path, name: str) -> str:
    rows = tomllib.loads((project / "grimoire.lock").read_text(encoding="utf-8"))
    return {row["name"]: row["pinned"] for row in rows["skill"]}[name]


def test_the_recovery_from_a_subdirectory_names_the_projects_pair_by_path(
    old: GrimRunner,
    current: GrimRunner,
    work: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """`grim remove skill m` from `work/sub` names `work`'s pair, not the shell's.

    0.15.1 locks `m` and adds `p`, which requires `m:1`; the lock records no edge of
    `p`. The walked run finds `work`'s config from `sub`, so a bare `grimoire.toml`
    in the note would be a pair in `sub`, which holds none. The note names the
    pair by path. Restoring it, one online `grim lock` and the same edit keep `m`'s
    pin, and a later `grim lock` keeps it after its tag moved. Found by review: the
    note printed the bare pair.
    """
    m = _skill(unique_repo, "m", "one")
    p = _dependent(unique_repo, "p", [("skill", "m", m.fq)])
    write_config(work, skills={"m": m.fq})
    assert old.run("lock").returncode == 0
    old.run("add", "--no-install", "--name", "p", p.fq)
    _store_requires(current, tmp_path, p)
    pair = [work / "grimoire.toml", work / "grimoire.lock"]
    saved = [path.read_bytes() for path in pair]
    sub = work / "sub"
    sub.mkdir()
    from_sub = GrimRunner(current.binary, current.grim_home, cwd=sub)

    edited = from_sub.run("remove", "skill", "m")

    here = work.resolve()
    named = f"restore {here}/grimoire.toml and {here}/grimoire.lock, run `grim lock`"
    assert named in edited.stderr, edited.stderr
    assert not any(sub.iterdir()), "the recovery's pair is not a pair in the shell's directory"
    for path, text in zip(pair, saved, strict=True):
        path.write_bytes(text)

    from_sub.run("lock")
    kept = _pin(work, "m")
    again = from_sub.run("remove", "skill", "m")

    assert "no dependency edges" not in again.stderr, again.stderr
    assert _pin(work, "m") == kept == f"{m.fq}@{m.digest}"
    retag(f"{unique_repo}/m", "keep", m.digest)
    assert _skill(unique_repo, "m", "two").digest != m.digest

    from_sub.run("lock")

    assert _pin(work, "m") == kept
