# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The remedy of a marketplace part's stale-dependency refusal.

A part 0.15.1 wrote records no dependency edges. When an export refuses it
(exit 65), the printed remedy names the pick of the stale row, not the
whole-plugin update, which would also move every sibling pin whose tag moved.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, make_dependent, printed_argv
from src.old_grim import OLD_BIN, checked_old_grim
from src.registry import PublishedArtifact, retag
from src.runner import GrimRunner


def _skill(repo: str, name: str, body: str = "v1") -> PublishedArtifact:
    files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {body}\n"}
    return make_artifact(f"{repo}/{name}", "skill", files, tag="1")


def _pins(work: Path) -> dict[str, str]:
    lock = tomllib.loads((work / "marketplace.lock").read_text(encoding="utf-8"))
    return {
        row["name"]: row["pinned"].split("@")[1] if "pinned" in row else row["hash"]
        for row in lock.get("skill", [])
    }


@pytest.fixture(scope="module")
def old_bin(tmp_path_factory):
    if not OLD_BIN:
        pytest.skip("GRIM_OLD_BIN unset")
    return checked_old_grim(tmp_path_factory.mktemp("old-grim-identity"))


def test_a_part_stale_dependency_remedy_relocks_only_the_stale_row(
    old_bin, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """`p` requires `m` and `z` is a sibling include. 0.15.1 wrote the part, then
    `z`'s tag moved. The refusal names `team:p`; running it adds `m` and keeps `z`.
    Found by review: it named the whole-plugin update, which moved `z` unsaid."""
    m = _skill(unique_repo, "m")
    files = {"p/SKILL.md": "---\nname: p\ndescription: d\n---\n# p\n"}
    p = make_dependent(f"{unique_repo}/p", "1", "skill", files, [("skill", "m", m.fq)])
    z = _skill(unique_repo, "z")
    work, home = tmp_path / "work", tmp_path / "home"
    work.mkdir()
    home.mkdir()
    (work / "marketplace.toml").write_text(
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f"[plugins.team]\ninclude = {json.dumps([p.fq, z.fq])}\n",
        encoding="utf-8",
        newline="\n",
    )
    old = GrimRunner(old_bin, home, cwd=work)
    assert old.run("update", "--marketplace", "marketplace.toml", check=False).returncode == 0
    written = _pins(work)
    retag(f"{unique_repo}/z", "keep", z.digest)
    _skill(unique_repo, "z", "v2")
    current = GrimRunner(grim_at(work).binary, home, cwd=work)
    export = ("export", "plugin", "--plugin", "team", "--client", "claude", "-o", "dist", "--force")

    refused = current.run(*export, check=False)

    assert refused.returncode == 65, refused.stderr
    printed = re.search(r"run `(grim update --marketplace [^`]+)`, which relocks only it and its dependencies", refused.stderr)
    assert printed, refused.stderr
    assert printed.group(1).endswith(" team:p"), printed.group(1)
    followed = current.run(*printed_argv(printed.group(1))[1:], check=False)
    assert followed.returncode == 0, followed.stderr
    after = _pins(work)
    assert "m" in after and "m" not in written, (written, after)
    assert after["z"] == written["z"], "the sibling's pin moved"
    assert current.run(*export, check=False).returncode == 0


def _local_skill(path: Path, name: str, body: str) -> None:
    path.mkdir(parents=True, exist_ok=True)
    (path / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: d\n---\n# {body}\n", encoding="utf-8", newline="\n"
    )


def _drifted_local_beside_a_moved_sibling(
    grim_at, work: Path, unique_repo: str
) -> tuple[GrimRunner, str, dict[str, str]]:
    """`loc` is a local include edited after the lock, `z` a sibling whose tag moved."""
    z = _skill(unique_repo, "z")
    _local_skill(work / "skills" / "loc", "loc", "v1")
    (work / "marketplace.toml").write_text(
        f"[plugins.team]\ninclude = {json.dumps(['./skills/loc', z.fq])}\n", encoding="utf-8", newline="\n"
    )
    runner = grim_at(work)
    assert runner.run("update", "--marketplace", "marketplace.toml", check=False).returncode == 0
    written = _pins(work)
    retag(f"{unique_repo}/z", "keep", z.digest)
    _skill(unique_repo, "z", "v2")
    _local_skill(work / "skills" / "loc", "loc", "v2")
    return runner, z.digest, written


def test_a_drifted_local_remedy_relocks_only_the_drifted_row(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """The export refuses 65 over `loc`'s edit. The printed command is the pick of `loc`:
    running it re-packs `loc` and leaves `z`, whose tag moved, at its pin.
    Found by review: it named the whole-plugin update, which moved `z` unsaid."""
    work = tmp_path / "work"
    work.mkdir()
    runner, _, written = _drifted_local_beside_a_moved_sibling(grim_at, work, unique_repo)
    export = ("export", "plugin", "--plugin", "team", "--client", "claude", "-o", "dist", "--force")

    refused = runner.run(*export, check=False)

    assert refused.returncode == 65, refused.stderr
    printed = re.search(
        r"run `(grim update --marketplace [^`]+)`, which relocks only it and its dependencies",
        refused.stderr,
    )
    assert printed, refused.stderr
    assert printed.group(1).endswith(" team:loc"), printed.group(1)
    followed = runner.run(*printed_argv(printed.group(1))[1:], check=False)
    assert followed.returncode == 0, followed.stderr
    after = _pins(work)
    assert after["z"] == written["z"], "the sibling's pin moved"
    assert after["loc"] != written["loc"], "the drifted row was not re-packed"
    assert runner.run(*export, check=False).returncode == 0
    skill = work / "dist" / "team.claude" / "skills" / "loc" / "SKILL.md"
    assert "# v2" in skill.read_text(encoding="utf-8")


def test_a_stale_part_pick_remedy_says_the_whole_plugin_update_moves_moved_tags(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """A pick of `a` over a part the manifest edit made stale refuses 65, and naming the
    pick again would refuse again. The whole-plugin update is the retry that works, and
    it says what it also does: `z`, whose tag moved, moves with it."""
    a = _skill(unique_repo, "a")
    z = _skill(unique_repo, "z")
    extra = _skill(unique_repo, "extra")
    work = tmp_path / "work"
    work.mkdir()
    manifest = work / "marketplace.toml"
    manifest.write_text(f"[plugins.team]\ninclude = {json.dumps([a.fq, z.fq])}\n", encoding="utf-8", newline="\n")
    runner = grim_at(work)
    assert runner.run("update", "--marketplace", "marketplace.toml", check=False).returncode == 0
    written = _pins(work)
    retag(f"{unique_repo}/z", "keep", z.digest)
    moved = _skill(unique_repo, "z", "v2")
    manifest.write_text(
        f"[plugins.team]\ninclude = {json.dumps([a.fq, z.fq, extra.fq])}\n", encoding="utf-8", newline="\n"
    )

    refused = runner.run("update", "--marketplace", "marketplace.toml", "team:a", check=False)

    assert refused.returncode == 65, refused.stderr
    printed = re.search(
        r"run `(grim update --marketplace [^`]+)`, which moves every pin of the plugin whose tag moved",
        refused.stderr,
    )
    assert printed, refused.stderr
    followed = runner.run(*printed_argv(printed.group(1))[1:], check=False)
    assert followed.returncode == 0, followed.stderr
    after = _pins(work)
    assert after["a"] == written["a"] and "extra" in after, after
    assert after["z"] == moved.digest != written["z"], "the printed update did not move z"


def test_a_stale_part_pick_remedy_refuses_with_78_where_two_bundles_then_list_different_versions(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """The retry's other half: `p2:1` moves onto a list naming `x:2` beside `p1`'s `x:1`.

    A pick over the part the manifest edit made stale refuses 65 and prints the whole-plugin
    update. The update re-resolves `p2` and refuses with 78 as it says, naming the bundles to
    drop, and the lock is left as it was.
    """
    x1 = _skill(unique_repo, "x")
    files = {"x/SKILL.md": "---\nname: x\ndescription: d\n---\n# v2\n"}
    x2 = make_artifact(f"{unique_repo}/x", "skill", files, tag="2")
    y = _skill(unique_repo, "y")
    extra = _skill(unique_repo, "extra")
    p1 = make_bundle(f"{unique_repo}/p1", [("skill", "x", x1.fq)], tag="1")
    p2 = make_bundle(f"{unique_repo}/p2", [("skill", "y", y.fq)], tag="1")
    work = tmp_path / "work"
    work.mkdir()
    manifest = work / "marketplace.toml"
    manifest.write_text(f"[plugins.team]\ninclude = {json.dumps([p1.fq, p2.fq])}\n", encoding="utf-8", newline="\n")
    runner = grim_at(work)
    assert runner.run("update", "--marketplace", "marketplace.toml", check=False).returncode == 0
    retag(f"{unique_repo}/p2", "keep", p2.digest)
    make_bundle(f"{unique_repo}/p2", [("skill", "x", x2.fq)], tag="1")
    manifest.write_text(
        f"[plugins.team]\ninclude = {json.dumps([p1.fq, p2.fq, extra.fq])}\n", encoding="utf-8", newline="\n"
    )
    lock = work / "marketplace.lock"
    written = lock.read_text(encoding="utf-8")

    refused = runner.run("update", "--marketplace", "marketplace.toml", "team:y", check=False)

    assert refused.returncode == 65, refused.stderr
    printed = re.search(
        r"run `(grim update --marketplace [^`]+)`, which moves every pin of the plugin whose tag "
        r"moved, and refuses with 78 where two bundles then list different versions",
        refused.stderr,
    )
    assert printed, refused.stderr
    followed = runner.run(*printed_argv(printed.group(1))[1:], check=False)
    assert followed.returncode == 78, followed.stderr
    assert "drop one of these bundles from plugin 'team'" in followed.stderr, followed.stderr
    assert lock.read_text(encoding="utf-8") == written


def test_a_held_requirement_remedy_names_the_pick_first_and_says_what_the_whole_plugin_update_moves(
    old_bin, grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """`pk2` lists `y`, which requires `held:1`; `pack` then lists `held:2`, and `z`'s tag moved.

    A pick of `pack` refuses 78. The remedy names the pick that takes `y` too: running it
    moves `held` and `y` and keeps `z`. It offers the whole-plugin update with what that
    also does, and running it moves `z`.
    """
    held = _skill(unique_repo, "held")
    files = {"y/SKILL.md": "---\nname: y\ndescription: d\n---\n# y\n"}
    y = make_dependent(f"{unique_repo}/y", "1", "skill", files, [("skill", "held", held.fq)])
    z = _skill(unique_repo, "z")
    pack = make_bundle(f"{unique_repo}/pack", [("skill", "held", held.fq)], tag="1")
    pk2 = make_bundle(f"{unique_repo}/pk2", [("skill", "y", y.fq)], tag="1")
    work, home = tmp_path / "work", tmp_path / "home"
    work.mkdir()
    home.mkdir()
    (work / "marketplace.toml").write_text(
        '[marketplace]\nname = "acme"\n\n[marketplace.owner]\nname = "Acme"\n\n'
        f"[plugins.team]\ninclude = {json.dumps([pack.fq, pk2.fq, z.fq])}\n",
        encoding="utf-8",
        newline="\n",
    )
    old = GrimRunner(old_bin, home, cwd=work)
    assert old.run("update", "--marketplace", "marketplace.toml", check=False).returncode == 0
    written = _pins(work)
    held_files = {"held/SKILL.md": "---\nname: held\ndescription: d\n---\n# v2\n"}
    held2 = make_artifact(f"{unique_repo}/held", "skill", held_files, tag="2")
    retag(f"{unique_repo}/pack", "keep", pack.digest)
    make_bundle(f"{unique_repo}/pack", [("skill", "held", held2.fq)], tag="1")
    retag(f"{unique_repo}/y", "keep", y.digest)
    make_dependent(f"{unique_repo}/y", "1", "skill", files, [("skill", "held", held2.fq)])
    retag(f"{unique_repo}/z", "keep", z.digest)
    moved = _skill(unique_repo, "z", "v2")
    current = GrimRunner(grim_at(work).binary, home, cwd=work)
    pick = ("update", "--marketplace", "marketplace.toml", "team:pack")

    refused = current.run(*pick, check=False)

    assert refused.returncode == 78, refused.stderr
    printed = re.search(
        r"add `(team:y)`, or run `(grim update --marketplace [^`]+)`, which moves every pin of the "
        r"plugin whose tag moved",
        refused.stderr,
    )
    assert printed, refused.stderr
    assert _pins(work) == written
    followed = current.run(*pick, printed.group(1), check=False)
    assert followed.returncode == 0, followed.stderr
    after = _pins(work)
    assert after["held"] == held2.digest and after["y"] != written["y"], after
    assert after["z"] == written["z"], "the sibling's pin moved"
    whole = current.run(*printed_argv(printed.group(2))[1:], check=False)
    assert whole.returncode == 0, whole.stderr
    assert _pins(work)["z"] == moved.digest, "the printed update did not move z"


def test_a_gone_pin_remedy_says_the_whole_plugin_update_moves_moved_tags(
    grim_at, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """`a`'s tag moved and the registry dropped the manifest its pin names; `z`'s tag moved too.

    A pick of `extra` carries `a` and refuses 79 on a machine whose store lacks that
    manifest. The printed whole-plugin update moves `a`, and says it moves `z` as well.
    """
    a = _skill(unique_repo, "a")
    z = _skill(unique_repo, "z")
    extra = _skill(unique_repo, "extra")
    work = tmp_path / "work"
    work.mkdir()
    (work / "marketplace.toml").write_text(
        f"[plugins.team]\ninclude = {json.dumps([a.fq, z.fq, extra.fq])}\n", encoding="utf-8", newline="\n"
    )
    runner = grim_at(work)
    assert runner.run("update", "--marketplace", "marketplace.toml", check=False).returncode == 0
    written = _pins(work)
    gone = _skill(unique_repo, "a", "v2")
    retag(f"{unique_repo}/z", "keep", z.digest)
    moved = _skill(unique_repo, "z", "v2")
    home = tmp_path / "elsewhere"
    home.mkdir()
    elsewhere = GrimRunner(runner.binary, home, cwd=work)

    refused = elsewhere.run("update", "--marketplace", "marketplace.toml", "team:extra", check=False)

    assert refused.returncode == 79, refused.stderr
    printed = re.search(
        r"run `(grim update --marketplace [^`]+)`, which moves every pin of the plugin whose tag "
        r"moved, this one included",
        refused.stderr,
    )
    assert printed, refused.stderr
    assert _pins(work) == written
    followed = elsewhere.run(*printed_argv(printed.group(1))[1:], check=False)
    assert followed.returncode == 0, followed.stderr
    after = _pins(work)
    assert after["a"] == gone.digest != written["a"], after
    assert after["z"] == moved.digest != written["z"], "the printed update did not move z"
