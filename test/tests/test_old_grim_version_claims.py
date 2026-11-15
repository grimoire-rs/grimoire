# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""How grim 0.15.1 and the current grim share files once a tag is a version claim.

A numeric `requires` tag now claims a version range; a bundle member's tag does
not. The lock and config formats are unchanged: `lock_version` stays 1 and
`pinned` reads `registry/repo:tag@sha256:…`, the tag advisory to 0.15.1. So a
0.15.1 lock reads and installs unchanged, a relock leaves it byte-identical but
for the advisory tags, and 0.15.1 installs a lock the current grim wrote once no
row records an edge. The one break is the documented one: two major tags
naming one manifest, which 0.15.1 locked, now refuse with 78.

The old binary comes from `GRIM_OLD_BIN`, a build of tag `v0.15.1`, and the
module skips when it is unset. `GRIM_COMMAND` is the current grim. Each side
keeps its own `GRIM_HOME`, so the old grim reaches a pin through the registry.
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
    publish_lie,
    publish_release,
    write_config,
)
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner
from tools.p9_diff import strip_pinned_tags

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

_PINNED = re.compile(r"[^@:/]+(?::\d+)?/[^@]+:[^@:/]+@sha256:[0-9a-f]{64}")


def _files(name: str, body: str | None = None) -> dict[str, str]:
    text = f"---\nname: {name}\ndescription: d\n---\n# {body or name}\n"
    return {f"{name}/SKILL.md": text}


def _ref(repo: str, tag: str | None = None) -> str:
    return f"{REGISTRY_HOST}/{repo}" + (f":{tag}" if tag else "")


def _dependent(
    repo: str, name: str, requires: list[tuple[str, str, str]]
) -> PublishedArtifact:
    return make_dependent(f"{repo}/{name}", "1", "skill", _files(name), requires)


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


def _lock(project_dir: Path) -> dict:
    return tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))


def _pins(project_dir: Path) -> dict[str, str]:
    return {
        entry["name"]: entry["pinned"] for entry in _lock(project_dir).get("skill", [])
    }


def _files_of(project_dir: Path) -> dict[str, bytes]:
    return {
        name: (project_dir / name).read_bytes()
        for name in ("grimoire.toml", "grimoire.lock")
    }


def _body(project_dir: Path, name: str) -> str:
    path = project_dir / ".claude" / "skills" / name / "SKILL.md"
    return path.read_text(encoding="utf-8").split()[-1]


def test_an_old_lock_with_a_numeric_member_tag_installs_and_relocks_unchanged(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """0.15.1 locks `d:2` and `pack`, whose member edge names `m:2`.

    `m:2`'s manifest is annotated 3.1.0. A member's tag claims no range, so the
    annotation does not count against it: the current grim installs the lock as
    it stands, and its relock changes no byte but the advisory tags it records.
    """
    d = publish_release(f"{unique_repo}/d", "2.4.0", _files("d"))
    m = publish_lie(f"{unique_repo}/m", "2", "3.1.0", _files("m"))
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "m", m.fq)], tag="1")
    write_config(
        project_dir, skills={"d": _ref(d.repo, "2")}, bundles={"pack": pack.fq}
    )
    old.run("lock")
    written = _files_of(project_dir)
    assert "requires" not in written["grimoire.lock"].decode()
    assert _pins(project_dir) == {
        "d": f"{_ref(d.repo)}@{d.digest}",
        "m": f"{_ref(m.repo)}@{m.digest}",
    }

    current.run("install")

    assert _files_of(project_dir) == written
    assert _body(project_dir, "d") == "d" and _body(project_dir, "m") == "m"

    result = current.run("lock", format="json")

    assert {row["action"] for row in json.loads(result.stdout)["items"]} == {
        "unchanged"
    }
    relocked = (project_dir / "grimoire.lock").read_bytes()
    assert strip_pinned_tags(relocked) == written["grimoire.lock"]
    assert _pins(project_dir) == {
        "d": f"{_ref(d.repo, '2')}@{d.digest}",
        "m": f"{_ref(m.repo, '2')}@{m.digest}",
    }
    assert _lock(project_dir)["metadata"]["lock_version"] == 1


@pytest.mark.parametrize("first", ["requires", "member"])
def test_two_majors_naming_one_manifest_refuse_the_relock_of_an_old_lock(
    old: GrimRunner,
    current: GrimRunner,
    project_dir: Path,
    unique_repo: str,
    first: str,
) -> None:
    """One manifest of `x` carries the tags `1` and `2`; `q` requires `x:2`.

    `x:1` reaches `x` through `p`'s edge or `pack`'s member list. 0.15.1 reads no
    `requires` layer and locks at exit 0, holding `x` where the member names it.
    The current grim reads `x:2` as a claim on 2.x that the tag `1` cannot show,
    so its relock refuses with 78 and leaves both files as 0.15.1 wrote them.
    """
    x = make_artifact(f"{unique_repo}/x", "skill", _files("x"), tag="1")
    retag(x.repo, "2", x.digest)
    q = _dependent(unique_repo, "q", [("skill", "x", _ref(x.repo, "2"))])
    if first == "requires":
        p = _dependent(unique_repo, "p", [("skill", "x", x.fq)])
        write_config(project_dir, skills={"p": p.fq, "q": q.fq})
        asker = f"{x.fq} required by skill/p"
    else:
        pack = make_bundle(f"{unique_repo}/pack", [("skill", "x", x.fq)], tag="1")
        write_config(project_dir, skills={"q": q.fq}, bundles={"pack": pack.fq})
        asker = f"{x.fq} from bundle {pack.fq}"

    old.run("lock")
    assert ("x" in _pins(project_dir)) == (first == "member")
    written = _files_of(project_dir)

    refused = current.run("lock", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "conflicting versions" in refused.stderr, refused.stderr
    assert asker in refused.stderr, refused.stderr
    assert f"{_ref(x.repo, '2')} required by skill/q" in refused.stderr
    assert "declare skill x directly in grimoire.toml with a tag" in refused.stderr
    assert _files_of(project_dir) == written


def test_old_grim_installs_a_pin_whose_tag_a_claim_chose(
    old: GrimRunner, current: GrimRunner, project_dir: Path, unique_repo: str
) -> None:
    """`p` claims `x:2`; then `x` is declared bare and `p` leaves.

    The current lock records `x:2@…`, the claim's target tag, under `lock_version`
    1. With `p`'s edge in it 0.15.1 refuses the lock and leaves it. Once the bare
    line keeps that pin and no row records an edge, 0.15.1 reads the lock as
    fresh and installs 2.4.0, not the 3.0.0 `latest` names.
    """
    v240 = publish_release(f"{unique_repo}/x", "2.4.0", _files("x", "two"))
    publish_release(f"{unique_repo}/x", "3.0.0", _files("x", "three"))
    x = v240.repo
    p = _dependent(unique_repo, "p", [("skill", "x", _ref(x, "2"))])
    write_config(project_dir, skills={"p": p.fq})
    current.run("lock")
    lock = _lock(project_dir)
    pinned = f"{_ref(x, '2')}@{v240.digest}"
    assert _pins(project_dir)["x"] == pinned
    assert lock["metadata"]["lock_version"] == 1
    assert all(_PINNED.fullmatch(row["pinned"]) for row in lock["skill"]), lock
    with_edge = _files_of(project_dir)

    refused = old.run("install", check=False)

    assert refused.returncode == 78, refused.stderr
    assert "requires" in refused.stderr, refused.stderr
    assert _files_of(project_dir) == with_edge

    write_config(project_dir, skills={"x": _ref(x)})
    current.run("lock")
    assert _pins(project_dir) == {"x": pinned}
    assert "requires" not in (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    written = _files_of(project_dir)

    assert {row["state"] for row in old.json("status")["items"]} == {"missing"}
    old.run("install")

    assert _body(project_dir, "x") == "two"
    assert _files_of(project_dir) == written
