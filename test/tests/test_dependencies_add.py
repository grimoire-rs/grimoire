# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim add` of a dependent artifact: the closure installs, is reported, and
`--force` stays scoped to the artifact that was typed."""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact, make_bundle, make_dependent, write_config


def _skill_files(name: str, body: str = "") -> dict[str, str]:
    return {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name} {body}\n"}


def _rule_files(name: str) -> dict[str, str]:
    return {f"{name}.md": f"# {name}\n"}


def _leaf(unique_repo: str, name: str, kind: str = "skill"):
    files = _skill_files(name) if kind == "skill" else _rule_files(name)
    return make_artifact(f"{unique_repo}/{name}", kind, files, tag="1")


def _dependent(unique_repo: str, name: str, requires: list[tuple[str, str, str]]):
    return make_dependent(
        f"{unique_repo}/{name}", "1", "skill", _skill_files(name), requires
    )


def _skill_dir(project_dir: Path, name: str) -> Path:
    return project_dir / ".claude" / "skills" / name


def test_pulled_in(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    b = _leaf(unique_repo, "b")
    c = _leaf(unique_repo, "c", kind="rule")
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1"), ("rule", "c", "./c:1")])
    plain = _leaf(unique_repo, "plain")
    write_config(project_dir)
    runner = grim_at(project_dir)

    # Positive control: an artifact with no dependencies reports none and
    # prints no "pulled in" line.
    control = runner.run("--format", "json", "add", plain.fq)
    assert json.loads(control.stdout)["pulled_in"] == []
    assert "pulled in" not in control.stderr, control.stderr

    result = runner.run("--format", "json", "add", a.fq)

    report = json.loads(result.stdout)
    assert report["name"] == "a"
    # Canonical order: kind enum order, then name.
    assert [(p["kind"], p["name"]) for p in report["pulled_in"]] == [
        ("skill", "b"),
        ("rule", "c"),
    ]
    pinned = {p["name"]: p["pinned"] for p in report["pulled_in"]}
    assert pinned["b"].endswith(b.digest) and pinned["c"].endswith(c.digest)
    # The note is a plain-output line; JSON output stays machine-only.
    assert "pulled in" not in result.stderr, result.stderr
    assert (_skill_dir(project_dir, "b") / "SKILL.md").is_file()
    assert (project_dir / ".claude/rules/c.md").is_file()
    # Only the typed artifact is declared.
    cfg = (project_dir / "grimoire.toml").read_text()
    assert "b =" not in cfg and "c =" not in cfg

    _leaf(unique_repo, "d")
    _leaf(unique_repo, "e", kind="rule")
    f = _dependent(unique_repo, "f", [("skill", "d", "./d:1"), ("rule", "e", "./e:1")])
    plain_out = runner.run("add", f.fq)
    assert "pulled in 2 dependencies: skill/d, rule/e" in plain_out.stderr, plain_out.stderr


def test_add_installs_missing_locked_dependency(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    b = _leaf(unique_repo, "b")
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir)
    runner = grim_at(project_dir)
    # The lock already pins b; nothing is on disk.
    runner.run("add", "--no-install", a.fq)
    assert 'name = "b"' in (project_dir / "grimoire.lock").read_text()
    assert not _skill_dir(project_dir, "b").exists()

    runner.run("add", a.fq)

    assert (_skill_dir(project_dir, "a") / "SKILL.md").is_file()
    assert (_skill_dir(project_dir, "b") / "SKILL.md").is_file(), (
        "an already-locked dependency with no files must be installed"
    )
    assert b.digest in (project_dir / "grimoire.lock").read_text()


def test_transitive_chain_installs(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    _leaf(unique_repo, "c")
    _dependent(unique_repo, "b", [("skill", "c", "./c:1")])
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir)
    runner = grim_at(project_dir)

    result = runner.run("--format", "json", "add", a.fq)

    for name in ("a", "b", "c"):
        assert (_skill_dir(project_dir, name) / "SKILL.md").is_file(), name
    pulled = json.loads(result.stdout)["pulled_in"]
    assert [p["name"] for p in pulled] == ["b", "c"]


def test_add_force_transitive_clobber_refused(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    c = _leaf(unique_repo, "c")
    b = _dependent(unique_repo, "b", [("skill", "c", "./c:1")])
    a = _dependent(unique_repo, "a", [("skill", "b", "./b:1")])
    write_config(project_dir)
    runner = grim_at(project_dir)
    # Untracked files the user wrote at the target's and at a transitive
    # dependency's destination.
    for name in ("a", "c"):
        mine = _skill_dir(project_dir, name)
        mine.mkdir(parents=True)
        (mine / "SKILL.md").write_text("mine\n")

    result = runner.run("--format", "json", "add", "--force", a.fq, check=False)

    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert (error["exit"], error["reason"]) == (65, "untracked-destination"), error
    assert "skill 'c'" in error["message"], error
    assert "skill 'a'" not in error["message"], error
    # The typed artifact was forced; the dependency was left alone.
    assert "mine" not in (_skill_dir(project_dir, "a") / "SKILL.md").read_text()
    assert (_skill_dir(project_dir, "c") / "SKILL.md").read_text() == "mine\n"

    # Positive control: install --force is the way to overwrite it.
    runner.run("install", "--force")
    assert "mine" not in (_skill_dir(project_dir, "c") / "SKILL.md").read_text()
    assert c.digest in (project_dir / "grimoire.lock").read_text()
    assert b.digest in (project_dir / "grimoire.lock").read_text()


def test_add_force_bundle_spares_directly_declared_nested_member(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    s = _leaf(unique_repo, "s")
    t = _leaf(unique_repo, "t")
    inner = make_bundle(
        f"{unique_repo}/inner", [("skill", "s", s.fq), ("skill", "t", t.fq)], tag="1"
    )
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    # `s` is declared directly, so the lock carries it without bundle provenance.
    write_config(project_dir, skills={"s": s.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    # `t` is only a nested member, so its untracked file is the bundle's to force.
    for name in ("s", "t"):
        _skill_dir(project_dir, name).mkdir(parents=True)
        (_skill_dir(project_dir, name) / "SKILL.md").write_text("mine\n")
    mine = _skill_dir(project_dir, "s")

    result = runner.run("--format", "json", "add", "--force", outer.fq, check=False)

    assert result.returncode == 65, result.stderr
    error = json.loads(result.stdout)["error"]
    assert error["reason"] == "untracked-destination", error
    assert (mine / "SKILL.md").read_text() == "mine\n"
    assert "mine" not in (_skill_dir(project_dir, "t") / "SKILL.md").read_text()


def test_add_names_a_pin_it_dropped(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A new version that no longer requires `x` takes it out of the lock while
    its files stay on disk and `status` stops showing it. Found by probe: exit 0
    and no output named the orphan. The report is the one place that can."""
    x = _leaf(unique_repo, "x")
    first = make_dependent(f"{unique_repo}/par", "1", "skill", _skill_files("par"), [("skill", "x", "./x:1")])
    second = make_artifact(f"{unique_repo}/par", "skill", _skill_files("par", "two"), tag="2")
    write_config(project_dir)
    runner = grim_at(project_dir)
    runner.run("add", first.fq)

    result = runner.run("--format", "json", "add", second.fq)

    dropped = json.loads(result.stdout)["dropped"]
    assert [(p["kind"], p["name"]) for p in dropped] == [("skill", "x")]
    assert dropped[0]["pinned"].endswith(x.digest)
    assert _skill_dir(project_dir, "x").is_dir()

    runner.run("add", first.fq)
    plain = runner.run("add", second.fq)
    assert "dropped 1 pin: skill/x" in plain.stderr, plain.stderr

    runner.run("update")
    assert not _skill_dir(project_dir, "x").exists(), "the note names this route"
