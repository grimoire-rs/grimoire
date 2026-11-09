# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""A current lock edit on a lock grim 0.15.1 wrote beside a dependent.

0.15.1 reads no `requires` layer, so it locks a dependent with no edge. The
current edit reads such a dependent as requiring nothing, as 0.15.1 does,
whatever the store holds, and `grim install` checks a stored requires layer.
Where the store holds the dependent's manifest, the edit names it beside each
row it drops. One online `grim lock` before the edit records the dependent's
edges, and the edit then keeps the pins they reach.

The old binary comes from `GRIM_OLD_BIN`, a build of tag `v0.15.1`, and the
module skips when it is unset. Both binaries share one `GRIM_HOME`, as on one
machine.
"""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest
import tomllib

from src.helpers import (
    make_artifact,
    make_bundle,
    make_dependent,
    make_hook,
    write_config,
)
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
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


def _dependent(
    repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}
    return make_dependent(f"{repo}/{name}", "1", "skill", files, requires)


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


@pytest.fixture()
def current(grim_at, project_dir: Path, registry: str) -> GrimRunner:
    """The grim under test, in the project."""
    return grim_at(project_dir)


@pytest.fixture()
def old(old_bin: Path, current: GrimRunner, project_dir: Path) -> GrimRunner:
    """The old grim in the same project and on the same `GRIM_HOME`."""
    return GrimRunner(old_bin, current.grim_home, cwd=project_dir)


def _store_requires(
    current: GrimRunner, tmp_path: Path, dependent: PublishedArtifact
) -> None:
    """Another project's `grim add` on the shared home stores `dependent`'s requires layer."""
    other = tmp_path / f"other-{dependent.repo.rsplit('/', 1)[-1]}"
    (other / ".claude").mkdir(parents=True)
    write_config(other)
    name = dependent.repo.rsplit("/", 1)[-1]
    GrimRunner(current.binary, current.grim_home, cwd=other).run(
        "add", "--no-install", "--name", name, dependent.fq
    )


def _names_unwalked(stderr: str, dropped: str, asker: str) -> bool:
    """Whether the edit named `dropped` beside `asker` and the remedy."""
    return any(
        dropped in line
        and f"{asker} records no dependency edges" in line
        and "run `grim lock` online once before such an edit" in line
        for line in stderr.splitlines()
    )


def _pins(project_dir: Path) -> dict[str, str]:
    lock = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    return {
        entry["name"]: entry["pinned"]
        for kind in ("skill", "bundle")
        for entry in lock.get(kind, [])
    }


def test_current_remove_of_a_bundle_drops_the_member_an_old_grim_dependent_requires(
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """0.15.1 locks `pack:1`, which lists `m:1`, then adds `p`, which requires `m:1`.

    Another project stores `p`'s requires layer. Each binary removes the bundle on
    its own copy, and the current one drops `m` and names `p` beside `m` alone, which
    is all that layer reaches. Found by review: kept
    for `p`, the stale lock changed `grim install --offline` and offline `grim
    update` exits against 0.15.1. The next `grim lock` resolves `m`
    afresh and its pin moves, the recorded cost.
    """
    m = _skill(unique_repo, "m", "one")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "m", m.fq)], tag="1")
    p = _dependent(unique_repo, "p", [("skill", "m", m.fq)])
    write_config(project_dir, bundles={"pack": pack.fq})
    old.run("lock")
    old.run("install")
    old.run("add", "--no-install", "--name", "p", p.fq)
    _store_requires(current, tmp_path, p)
    own = shutil.copytree(project_dir, tmp_path / "old" / "project")
    own_home = shutil.copytree(current.grim_home, tmp_path / "old" / "grim-home")
    GrimRunner(old.binary, own_home, cwd=own).run("remove", "bundle", "pack")

    edit = current.run("remove", "bundle", "pack")

    assert "m" not in _pins(own)
    assert "m" not in _pins(project_dir)
    assert _names_unwalked(edit.stderr, "skill 'm' leaves the lock", "skill 'p'"), (
        edit.stderr
    )
    by_old = _exits_on_copies(old.binary, own, own_home, tmp_path / "old-own")
    assert by_old == [0, 65, 81]
    home = current.grim_home
    # `grim install` checks `p`'s stored requires layer, offline too.
    assert _exits_on_copies(current.binary, project_dir, home, tmp_path / "mine") == [
        0,
        65,
        65,
    ]
    assert _exits_on_copies(old.binary, project_dir, home, tmp_path / "reads") == by_old
    retag(f"{unique_repo}/m", "keep", m.digest)
    moved = _skill(unique_repo, "m", "two")
    assert moved.digest != m.digest

    current.run("lock")

    assert (
        _pins(project_dir)["m"] == f"{REGISTRY_HOST}/{unique_repo}/m:1@{moved.digest}"
    )


def test_current_remove_of_a_bundle_an_old_grim_dependent_requires_drops_its_row(
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """0.15.1 locks `q`, which requires bundle `pack:2`, beside `pack`'s own line.

    Another project stores `q`'s requires layer. The current `grim remove bundle
    pack` drops the bundle's row, as 0.15.1 does, names `q`, and 0.15.1 still reads
    the lock. The next `grim lock` reads `q`'s edge and resolves the moved `pack:2`
    afresh, the recorded cost.
    """
    n = _skill(unique_repo, "n", "one")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "n", n.fq)], tag="2")
    q = _dependent(unique_repo, "q", [("bundle", "pack", pack.fq)])
    write_config(project_dir, skills={"q": q.fq}, bundles={"pack": pack.fq})
    old.run("lock")
    _store_requires(current, tmp_path, q)

    edit = current.run("remove", "bundle", "pack")

    assert "pack" not in _pins(project_dir)
    assert _names_unwalked(edit.stderr, "bundle 'pack'", "skill 'q'"), edit.stderr
    assert old.run("status", check=False).returncode == 0
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    m = _skill(unique_repo, "m", "one")
    moved = make_bundle(
        f"{unique_repo}/pack", [("skill", "n", n.fq), ("skill", "m", m.fq)], tag="2"
    )
    assert moved.digest != pack.digest

    relock = current.run("lock")

    assert moved.digest in _pins(project_dir)["pack"], relock.stderr


@pytest.mark.parametrize(
    "edit",
    [
        ("remove", "skill", "m"),
        ("uninstall", "skill", "m"),
        ("remove", "bundle", "pack"),
    ],
    ids=["remove", "uninstall", "remove-bundle"],
)
def test_an_online_lock_before_the_edit_keeps_the_pin_an_old_grim_dependent_requires(
    edit: tuple[str, str, str],
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    unique_repo: str,
) -> None:
    """The upgrade remedy: 0.15.1 locks `p`, which requires `m:1`, with no edge.

    The current online `grim lock` records `p`'s edge, so the edit keeps `m` and
    names no dependent, and the next `grim lock` keeps `m`'s pin after its tag
    moved.
    """
    m = _skill(unique_repo, "m", "one")
    p = _dependent(unique_repo, "p", [("skill", "m", m.fq)])
    if edit[1] == "bundle":
        pack = make_bundle(f"{unique_repo}/pack", [("skill", "m", m.fq)], tag="1")
        write_config(project_dir, bundles={"pack": pack.fq})
    else:
        write_config(project_dir, skills={"m": m.fq})
    old.run("lock")
    old.run("install")
    old.run("add", "--no-install", "--name", "p", p.fq)
    current.run("lock")

    removed = current.run(*edit)

    kept = f"{REGISTRY_HOST}/{unique_repo}/m:1@{m.digest}"
    assert _pins(project_dir)["m"] == kept
    assert "no dependency edges" not in removed.stderr, removed.stderr
    retag(f"{unique_repo}/m", "keep", m.digest)
    assert _skill(unique_repo, "m", "two").digest != m.digest

    relock = current.run("lock")

    assert _pins(project_dir)["m"] == kept, relock.stderr


def _exits_on_copies(
    binary: Path, project: Path, home: Path, scratch: Path
) -> list[int]:
    """`status`, `install` and `install --offline`, each on its own copy of both.

    An online install stores a dependent's requires layer, which a later offline
    install would then read, so no command sees another's store writes.
    """
    codes = []
    for index, args in enumerate((("status",), ("install",), ("install", "--offline"))):
        clone = shutil.copytree(project, scratch / f"{index}-project")
        clone_home = shutil.copytree(home, scratch / f"{index}-home")
        run = GrimRunner(binary, clone_home, cwd=clone).run(*args, check=False)
        codes.append(run.returncode)
    return codes


@pytest.mark.parametrize("layer", ["stored", "missing"])
@pytest.mark.parametrize(
    "edit",
    [
        ("remove", "skill", "m"),
        ("uninstall", "skill", "m"),
        ("remove", "bundle", "pack"),
    ],
    ids=["remove", "uninstall", "remove-bundle"],
)
def test_an_edit_after_an_offline_lock_drops_beside_each_row_that_records_no_edge(
    edit: tuple[str, str, str],
    layer: str,
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """0.15.1 adds `p`, which requires `m:1`, and `r`, which requires `z:1`.

    Another project's `grim add` stores `r`'s requires layer, and the current
    offline `grim lock` records `r`'s edge and none for `p`, whose layer is missing.
    Another project may store `p`'s layer after that lock. Either way the edit
    drops `m`, as 0.15.1 reads `p`, and names `p`. `status`, `install`
    and `install --offline` exit as after 0.15.1's own offline lock and edit where
    the layer is missing; where it is stored, `install` checks it, offline too. Found
    by review: kept, `install --offline` exited 65 where 0.15.1 exits 81, and
    offline `grim update` exits changed against both. The next `grim lock` resolves
    `m` afresh and its pin moves, the recorded cost.
    """
    m = _skill(unique_repo, "m", "one")
    z = _skill(unique_repo, "z", "z")
    p = _dependent(unique_repo, "p", [("skill", "m", m.fq)])
    r = _dependent(unique_repo, "r", [("skill", "z", z.fq)])
    if edit[1] == "bundle":
        pack = make_bundle(f"{unique_repo}/pack", [("skill", "m", m.fq)], tag="1")
        write_config(project_dir, skills={"z": z.fq}, bundles={"pack": pack.fq})
    else:
        write_config(project_dir, skills={"m": m.fq, "z": z.fq})
    old.run("lock")
    old.run("add", "--no-install", "--name", "p", p.fq)
    old.run("add", "--no-install", "--name", "r", r.fq)
    _store_requires(current, tmp_path, r)
    own = shutil.copytree(project_dir, tmp_path / "old" / "project")
    own_home = shutil.copytree(current.grim_home, tmp_path / "old" / "grim-home")
    current.run("lock", "--offline")
    rows = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    requires = {row["name"]: row.get("requires", []) for row in rows["skill"]}
    assert [edge["name"] for edge in requires["r"]] == ["z"], rows
    assert requires["p"] == [], rows
    if layer == "stored":
        _store_requires(current, tmp_path, p)

    edited = current.run(*edit)

    old_runner = GrimRunner(old.binary, own_home, cwd=own)
    old_runner.run("lock", "--offline", check=False)
    old_runner.run(*edit)
    by_old = _exits_on_copies(old.binary, own, own_home, tmp_path / "old")
    assert by_old == [0, 65, 81]
    mine = _exits_on_copies(
        current.binary, project_dir, current.grim_home, tmp_path / "current"
    )
    # Where the layer is stored, `grim install` checks it, offline too.
    assert mine == (by_old if layer == "missing" else [0, 65, 65])
    assert "m" not in _pins(own)
    assert "m" not in _pins(project_dir)
    # A missing layer may reach the bundle too; a stored one shows it reaches `m` alone.
    whole = edit[1] == "bundle" and layer == "missing"
    dropped = "skill 'm' and bundle 'pack'" if whole else "skill 'm' leaves the lock"
    assert _names_unwalked(edited.stderr, dropped, "skill 'p'"), edited.stderr
    retag(f"{unique_repo}/m", "keep", m.digest)
    moved = _skill(unique_repo, "m", "two")
    assert moved.digest != m.digest

    relock = current.run("lock")

    after = f"{REGISTRY_HOST}/{unique_repo}/m:1@{moved.digest}"
    assert _pins(project_dir)["m"] == after, relock.stderr


def test_an_uninstall_beside_an_old_grim_dependent_names_no_lock_for_a_gatekeeper(
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    tmp_path: Path,
    unique_repo: str,
) -> None:
    """0.15.1 locks `m` and a gatekeeper hook, then adds `p`, which requires `m:1`.

    With `p`'s requires layer stored and the gatekeeper's client registration
    deleted, `grim uninstall skill m` names `p`: the next `grim lock` may move
    `m`'s pin. The gatekeeper's warning names `grim install` and no `grim lock`.
    Found by review: it asked for that `grim lock` first.
    """
    m = _skill(unique_repo, "m", "one")
    p = _dependent(unique_repo, "p", [("skill", "m", m.fq)])
    gate = make_hook(f"{unique_repo}/gate", "gate", tag="1")
    write_config(project_dir, skills={"m": m.fq}, hooks={"gate": gate.fq})
    current.run("config", "set", "options.experimental.hooks", "true")
    old.run("lock")
    current.run("hook", "allow")
    current.run("install")
    old.run("add", "--no-install", "--name", "p", p.fq)
    _store_requires(current, tmp_path, p)
    (project_dir / ".claude" / "settings.local.json").unlink()

    edit = current.run("uninstall", "skill", "m")

    assert _names_unwalked(edit.stderr, "skill 'm' leaves the lock", "skill 'p'"), (
        edit.stderr
    )
    lines = edit.stderr.splitlines()
    warning = [line for line in lines if "no client registration" in line]
    assert len(warning) == 1, edit.stderr
    assert "hook 'gate'" in warning[0], warning[0]
    assert "`grim install` restores it" in warning[0], warning[0]
    assert "grim lock" not in warning[0], warning[0]
