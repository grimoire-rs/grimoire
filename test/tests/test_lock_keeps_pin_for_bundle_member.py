# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""An undeclare keeps the pin of a key a bundle still provides at the same reference.

A lock written before 0.16.0 records no tag in `pinned`. Dropping the direct
declaration of a key that a declared bundle lists by the same reference must
keep its pin: only `grim update` moves one. The lock here is a current lock with
the tags cut out of `pinned`, which is what grim 0.15.1 wrote.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest
import tomllib

from src.helpers import make_artifact, make_bundle, write_config
from src.registry import REGISTRY_HOST, PublishedArtifact, retag
from src.runner import GrimRunner

_OUTPUT = {"skill": ".claude/skills/{}/SKILL.md", "rule": ".claude/rules/{}.md"}


def _publish(repo: str, kind: str, name: str, body: str, tag: str) -> PublishedArtifact:
    if kind == "skill":
        files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {body}\n"}
    else:
        files = {f"{name}.md": f"---\npaths: ['**/*.rs']\n---\n# {body}\n"}
    return make_artifact(f"{repo}/{name}", kind, files, tag=tag)


def _pin(project_dir: Path, kind: str, name: str) -> str:
    lock = tomllib.loads((project_dir / "grimoire.lock").read_text(encoding="utf-8"))
    (entry,) = [e for e in lock[kind] if e["name"] == name]
    return entry["pinned"]


def _cut_tags(project_dir: Path) -> None:
    """Rewrite `pinned = "repo:tag@sha256:…"` as `repo@sha256:…`, as 0.15.1 did."""
    path = project_dir / "grimoire.lock"
    text = path.read_text(encoding="utf-8")
    path.write_text(re.sub(r'(pinned = "[^"@]*?):[^":/@]+@', r"\1@", text), encoding="utf-8", newline="\n")


def _setup(
    grim: GrimRunner, project_dir: Path, repo: str, kind: str, listed_tag: str
) -> tuple[str, str]:
    """Declare `m` and a bundle listing it at `listed_tag`; return its old and moved digests."""
    base = f"{REGISTRY_HOST}/{repo}"
    old = _publish(repo, kind, "m", "v1", "1")
    other = _publish(repo, kind, "m", "other", "2")
    bundle = make_bundle(f"{repo}/pack", [(kind, "m", f"{base}/m:{listed_tag}")], tag="1")
    write_config(
        project_dir,
        **{f"{kind}s": {"m": old.fq}},
        bundles={"pack": bundle.fq},
    )
    grim.run("lock")
    grim.run("install")
    _cut_tags(project_dir)
    assert "@" in _pin(project_dir, kind, "m") and ":1@" not in _pin(project_dir, kind, "m")
    retag(f"{repo}/m", "keep", old.digest)
    moved = _publish(repo, kind, "m", "v2", "1")
    assert moved.digest != old.digest
    return old.digest, other.digest


@pytest.mark.parametrize("kind", ["skill", "rule"])
@pytest.mark.parametrize("command", ["uninstall", "remove"])
def test_undeclaring_a_bundle_member_keeps_its_tagless_pin(
    kind: str, command: str, grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    grim = grim_at(project_dir)
    old, _ = _setup(grim, project_dir, unique_repo, kind, "1")
    before = (project_dir / _OUTPUT[kind].format("m")).read_text(encoding="utf-8")

    grim.run(command, kind, "m")

    assert old in _pin(project_dir, kind, "m")
    assert (project_dir / _OUTPUT[kind].format("m")).read_text(encoding="utf-8") == before


@pytest.mark.parametrize("kind", ["skill", "rule"])
def test_a_remaining_request_at_another_tag_still_resolves(
    kind: str, grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    grim = grim_at(project_dir)
    old, other = _setup(grim, project_dir, unique_repo, kind, "2")

    grim.run("uninstall", kind, "m")

    pinned = _pin(project_dir, kind, "m")
    assert other in pinned and old not in pinned
