# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim build` acceptance tests — validate + pack a local skill/rule."""
from __future__ import annotations

from pathlib import Path


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def test_build_skill_dir(grim_at, project_dir: Path) -> None:
    skill = project_dir / "code-review"
    _write(
        skill / "SKILL.md",
        "---\nname: code-review\ndescription: Review code.\n---\n# Body\n",
    )
    _write(skill / "scripts/run.sh", "echo hi\n")

    runner = grim_at(project_dir)
    out = runner.json("build", str(skill))
    assert out["kind"] == "skill"
    assert out["name"] == "code-review"
    assert out["status"] == "built"
    assert out["layer_digest"].startswith("sha256:")
    assert out["annotation_count"] >= 1


def test_build_rule_file(grim_at, project_dir: Path) -> None:
    rule = project_dir / "rust-style.md"
    _write(rule, "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n")

    runner = grim_at(project_dir)
    out = runner.json("build", str(rule))
    assert out["kind"] == "rule"
    assert out["name"] == "rust-style"
    assert out["status"] == "built"


def test_build_rejects_name_mismatch(grim_at, project_dir: Path) -> None:
    skill = project_dir / "code-review"
    _write(
        skill / "SKILL.md",
        "---\nname: wrong-name\ndescription: d\n---\n",
    )
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill), check=False)
    assert result.returncode == 65, (
        f"name mismatch must exit 65, got {result.returncode}; {result.stderr}"
    )


def test_build_rejects_missing_skill_md(grim_at, project_dir: Path) -> None:
    skill = project_dir / "empty-skill"
    skill.mkdir(parents=True)
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill), check=False)
    assert result.returncode in (65, 74), (
        f"missing SKILL.md must fail, got {result.returncode}; {result.stderr}"
    )


def test_build_rejects_non_https_repository(grim_at, project_dir: Path) -> None:
    """The repository publish gate fires at build time too (local
    pre-flight), for the rule's top-level authoring surface."""
    rule = project_dir / "bad-repo.md"
    _write(
        rule,
        "---\npaths: ['**/*.rs']\nrepository: http://github.com/acme/x\n---\n# R\nbody\n",
    )
    runner = grim_at(project_dir)
    result = runner.run("build", str(rule), check=False)
    assert result.returncode == 65, (
        f"non-HTTPS repository must exit 65, got {result.returncode}; {result.stderr}"
    )
    assert "repository" in result.stderr, result.stderr


def test_build_rejects_missing_description(grim_at, project_dir: Path) -> None:
    skill = project_dir / "code-review"
    _write(skill / "SKILL.md", "---\nname: code-review\n---\n# Body\n")
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill), check=False)
    assert result.returncode == 65, (
        f"missing description must exit 65, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_build_dotted_skill_dir(grim_at, project_dir: Path) -> None:
    """Issue #40: a dotted skill name is valid and must build."""
    skill = project_dir / "socket.io"
    _write(
        skill / "SKILL.md",
        "---\nname: socket.io\ndescription: Socket helpers.\n---\n# Body\n",
    )
    runner = grim_at(project_dir)
    out = runner.json("build", str(skill))
    assert out["kind"] == "skill"
    assert out["name"] == "socket.io"
    assert out["status"] == "built"


def test_build_dotted_rule_stem(grim_at, project_dir: Path) -> None:
    """Issue #40: a dotted rule file stem ('vue.js.md' -> 'vue.js') builds."""
    rule = project_dir / "vue.js.md"
    _write(rule, "---\npaths: ['**/*.vue']\n---\n# Vue Style\nBody.\n")
    runner = grim_at(project_dir)
    out = runner.json("build", str(rule))
    assert out["kind"] == "rule"
    assert out["name"] == "vue.js"
    assert out["status"] == "built"


def test_build_warns_on_oversized_compatibility(grim_at, project_dir: Path) -> None:
    """Issue #154: `compatibility` over the agentskills 500-char cap warns
    on stderr but still builds cleanly (exit 0) — additive, never a hard
    failure."""
    skill = project_dir / "code-review"
    _write(
        skill / "SKILL.md",
        "---\nname: code-review\ndescription: Review code.\n"
        f"compatibility: {'x' * 501}\n---\n# Body\n",
    )
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill))
    assert result.returncode == 0, result.stderr
    assert "compatibility" in result.stderr, result.stderr
    assert "500" in result.stderr, result.stderr


def test_build_warns_on_empty_compatibility(grim_at, project_dir: Path) -> None:
    """Issue #154: a blank `compatibility` warns but still builds (exit 0)."""
    skill = project_dir / "code-review"
    _write(
        skill / "SKILL.md",
        '---\nname: code-review\ndescription: Review code.\ncompatibility: "   "\n---\n# Body\n',
    )
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill))
    assert result.returncode == 0, result.stderr
    assert "compatibility" in result.stderr, result.stderr
    assert "empty" in result.stderr, result.stderr


def test_build_rejects_leading_dot_skill_dir(grim_at, project_dir: Path) -> None:
    """Issue #40 guard rail: a leading-dot name (hidden dir) stays a data
    error (65) after the dotted-name relaxation."""
    skill = project_dir / ".hidden"
    _write(skill / "SKILL.md", "---\nname: .hidden\ndescription: d\n---\n# Body\n")
    runner = grim_at(project_dir)
    result = runner.run("build", str(skill), check=False)
    assert result.returncode == 65, (
        f"leading-dot name must exit 65, got {result.returncode}; {result.stderr}"
    )


# ── .grimignore ────────────────────────────────────────────────────────
# `grim build` emits no layer, but its `layer_digest` covers exactly the
# packed entries, so equal digests prove a file was left out.


def _layer_digest(runner, skill: Path) -> str:
    return runner.json("build", str(skill))["layer_digest"]


def _grimignore_skill(project_dir: Path) -> Path:
    skill = project_dir / "runner"
    _write(skill / "SKILL.md", "---\nname: runner\ndescription: d\n---\n")
    _write(skill / "scripts/foo.py", "print('hi')\n")
    return skill


def test_build_omits_default_ignored_junk(grim_at, project_dir: Path) -> None:
    skill = _grimignore_skill(project_dir)
    runner = grim_at(project_dir)
    clean = _layer_digest(runner, skill)

    _write(skill / "scripts/__pycache__/foo.cpython-313.pyc", "bytecode")
    _write(skill / ".DS_Store", "junk")
    assert _layer_digest(runner, skill) == clean


def test_build_grimignore_negation_ships_a_default(
    grim_at, project_dir: Path
) -> None:
    skill = _grimignore_skill(project_dir)
    _write(skill / ".grimignore", "!.DS_Store\n")
    runner = grim_at(project_dir)
    without = _layer_digest(runner, skill)

    _write(skill / ".DS_Store", "kept")
    assert _layer_digest(runner, skill) != without, "`!.DS_Store` must ship it"


def test_build_grimignore_excludes_listed_file(
    grim_at, project_dir: Path
) -> None:
    skill = _grimignore_skill(project_dir)
    _write(skill / ".grimignore", "secret.txt\n")
    runner = grim_at(project_dir)
    without = _layer_digest(runner, skill)

    _write(skill / "secret.txt", "do not ship")
    assert _layer_digest(runner, skill) == without


def test_build_rejects_invalid_grimignore(grim_at, project_dir: Path) -> None:
    skill = _grimignore_skill(project_dir)
    _write(skill / ".grimignore", "ok.txt\n{unclosed\n")
    result = grim_at(project_dir).run("build", str(skill), check=False)
    assert result.returncode == 65, result.stderr
    assert ".grimignore" in result.stderr and "line 2" in result.stderr, (
        result.stderr
    )


def test_build_relative_path_honours_multi_segment_pattern(
    grim_at, project_dir: Path
) -> None:
    """Regression: a relative root `s` must not byte-strip `scripts/…`."""
    skill = project_dir / "s"
    _write(skill / "SKILL.md", "---\nname: s\ndescription: d\n---\n")
    _write(skill / ".grimignore", "scripts/secret.txt\n")
    runner = grim_at(project_dir)
    without = _layer_digest(runner, Path("s"))

    _write(skill / "scripts/secret.txt", "do not ship")
    assert _layer_digest(runner, Path("s")) == without


def test_build_rejects_oversized_grimignore(grim_at, project_dir: Path) -> None:
    skill = _grimignore_skill(project_dir)
    _write(skill / ".grimignore", "#" * (64 * 1024 + 1))
    result = grim_at(project_dir).run("build", str(skill), check=False)
    assert result.returncode == 65, result.stderr
    assert ".grimignore" in result.stderr, result.stderr
