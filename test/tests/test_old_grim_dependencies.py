"""How grim 0.15.1 meets artifacts and locks that carry dependencies.

Dependents are published by newer grims; an older grim on the same machine must
refuse cleanly or ignore what it cannot see, never corrupt.  The binary under
test is the *old* one, given by ``GRIM_OLD_BIN`` (a grim 0.15.1 build); the
module skips when it is unset.  The fixtures are pushed by the test harness, so
no new grim is involved.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, make_dependent, write_config
from src.registry import LAYER_MEDIA_TYPE, REGISTRY_HOST, push_manifest
from src.runner import GrimRunner

OLD_BIN = os.environ.get("GRIM_OLD_BIN")

pytestmark = pytest.mark.skipif(
    not OLD_BIN, reason="GRIM_OLD_BIN names no grim 0.15.1 binary"
)

REQUIRES_LAYER = "application/vnd.grimoire.bundle.v1+json"
SKILL = {"s/SKILL.md": "---\nname: s\ndescription: a skill\n---\n# s\n"}


@pytest.fixture()
def old(tmp_path: Path) -> GrimRunner:
    """The old grim, in a Claude-detected project with its own GRIM_HOME."""
    assert OLD_BIN is not None
    grim_home = tmp_path / "grim-home"
    grim_home.mkdir()
    project = tmp_path / "project"
    (project / ".claude").mkdir(parents=True)
    runner = GrimRunner(Path(OLD_BIN), grim_home, cwd=project)
    version = runner.run("--version").stdout
    assert "0.15." in version, f"GRIM_OLD_BIN is not a grim 0.15: {version!r}"
    runner.run("init")
    return runner


def _lock_text(runner: GrimRunner) -> str:
    assert runner.cwd is not None
    return (runner.cwd / "grimoire.lock").read_text()


def test_old_grim_adds_a_plain_artifact(old, registry, unique_repo):
    """Positive control: the old binary installs an ordinary skill here."""
    skill = make_artifact(f"{unique_repo}/plain", "skill", SKILL, tag="1")

    old.run("add", skill.fq, "--name", "plain")

    assert (old.cwd / ".claude" / "skills" / "plain" / "SKILL.md").is_file()


def test_old_grim_add_of_a_dependent_exits_65(old, registry, unique_repo):
    parent = make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        SKILL,
        [("skill", "dep", f"{REGISTRY_HOST}/{unique_repo}/dep:1")],
    )

    result = old.run("add", parent.fq, "--name", "parent", check=False)

    assert result.returncode == 65, result.stderr
    assert "expected a single-layer artifact" in result.stderr
    assert not (old.cwd / ".claude" / "skills" / "parent").exists()


def test_old_grim_locks_a_dependent_without_its_edges(old, registry, unique_repo):
    dep = make_artifact(f"{unique_repo}/dep", "skill", SKILL, tag="1")
    parent = make_dependent(
        f"{unique_repo}/parent",
        "1",
        "skill",
        SKILL,
        [("skill", "dep", dep.fq)],
    )

    old.run("add", "--no-install", parent.fq, "--name", "parent")
    added = _lock_text(old)
    old.run("lock")
    locked = _lock_text(old)

    for text in (added, locked):
        assert 'name = "parent"' in text  # the dependent itself is locked
        assert "requires" not in text  # but its edges are not
        assert 'name = "dep"' not in text  # and neither is the dependency
    assert "[[skill.requires]]" not in locked


_LOCK_HEAD = (
    "[metadata]\n"
    "lock_version = 1\n"
    "declaration_hash_version = 1\n"
    f'declaration_hash = "sha256:{"0" * 64}"\n'
    'generated_by = "grim 0.16.0"\n'
    'generated_at = "2026-01-01T00:00:00Z"\n\n'
)
_SKILL_ENTRY = f'[[skill]]\nname = "a"\npinned = "localhost:5000/x/a@sha256:{"1" * 64}"\n'
_BUNDLE_ENTRY = (
    '[[bundle]]\nname = "b"\nrepo = "localhost:5000/x/b"\ntag = "1"\n'
    f'pinned = "localhost:5000/x/b@sha256:{"2" * 64}"\n'
)


@pytest.mark.parametrize(
    "body",
    [
        pytest.param(
            _SKILL_ENTRY
            + '\n[[skill.requires]]\nkind = "skill"\nname = "dep"\nid = "x/dep:1"\n',
            id="skill-requires",
        ),
        pytest.param(_BUNDLE_ENTRY + "transitive = true\n", id="transitive"),
    ],
)
def test_old_grim_rejects_a_lock_with_dependency_fields(old, body):
    field = "requires" if "requires" in body else "transitive"
    write_config(old.cwd, skills={"a": "localhost:5000/x/a:1"})
    (old.cwd / "grimoire.lock").write_text(_LOCK_HEAD + body)

    result = old.run("status", check=False)

    assert result.returncode == 78, result.stderr
    assert field in result.stderr and "invalid TOML" in result.stderr


def test_old_grim_reads_a_lock_whose_bundle_member_is_a_bundle(
    old, registry, unique_repo
):
    """The lock reader does not police member kinds; only manifest expansion does.

    Pinned as observed on 0.15.1: a bundle member of kind `bundle` in the lock is
    parsed and ignored, and `install` materializes the lock's own entries.
    """
    member = make_artifact(f"{unique_repo}/member", "skill", SKILL, tag="1")
    bundle = make_bundle(f"{unique_repo}/b", [("skill", "member", member.fq)], tag="1")
    old.run("add", "--no-install", bundle.fq, "--name", "b")
    text = _lock_text(old)
    member_key = 'kind = "skill"\nname = "member"'
    assert member_key in text, text
    (old.cwd / "grimoire.lock").write_text(
        text.replace(member_key, 'kind = "bundle"\nname = "member"')
    )

    for command in ("status", "install"):
        result = old.run(command, check=False)
        assert result.returncode == 0, (command, result.stderr)
    assert (old.cwd / ".claude" / "skills" / "member" / "SKILL.md").is_file()


def test_old_grim_refuses_a_three_layer_manifest(old, registry, unique_repo):
    repo = f"{unique_repo}/three"
    push_manifest(
        repo,
        "1",
        [
            (LAYER_MEDIA_TYPE, b"x"),
            (REQUIRES_LAYER, b"{}"),
            (REQUIRES_LAYER, b"{}"),
        ],
        {"com.grimoire.kind": "skill"},
    )

    result = old.run("add", f"{REGISTRY_HOST}/{repo}:1", "--name", "three", check=False)

    assert result.returncode == 65, result.stderr
    assert "expected a single-layer artifact, manifest has 3 layers" in result.stderr


def test_old_grim_ignores_an_unknown_requires_feature(old, registry, unique_repo):
    skill = make_artifact(
        f"{unique_repo}/featured",
        "skill",
        SKILL,
        tag="1",
        annotations={"com.grimoire.requires-features": "future-thing"},
    )

    old.run("add", skill.fq, "--name", "featured")

    assert (old.cwd / ".claude" / "skills" / "featured" / "SKILL.md").is_file()


def test_old_grim_rejects_a_nested_bundle_manifest(old, registry, unique_repo):
    inner = make_bundle(f"{unique_repo}/inner", [], tag="1")
    outer = make_bundle(
        f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1"
    )

    result = old.run("add", outer.fq, "--name", "outer", check=False)

    assert result.returncode == 65, result.stderr
    assert "nested bundle member" in result.stderr
