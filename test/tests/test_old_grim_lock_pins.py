# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""How grim 0.15.1 and the current grim share a lock that records its tags.

`pinned` reads `registry/repo:tag@sha256:…` and `lock_version` stays 1, so the
promise that holds is reading on both sides: 0.15.1 treats the tag as advisory
and strips it on write, and the current grim takes that write as a clean
pre-change lock.

The old binary comes from `GRIM_OLD_BIN`, a build of tag `v0.15.1`, and the
module skips when it is unset. `GRIM_COMMAND` is the current grim. Each side
keeps its own `GRIM_HOME` unless a test says otherwise, so the old grim reaches
a pin through the registry rather than a store the current grim filled.
"""

from __future__ import annotations

import json
import shutil
import urllib.request
from collections.abc import Callable
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


def _skill(repo: str, name: str, body: str) -> PublishedArtifact:
    return make_artifact(
        f"{repo}/{name}",
        "skill",
        {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {body}\n"},
        tag="1",
    )


def _rule(repo: str, name: str, body: str) -> PublishedArtifact:
    return make_artifact(
        f"{repo}/{name}",
        "rule",
        {f"{name}.md": f"---\npaths: ['**/*.rs']\n---\n# {body}\n"},
        tag="1",
    )


def _agent(repo: str, name: str, body: str) -> PublishedArtifact:
    return make_artifact(
        f"{repo}/{name}",
        "agent",
        {f"{name}.md": f"---\nname: {name}\ndescription: d\n---\n# {body}\n"},
        tag="1",
    )


# Every tag here moves when a test moves tags; `steady` and `frozen` never do.
_MOVING: dict[str, Callable[[str, str, str], PublishedArtifact]] = {
    "moving": _skill,
    "member": _skill,
    "style": _rule,
    "helper": _agent,
}
_OUTPUTS = {
    "moving": ".claude/skills/moving/SKILL.md",
    "member": ".claude/skills/member/SKILL.md",
    "style": ".claude/rules/style.md",
    "helper": ".claude/agents/helper.md",
}


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


@pytest.fixture()
def old(old_bin: Path, tmp_path: Path, project_dir: Path) -> GrimRunner:
    """The old grim in the project, with its own cold `GRIM_HOME` and `HOME`."""
    grim_home = tmp_path / "old" / "grim-home"
    grim_home.mkdir(parents=True)
    return GrimRunner(old_bin, grim_home, cwd=project_dir)


@pytest.fixture()
def current(grim_at, project_dir: Path, registry: str) -> GrimRunner:
    """The grim under test, in the same project as `old`."""
    return grim_at(project_dir)


def _lock_path(project_dir: Path) -> Path:
    return project_dir / "grimoire.lock"


def _pins(project_dir: Path, file: str = "grimoire.lock") -> dict[str, str]:
    lock = tomllib.loads((project_dir / file).read_text(encoding="utf-8"))
    return {
        entry["name"]: entry["pinned"]
        for kind in ("skill", "rule", "agent", "bundle")
        for entry in lock.get(kind, [])
    }


def _tagless(repo: str, digests: dict[str, str]) -> dict[str, str]:
    return {
        name: f"{REGISTRY_HOST}/{repo}/{name}@{digest}"
        for name, digest in digests.items()
    }


def _recorded(repo: str, digests: dict[str, str]) -> dict[str, str]:
    """Each `pinned` of a lock that records tags.

    `frozen` is a digest-only request, and `[[bundle]]` keeps its tag in a
    sibling field, so those two stay tagless.
    """
    tagged = {
        name: f"{REGISTRY_HOST}/{repo}/{name}:1@{digests[name]}"
        for name in (*_MOVING, "steady")
    }
    return _tagless(repo, digests) | tagged


def _status_pins(runner: GrimRunner) -> dict[str, str | None]:
    return {row["name"]: row["pinned"] for row in runner.json("status")["items"]}


def _states(runner: GrimRunner) -> dict[str, str]:
    return {row["name"]: row["state"] for row in runner.json("status")["items"]}


def _bodies(project_dir: Path) -> dict[str, str]:
    """What each output says about itself: the last word of the file."""
    return {
        name: (project_dir / path).read_text(encoding="utf-8").split()[-1]
        for name, path in _OUTPUTS.items()
    }


def _state_pins(project_dir: Path) -> dict[str, str]:
    path = project_dir / ".grimoire" / "state.json"
    records = json.loads(path.read_text(encoding="utf-8"))["records"]
    return {record["name"]: record["pinned"] for record in records}


def _lock_project(current: GrimRunner, project_dir: Path, repo: str) -> dict[str, str]:
    """Lock a registry-only project with the current grim; return its digests.

    Fails unless the file really carries `:1@sha256:`, so nothing that follows
    can pass on a lock that happens to be tagless.
    """
    moving = {name: publish(repo, name, "v1") for name, publish in _MOVING.items()}
    steady = _skill(repo, "steady", "v1")
    frozen = _skill(repo, "frozen", "v1")
    pack = make_bundle(
        f"{repo}/pack", [("skill", "member", moving["member"].fq)], tag="1"
    )
    write_config(
        project_dir,
        skills={
            "moving": moving["moving"].fq,
            "steady": steady.fq,
            "frozen": frozen.pinned,
        },
        rules={"style": moving["style"].fq},
        agents={"helper": moving["helper"].fq},
        bundles={"pack": pack.fq},
    )

    current.run("lock")

    digests = {name: artifact.digest for name, artifact in moving.items()}
    digests |= {"steady": steady.digest, "frozen": frozen.digest, "pack": pack.digest}
    assert _pins(project_dir) == _recorded(repo, digests)
    return digests


def _move_tags(
    repo: str, digests: dict[str, str], body: str, keep: str
) -> dict[str, str]:
    """Publish `body` under each moving tag; return the digests the tags now name.

    zot drops a manifest once its last tag moves, and a pin is read by digest,
    so the manifest each tag leaves stays reachable under `keep`.
    """
    moved = dict(digests)
    for name, publish in _MOVING.items():
        retag(f"{repo}/{name}", keep, digests[name])
        moved[name] = publish(repo, name, body).digest
        assert moved[name] != digests[name], "a moved tag must change the digest"
    return moved


def test_old_grim_reads_recorded_tags_and_installs_the_pinned_bytes(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """C-036, S-030: 0.15.1 reads the lock, and digests hold over moved tags."""
    digests = _lock_project(current, project_dir, unique_repo)
    _move_tags(unique_repo, digests, "v2", keep="keep")
    written = _lock_path(project_dir).read_bytes()
    held = _tagless(unique_repo, digests) | {"pack": None}

    assert _status_pins(old) == held

    old.run("install")

    assert _bodies(project_dir) == dict.fromkeys(_OUTPUTS, "v1")
    assert _status_pins(old) == held
    assert set(_states(old).values()) == {"installed"}
    assert _lock_path(project_dir).read_bytes() == written

    old.run("lock")


def test_current_grim_keeps_the_pins_old_grim_relocked(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """C-036, S-030: 0.15.1's `lock` strips and re-resolves; those pins then hold.

    The tags move again after that lock. A tag resolve by the current grim would
    land on v3, while a tag that stood still would answer with the digest it
    already held and hide the resolve.
    """
    digests = _lock_project(current, project_dir, unique_repo)
    relocked = _move_tags(unique_repo, digests, "v2", keep="keep")

    old.run("lock")

    assert _pins(project_dir) == _tagless(unique_repo, relocked)
    written = _lock_path(project_dir).read_bytes()
    generated_at = tomllib.loads(written.decode())["metadata"]["generated_at"]
    _move_tags(unique_repo, relocked, "v3", keep="keep-relocked")

    assert _status_pins(current) == _tagless(unique_repo, relocked) | {"pack": None}
    current.run("install")
    assert _bodies(project_dir) == dict.fromkeys(_OUTPUTS, "v2")
    assert _lock_path(project_dir).read_bytes() == written

    result = current.run("lock", format="json")

    assert "re-resolved" not in result.stderr, "a clean lock migrates in silence"
    rows = json.loads(result.stdout)["items"]
    assert {row["action"] for row in rows} == {"unchanged"}
    assert _pins(project_dir) == _recorded(unique_repo, relocked)
    metadata = tomllib.loads(_lock_path(project_dir).read_text(encoding="utf-8"))
    assert metadata["metadata"]["generated_at"] == generated_at
    assert metadata["metadata"]["lock_version"] == 1
    current.run("install")
    assert _bodies(project_dir) == dict.fromkeys(_OUTPUTS, "v2")


def test_current_grim_reads_the_tagged_state_old_grim_wrote(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """C-036, S-030: a pin spelled with its tag in `state.json` still matches.

    0.15.1 loads the lock's `pinned` verbatim, so its `install` copies the tag
    into the state, where the current grim's own records carry none. The tags
    stand still, so one digest meets both spellings: a compare that counted the
    tag would read outdated.
    """
    digests = _lock_project(current, project_dir, unique_repo)
    old.run("install")
    state = _state_pins(project_dir)
    assert state["moving"] == _recorded(unique_repo, digests)["moving"]

    assert set(_states(current).values()) == {"installed"}

    rows = current.json("install")["items"]
    assert {row["status"] for row in rows} == {"unchanged"}


def test_old_grim_reads_a_current_install_on_the_same_home(
    old_bin: Path,
    current: GrimRunner,
    grim_home: Path,
    project_dir: Path,
    unique_repo: str,
) -> None:
    """C-036, S-030: a downgrade in place meets tagless state and a warm store.

    The mirror of the tagged state: 0.15.1 now holds tagged lock pins against the
    tagless state the current grim wrote, in the `GRIM_HOME` both share.
    """
    digests = _lock_project(current, project_dir, unique_repo)
    current.run("install")
    state = _state_pins(project_dir)
    assert state["moving"] == _tagless(unique_repo, digests)["moving"]
    downgraded = GrimRunner(old_bin, grim_home, cwd=project_dir)

    assert set(_states(downgraded).values()) == {"installed"}

    rows = downgraded.json("install")["items"]
    assert {row["status"] for row in rows} == {"unchanged"}


def test_old_grim_exports_a_marketplace_lock_at_its_digests(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """R2, C-036, S-030: 0.15.1 reads a `marketplace.lock` that records its tags.

    `export` renders what the lock pins over moved tags and leaves the file
    alone, and `update --marketplace` rolls the tags forward and strips them.
    """
    artifacts = {
        name: publish(unique_repo, name, "v1") for name, publish in _MOVING.items()
    }
    artifacts["steady"] = _skill(unique_repo, "steady", "v1")
    digests = {name: artifact.digest for name, artifact in artifacts.items()}
    includes = ", ".join(json.dumps(artifact.fq) for artifact in artifacts.values())
    (project_dir / "marketplace.toml").write_text(
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f"[plugins.team]\ninclude = [{includes}]\n",
        encoding="utf-8",
    )
    current.run("update", "--marketplace", "marketplace.toml")
    lock = project_dir / "marketplace.lock"
    assert _pins(project_dir, lock.name) == _recorded(unique_repo, digests)
    written = lock.read_bytes()
    moved = _move_tags(unique_repo, digests, "v2", keep="keep")

    old.run("export", "marketplace")

    exported = project_dir / "claude" / "team" / "skills" / "moving" / "SKILL.md"
    assert exported.read_text(encoding="utf-8").split()[-1] == "v1"
    assert lock.read_bytes() == written

    old.run("update", "--marketplace", "marketplace.toml")

    assert _pins(project_dir, lock.name) == _tagless(unique_repo, moved)


def _declaration_hash(project_dir: Path) -> str:
    lock = tomllib.loads(_lock_path(project_dir).read_text(encoding="utf-8"))
    return lock["metadata"]["declaration_hash"]


def test_old_grim_reads_a_lock_a_current_remove_left_stale_as_stale(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """A `grim remove` on a stale lock writes a hash no declaration produces.

    `a` moves from `:1` to `:2` by hand, so the lock is stale, and the remove of
    `c` changes its rows. The hash it writes keeps the `sha256:<hex>` form, so
    0.15.1 parses it and reads the lock as stale: every row `stale`, `install`
    refused, and its own `grim lock` resolving from scratch.
    """
    a1 = _skill(unique_repo, "a", "one")
    a2 = make_artifact(
        f"{unique_repo}/a",
        "skill",
        {"a/SKILL.md": "---\nname: a\ndescription: d\n---\n# two\n"},
        tag="2",
    )
    c = _skill(unique_repo, "c", "c")
    o = _skill(unique_repo, "o", "o")
    write_config(project_dir, skills={"a": a1.fq, "c": c.fq, "o": o.fq})
    current.run("lock")
    current.run("install")
    proven = _declaration_hash(project_dir)

    write_config(project_dir, skills={"a": a2.fq, "c": c.fq, "o": o.fq})
    current.run("remove", "skill", "c")

    edited = _declaration_hash(project_dir)
    assert edited.startswith("sha256:") and len(edited) == len(proven), edited
    assert edited != proven, "a remove that changed the rows kept the old hash"
    left = _lock_path(project_dir).read_bytes()

    assert set(_states(old).values()) == {"stale"}
    refused = old.run("install", check=False)
    assert refused.returncode == 65 and "stale" in refused.stderr, refused.stderr

    offline = old.run("lock", "--offline", check=False)
    assert offline.returncode == 69, offline.stderr
    assert _lock_path(project_dir).read_bytes() == left

    old.run("lock")

    assert _pins(project_dir) == {
        "a": f"{REGISTRY_HOST}/{unique_repo}/a@{a2.digest}",
        "o": f"{REGISTRY_HOST}/{unique_repo}/o@{o.digest}",
    }
    old.run("install")


def _lock_with_no_member_list(
    project_dir: Path, runner: GrimRunner, unique_repo: str
) -> None:
    """`y` beside `pack`, which does not provide it, on a lock that holds no list."""
    y = _skill(unique_repo, "y", "y")
    m = _skill(unique_repo, "m", "m")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "m", m.fq)], tag="1")
    write_config(project_dir, skills={"y": y.fq}, bundles={"pack": pack.fq})
    runner.run("lock")
    kept, cutting = [], False
    for line in (
        _lock_path(project_dir).read_text(encoding="utf-8").splitlines(keepends=True)
    ):
        if line.startswith("["):
            cutting = line.startswith(("[[bundle]]", "[[bundle."))
        if not cutting:
            kept.append(line)
    _lock_path(project_dir).write_text("".join(kept), encoding="utf-8")


@pytest.mark.parametrize("binary", ["old", "current"])
def test_a_remove_of_a_key_a_bundle_does_not_provide_exits_install_0(
    binary: str,
    request: pytest.FixtureRequest,
    project_dir: Path,
    unique_repo: str,
) -> None:
    """Both binaries drop the row and restamp, so `grim install` exits 0.

    The current grim needs the bundle's list in the store to show that `pack`
    does not provide `y`, which the `grim lock` here left behind.
    """
    runner: GrimRunner = request.getfixturevalue(binary)
    _lock_with_no_member_list(project_dir, runner, unique_repo)

    runner.run("remove", "skill", "y")

    assert "y" not in _pins(project_dir)
    assert runner.run("install", check=False).returncode == 0


def test_old_grim_refuses_the_conflict_a_current_remove_left_stale(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """The remove of a direct line that held a version conflict off.

    The current grim keeps `x`'s row and leaves the lock stale under a hash of
    its own. 0.15.1 reads that lock as stale and its `grim lock` exits 78 on
    the conflict, as a fresh resolve does.
    """
    base = f"{REGISTRY_HOST}/{unique_repo}"
    x1 = _skill(unique_repo, "x", "one")
    make_artifact(
        f"{unique_repo}/x",
        "skill",
        {"x/SKILL.md": "---\nname: x\ndescription: d\n---\n# two\n"},
        tag="2",
    )
    b1 = make_bundle(f"{unique_repo}/b1", [("skill", "x", f"{base}/x:1")], tag="1")
    b2 = make_bundle(f"{unique_repo}/b2", [("skill", "x", f"{base}/x:2")], tag="1")
    write_config(project_dir, skills={"x": x1.fq}, bundles={"b1": b1.fq, "b2": b2.fq})
    current.run("lock")
    current.run("install")
    proven = _declaration_hash(project_dir)

    current.run("remove", "skill", "x")

    edited = _declaration_hash(project_dir)
    assert edited.startswith("sha256:") and edited != proven, edited
    assert _pins(project_dir)["x"] == f"{base}/x:1@{x1.digest}"
    left = _lock_path(project_dir).read_bytes()

    assert set(_states(old).values()) == {"stale"}
    assert old.run("install", check=False).returncode == 65

    conflict = old.run("lock", check=False)
    assert conflict.returncode == 78 and "conflicting versions" in conflict.stderr, (
        conflict.stderr
    )
    assert _lock_path(project_dir).read_bytes() == left


def _cut_bundle_rows(project_dir: Path) -> None:
    """Cut every `[[bundle]]` table, as a lock written before member lists."""
    kept, cutting = [], False
    for line in _lock_path(project_dir).read_text().splitlines(keepends=True):
        if line.startswith("["):
            cutting = line.startswith(("[[bundle]]", "[[bundle."))
        if not cutting:
            kept.append(line)
    _lock_path(project_dir).write_text("".join(kept))


def test_current_lock_keeps_a_member_old_remove_left_with_no_list(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """0.15.1 proves the hash of a lock with no member list after a `remove`.

    The lock names no bundle and no edge, yet its pin keeps its digest and gains
    the tag the bundle lists, which is the stamp clause on its widest reach.
    """
    member = _skill(unique_repo, "member", "one")
    leaving = _skill(unique_repo, "leaving", "one")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "member", member.fq)], tag="1")
    write_config(project_dir, skills={"leaving": leaving.fq}, bundles={"pack": pack.fq})
    old.run("lock")
    _cut_bundle_rows(project_dir)
    old.run("remove", "skill", "leaving")
    assert "bundle" not in tomllib.loads(_lock_path(project_dir).read_text())
    retag(f"{unique_repo}/member", "keep", member.digest)
    moved = _skill(unique_repo, "member", "two")
    assert moved.digest != member.digest

    result = current.run("lock", check=False)

    assert result.returncode == 0, result.stderr
    assert _pins(project_dir)["member"] == (
        f"{REGISTRY_HOST}/{unique_repo}/member:1@{member.digest}"
    )
    assert "no tag recorded" not in result.stderr, result.stderr


def _dependent_on(
    repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}
    return make_dependent(f"{repo}/{name}", "1", "skill", files, requires)


def _exits_on_copies(
    binary: Path, project: Path, home: Path, scratch: Path
) -> list[int]:
    """`status`, `install` and `install --offline`, each on its own copy of both.

    An online install stores a dependent's requires layer, which a later offline
    install would then read, so no command sees another's store writes.
    """
    codes = []
    for index, args in enumerate((("status",), ("install",), ("install", "--offline"))):
        clone = scratch / f"{index}-project"
        shutil.copytree(project, clone)
        clone_home = scratch / f"{index}-home"
        shutil.copytree(home, clone_home)
        codes.append(
            GrimRunner(binary, clone_home, cwd=clone).run(*args, check=False).returncode
        )
    return codes


def _names_unwalked(stderr: str, asker: str) -> bool:
    """Whether the edit named a row it dropped beside `asker` and the remedy."""
    return any(
        f"{asker} records no dependency edges" in line
        and "run `grim lock` online once before such an edit" in line
        for line in stderr.splitlines()
    )


def _edit_beside_p(
    edit: str, old_bin: Path, current: GrimRunner, project_dir: Path, repo: str
) -> tuple[PublishedArtifact, tuple[str, ...]]:
    """0.15.1 locks `m`'s line, or `pack`'s, then adds `p`, which requires `m`."""
    old = GrimRunner(old_bin, current.grim_home, cwd=project_dir)
    m = _skill(repo, "m", "one")
    p = _dependent_on(repo, "p", [("skill", "m", m.fq)])
    if edit == "remove-bundle":
        pack = make_bundle(f"{repo}/pack", [("skill", "m", m.fq)], tag="1")
        write_config(project_dir, bundles={"pack": pack.fq})
        args = ("remove", "bundle", "pack")
    else:
        write_config(project_dir, skills={"m": m.fq})
        args = ("uninstall" if edit == "uninstall" else "remove", "skill", "m")
    old.run("lock")
    if edit == "remove-bundle":
        old.run("install")
    old.run("add", "--no-install", "--name", "p", p.fq)
    return m, args


@pytest.mark.parametrize("edit", ["remove", "uninstall", "remove-bundle"])
def test_an_edit_beside_a_stored_requires_layer_drops_what_old_grim_drops(
    edit: str,
    old_bin: Path,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """0.15.1 adds `p` with no edge, and another project stores `p`'s requires layer.

    Both binaries share one `GRIM_HOME`, as on one machine. Each edits its own copy
    and drops `m`, as 0.15.1 reads `p`, and the current edit names `p`.
    Found by review: kept for `p`, the stale lock changed `grim install --offline`
    and offline `grim update` exits against 0.15.1. The next `grim
    lock` resolves `m` afresh and its pin moves, the recorded cost.
    """
    m, args = _edit_beside_p(edit, old_bin, current, project_dir, unique_repo)
    p = f"{REGISTRY_HOST}/{unique_repo}/p:1"
    other = tmp_path / "other"
    (other / ".claude").mkdir(parents=True)
    write_config(other)
    GrimRunner(current.binary, current.grim_home, cwd=other).run(
        "add", "--no-install", "--name", "p", p
    )

    def edited(binary: Path, name: str) -> tuple[Path, Path, str]:
        project = shutil.copytree(project_dir, tmp_path / name / "project")
        home = shutil.copytree(current.grim_home, tmp_path / name / "grim-home")
        return project, home, GrimRunner(binary, home, cwd=project).run(*args).stderr

    own, own_home, _ = edited(old_bin, "old")
    project, home, stderr = edited(current.binary, "current")

    by_old = _exits_on_copies(old_bin, own, own_home, tmp_path / "old")
    assert by_old == [0, 65, 81]
    # `grim install` checks `p`'s stored requires layer, offline too.
    mine = _exits_on_copies(current.binary, project, home, tmp_path / "current")
    assert mine == [0, 65, 65]
    assert _exits_on_copies(old_bin, project, home, tmp_path / "old-reads") == by_old
    assert "m" not in _pins(own)
    assert "m" not in _pins(project)
    assert _names_unwalked(stderr, "skill 'p'"), stderr
    retag(f"{unique_repo}/m", "keep", m.digest)
    moved = _skill(unique_repo, "m", "two")
    assert moved.digest != m.digest
    GrimRunner(current.binary, home, cwd=project).run("lock")
    assert _pins(project)["m"] == f"{REGISTRY_HOST}/{unique_repo}/m:1@{moved.digest}"


@pytest.mark.parametrize("edit", ["remove", "fetched", "uninstall", "remove-bundle"])
def test_an_edit_past_a_requires_layer_the_store_lacks_exits_as_old_grim_does(
    edit: str,
    old_bin: Path,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """0.15.1 adds `p`, which requires `m`, and stores its manifest and no requires layer.

    `grim fetch` stores `p`'s content and no requires layer either. Each binary edits
    its own copy of the project and the shared home and drops `m`, as 0.15.1 reads
    `p`. Found by review: the current edit kept `m` and left the lock stale, so
    `grim install --offline` exited 65 where 0.15.1 exits 81. The edit
    names `p`, whose manifest the store holds. The next `grim lock` resolves `m`
    afresh and its pin moves, the recorded cost.
    """
    m, args = _edit_beside_p(
        "remove" if edit == "fetched" else edit,
        old_bin,
        current,
        project_dir,
        unique_repo,
    )
    if edit == "fetched":
        current.run("fetch", f"{REGISTRY_HOST}/{unique_repo}/p:1")

    def edited(binary: Path, name: str) -> tuple[Path, Path, str]:
        project = shutil.copytree(project_dir, tmp_path / name / "project")
        home = shutil.copytree(current.grim_home, tmp_path / name / "grim-home")
        return project, home, GrimRunner(binary, home, cwd=project).run(*args).stderr

    own, own_home, _ = edited(old_bin, "old")
    project, home, stderr = edited(current.binary, "current")

    by_old = _exits_on_copies(old_bin, own, own_home, tmp_path / "old")
    assert by_old == [0, 65, 81]
    assert (
        _exits_on_copies(current.binary, project, home, tmp_path / "current") == by_old
    )
    assert _exits_on_copies(old_bin, project, home, tmp_path / "old-reads") == by_old
    assert "m" not in _pins(own)
    assert "m" not in _pins(project)
    assert _names_unwalked(stderr, "skill 'p'"), stderr
    retag(f"{unique_repo}/m", "keep", m.digest)
    moved = _skill(unique_repo, "m", "two")
    assert moved.digest != m.digest
    GrimRunner(current.binary, home, cwd=project).run("lock")
    assert _pins(project)["m"] == f"{REGISTRY_HOST}/{unique_repo}/m:1@{moved.digest}"


def _evict_manifest(grim_home: Path, digest: str) -> None:
    """Delete one manifest from the store, as a pruned or partly copied home lacks it."""
    algorithm, hexdigest = digest.split(":")
    blob = grim_home / "blobs" / algorithm / hexdigest[:2] / hexdigest
    assert blob.is_file(), blob
    blob.unlink()


def _clone_install(binary: Path, project_dir: Path, clone: Path) -> int:
    """A teammate's `grim install` of the project's two files, on a home of its own."""
    _copied(project_dir, clone)
    (clone / ".claude").mkdir()
    home = clone.parent / f"{clone.name}-home"
    home.mkdir()
    return GrimRunner(binary, home, cwd=clone).run("install", check=False).returncode


@pytest.mark.parametrize("line", ["skill", "bundle"])
@pytest.mark.parametrize("home", ["empty", "old-lock", "evicted"])
def test_a_line_deleted_by_hand_stays_gone_after_an_offline_lock(
    home: str,
    line: str,
    old_bin: Path,
    current: GrimRunner,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """A line deleted by hand, an offline `grim lock`, then a teammate's `grim install`.

    The store lacks `a`'s manifest: an empty `GRIM_HOME`, the home of a 0.15.1
    `grim lock`, which stores none, or a home with that one manifest evicted.
    Found by review: the current offline lock kept the deleted line's row under
    the declaration's hash, and the clone's install brought it back at exit 0.
    It now drops the row, as 0.15.1 does, and each binary runs the sequence on
    its own. The exits agree but over an empty home, where 0.15.1's offline lock
    needs a tag and exits 69, then its install refuses the stale lock with 65,
    and the current grim carries the pins and exits 0 twice.
    """
    a = _skill(unique_repo, "a", "a")
    if line == "skill":
        declared = {"skills": {"a": a.fq, "b": _skill(unique_repo, "b", "b").fq}}
        rows, output = ("b",), ".claude/skills/b/SKILL.md"
    else:
        member = _skill(unique_repo, "bm", "bm")
        pack = make_bundle(f"{unique_repo}/pack", [("skill", "bm", member.fq)], tag="1")
        declared = {"skills": {"a": a.fq}, "bundles": {"pack": pack.fq}}
        rows, output = ("pack", "bm"), ".claude/skills/bm/SKILL.md"

    def sequence(binary: Path, name: str) -> tuple[list[int], Path]:
        root = tmp_path / name
        project, own = root / "project", root / "grim-home"
        (project / ".claude").mkdir(parents=True)
        own.mkdir()
        write_config(project, **declared)
        writer = GrimRunner(old_bin if home == "old-lock" else binary, own, cwd=project)
        writer.run("lock")
        if home != "old-lock":
            writer.run("install")
        write_config(project, skills={"a": a.fq})
        relocking = own
        if home == "empty":
            relocking = root / "empty-home"
            relocking.mkdir()
        elif home == "evicted":
            _evict_manifest(own, a.digest)
        offline = GrimRunner(binary, relocking, cwd=project).run(
            "lock", "--offline", check=False
        )
        clone = root / "clone"
        exits = [offline.returncode, _clone_install(binary, project, clone)]
        assert not (clone / output).exists(), (
            f"{name} installed the deleted line: {exits}"
        )
        return exits, project

    old_exits, _ = sequence(old_bin, "old")
    current_exits, project = sequence(current.binary, "current")

    assert old_exits == ([69, 65] if home == "empty" else [0, 0])
    assert current_exits == ([0, 0] if home == "empty" else old_exits)
    relocked = _lock_path(project).read_text(encoding="utf-8")
    assert not set(rows) & set(_pins(project)), relocked
    reader = tmp_path / "old-reads"
    assert _clone_install(old_bin, project, reader) == 0
    assert not (reader / output).exists()


def test_an_offline_lock_past_an_unread_requires_layer_exits_as_old_grim_does(
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """Both binaries read an offline lock that could not read a dependent's requires.

    0.15.1 locks `a` and `pack` and adds `p`, which requires `a`; its `add` stores
    `p`'s manifest and not the requires layer. `pack`'s line is deleted by hand, and
    the current offline `grim lock` drops `pack` and `m`, as 0.15.1's own offline
    lock on a copy of that home drops them. Found by review: it held them under a
    hash no declaration produces, so `grim install --offline` exited 65 on both
    binaries where 0.15.1's lock gives 81; held under the declaration's hash,
    0.15.1's install wrote `m`. Each command runs on a fresh home of its own, since
    an online install stores `p`'s requires layer.
    """
    a = _skill(unique_repo, "a", "a")
    m = _skill(unique_repo, "m", "m")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "m", m.fq)], tag="1")
    p = make_dependent(
        f"{unique_repo}/p",
        "1",
        "skill",
        {"p/SKILL.md": "---\nname: p\ndescription: d\n---\n# p\n"},
        [("skill", "a", a.fq)],
    )
    write_config(project_dir, skills={"a": a.fq}, bundles={"pack": pack.fq})

    def exits(role: str, binary: Path, project: Path) -> list[int]:
        runs = (("status",), ("install",), ("install", "--offline"))
        codes = []
        for index, args in enumerate(runs):
            # By role, never file name: CI's old and current binaries are both `grim`,
            # and a reused clone dir fails the run with FileExistsError.
            clone = _copied(project, tmp_path / f"{role}-{project.name}-{index}")
            (clone / ".claude").mkdir()
            home = clone.parent / f"{clone.name}-home"
            home.mkdir()
            run = GrimRunner(binary, home, cwd=clone).run(*args, check=False)
            codes.append(run.returncode)
            assert not (clone / ".claude/skills/m/SKILL.md").exists(), (args, clone)
        return codes

    old.run("lock")
    old.run("add", "--no-install", "--name", "p", p.fq)
    write_config(project_dir, skills={"a": a.fq, "p": p.fq})
    own_dir = _copied(project_dir, tmp_path / "own")
    own_home = tmp_path / "own-home"
    shutil.copytree(old.grim_home, own_home)
    by_old = GrimRunner(old.binary, own_home, cwd=own_dir).run(
        "lock", "--offline", check=False
    )
    assert by_old.returncode == 0, by_old.stderr
    assert set(_pins(own_dir)) == {"a", "p"}

    offline = GrimRunner(current.binary, old.grim_home, cwd=project_dir).run(
        "lock", "--offline", check=False
    )

    assert offline.returncode == 0, offline.stderr
    relocked = _lock_path(project_dir).read_text(encoding="utf-8")
    assert set(_pins(project_dir)) == {"a", "p"}, relocked
    own_exits = exits("old", old.binary, own_dir)
    assert own_exits == [0, 65, 81]
    assert exits("old", old.binary, project_dir) == own_exits, relocked
    assert exits("current", current.binary, project_dir) == own_exits, relocked


def _delete_tag(repo: str, tag: str) -> None:
    """Delete `tag`, as a publisher withdraws one; a manifest another tag names stays."""
    url = f"{REGISTRY_BASE}/v2/{repo}/manifests/{tag}"
    request = urllib.request.Request(url, method="DELETE")
    with urllib.request.urlopen(request) as response:
        assert response.status == 202, response.status


def _copied(project_dir: Path, target: Path) -> Path:
    """`target` holding the project's config and lock, byte for byte."""
    target.mkdir()
    for name in ("grimoire.toml", "grimoire.lock"):
        (target / name).write_bytes((project_dir / name).read_bytes())
    return target


@pytest.mark.parametrize("offline", [False, True], ids=["online", "offline"])
def test_current_add_keeps_a_member_pin_whose_tag_is_gone(
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
    offline: bool,
) -> None:
    """A line typed at a member's tag keeps the pin the bundle's edge stamps, tag gone.

    0.15.1 writes a `[[bundle]]` row; cut, the lock is one written before member
    lists, where only the bundle's listed edge vouches for `x`'s tagless pin. `x:1`
    is deleted while its manifest stays by digest, and offline the current grim's
    tag cache never held it, since `grim lock` carries `x`. 0.15.1's `grim add`
    resolves the line and refuses with 79, writing nothing; offline it reads no
    bundle manifest from the store and exits 69 before it reaches `x`. Found by
    review: the current add resolved the line before reading the edge onto `x`,
    and refused too.
    """
    x = _skill(unique_repo, "x", "one")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", x.fq)], tag="1")
    write_config(project_dir, bundles={"pack": pack.fq})
    old.run("lock")
    _cut_bundle_rows(project_dir)
    assert _pins(project_dir) == {"x": f"{REGISTRY_HOST}/{unique_repo}/x@{x.digest}"}
    retag(f"{unique_repo}/x", "keep", x.digest)
    _delete_tag(f"{unique_repo}/x", "1")
    warm_dir = _copied(project_dir, tmp_path / "warm")
    GrimRunner(current.binary, current.grim_home, cwd=warm_dir).run("lock")
    old_dir = _copied(project_dir, tmp_path / "old-project")
    old_home = current.grim_home if offline else old.grim_home
    old_add = GrimRunner(old.binary, old_home, cwd=old_dir)
    flags = ["--offline"] if offline else []
    line = ("add", *flags, "--kind", "skill", "--no-install", x.fq)
    files = ("grimoire.toml", "grimoire.lock")
    before = {name: (old_dir / name).read_bytes() for name in files}

    refused = old_add.run(*line, check=False)
    added = current.run(*line, check=False)

    assert refused.returncode == (69 if offline else 79), refused.stderr
    assert {name: (old_dir / name).read_bytes() for name in before} == before
    assert added.returncode == 0, added.stderr
    assert _pins(project_dir)["x"] == f"{REGISTRY_HOST}/{unique_repo}/x:1@{x.digest}"
