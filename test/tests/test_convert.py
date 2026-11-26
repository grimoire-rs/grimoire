# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim convert` acceptance tests — Claude-authored artifacts to the portable form."""
from __future__ import annotations

import json
import shutil
from pathlib import Path

DATA = Path(__file__).resolve().parent.parent / "data" / "convert"


def _stage(src: str, project_dir: Path) -> Path:
    dst = project_dir / "in" / Path(src).name
    shutil.copytree(DATA / src, dst)
    return dst


def test_convert_skill_then_build_and_install(grim_at, project_dir: Path) -> None:
    src = _stage("claude-skill/pdf", project_dir)
    out = project_dir / "out"
    runner = grim_at(project_dir)
    report = runner.json("convert", str(src), "--from", "claude", "--out", str(out))

    assert report["kind"] == "skill" and report["name"] == "pdf"
    moved = {m["field"]: m["to"] for m in report["moved"]}
    assert moved == {
        "user-invocable": "claude.user-invocable",
        "when_to_use": "claude.when-to-use",
        "effort": "claude.effort",
    }
    dropped = {d["field"]: d["reason"] for d in report["dropped"]}
    assert dropped == {"hooks": "unknown-field", "version": "unknown-field"}
    warned = {(w["file"], w["line"], w["token"]) for w in report["warnings"]}
    assert warned == {
        ("SKILL.md", 13, "${CLAUDE_PLUGIN_ROOT}"),
        ("SKILL.md", 13, "$ARGUMENTS"),
        ("SKILL.md", 14, "$CLAUDE_PROJECT_DIR"),
        ("ref/notes.md", 1, "${CLAUDE_PROJECT_DIR}"),
    }

    converted = out / "pdf"
    assert (converted / "ref" / "blob.bin").read_bytes() == (
        src / "ref" / "blob.bin"
    ).read_bytes()
    text = (converted / "SKILL.md").read_text(encoding="utf-8")
    assert text.endswith((src / "SKILL.md").read_text(encoding="utf-8").split("---\n", 2)[2])

    built = runner.json("build", str(converted))
    assert built["status"] == "built"

    # Installing for Claude lifts the moved keys back to their native names.
    runner.env["GRIM_OFFLINE"] = "1"
    (project_dir / "grimoire.toml").write_text("[skills]\n", encoding="utf-8", newline="\n")
    runner.run("install", str(converted), "--client", "claude")
    rendered = (project_dir / ".claude/skills/pdf/SKILL.md").read_text(encoding="utf-8")
    assert "user-invocable: false" in rendered
    assert "when_to_use: Use when the user mentions a PDF." in rendered
    assert "effort: high" in rendered
    assert "hooks" not in rendered


def test_convert_plain_table(grim_at, project_dir: Path) -> None:
    src = _stage("claude-skill/pdf", project_dir)
    result = grim_at(project_dir).run(
        "convert", str(src), "--from", "claude", "--out", str(project_dir / "o")
    )
    lines = result.stdout.splitlines()
    assert lines[0] == f"converted skill pdf -> {project_dir / 'o' / 'pdf'}"
    assert lines[1].split() == ["Field", "Action", "Detail"]
    assert any("moved" in line and "when_to_use" in line for line in lines)
    assert any("dropped" in line and "hooks" in line for line in lines)
    assert any("warning" in line and "SKILL.md:13" in line for line in lines)


def test_convert_agent(grim_at, project_dir: Path) -> None:
    src = _stage("claude-agent", project_dir) / "reviewer.md"
    out = project_dir / "out"
    runner = grim_at(project_dir)
    report = runner.json(
        "convert", str(src), "--from", "claude", "--out", str(out), "--kind", "agent"
    )
    assert report["kind"] == "agent" and report["name"] == "reviewer"
    assert [m["field"] for m in report["moved"]] == ["maxTurns"]
    assert {d["field"]: d["reason"] for d in report["dropped"]} == {
        "mcpServers": "unknown-field",
        "color": "unrepresentable",
    }
    assert report["warnings"] == [
        {"file": "reviewer.md", "line": 9, "token": "$ARGUMENTS"}
    ]
    assert (out / "reviewer.md").read_text(encoding="utf-8").endswith(
        "---\nReview $ARGUMENTS carefully.\n"
    )
    assert runner.json("build", str(out / "reviewer.md"), "--kind", "agent")["status"] == "built"


def test_convert_rule_with_and_without_frontmatter(grim_at, project_dir: Path) -> None:
    src = _stage("claude-rule", project_dir)
    runner = grim_at(project_dir)
    report = runner.json(
        "convert", str(src / "style.md"), "--from", "claude", "--out", str(project_dir / "o1")
    )
    assert report["kind"] == "rule" and report["name"] == "style"
    assert report["moved"] == []
    assert report["dropped"] == [{"field": "version", "reason": "unknown-field"}]
    assert report["warnings"][0]["token"] == "${CLAUDE_PLUGIN_ROOT}"
    assert "version" not in (project_dir / "o1/style.md").read_text(encoding="utf-8")

    runner.run(
        "convert", str(src / "plain.md"), "--from", "claude", "--out", str(project_dir / "o2")
    )
    assert (project_dir / "o2/plain.md").read_bytes() == (src / "plain.md").read_bytes()


def test_convert_exit_codes(grim_at, project_dir: Path) -> None:
    src = _stage("claude-skill/pdf", project_dir)
    rule = _stage("claude-rule", project_dir) / "plain.md"
    runner = grim_at(project_dir)
    base = ["--from", "claude"]

    # 79: input absent
    r = runner.run("convert", str(project_dir / "nope"), *base, "--out", str(project_dir / "o"), check=False)
    assert r.returncode == 79, r.stderr

    # 64: kind mismatch
    r = runner.run("convert", str(rule), *base, "--out", str(project_dir / "o"), "--kind", "skill", check=False)
    assert r.returncode == 64, r.stderr
    assert "is a .md file; pass --kind rule or --kind agent" in r.stderr
    r = runner.run("convert", str(src), *base, "--out", str(project_dir / "o"), "--kind", "agent", check=False)
    assert r.returncode == 64, r.stderr
    assert "is a skill directory (holds SKILL.md); drop --kind or pass --kind skill" in r.stderr

    # 65: occupied --out
    occupied = project_dir / "occupied"
    occupied.mkdir()
    (occupied / "x").write_text("x", encoding="utf-8", newline="\n")
    r = runner.run("convert", str(src), *base, "--out", str(occupied), check=False)
    assert r.returncode == 65, r.stderr
    assert "not empty" in r.stderr
    # `convert` has no --force, so the error document must not offer one.
    r = runner.run("--format", "json", "convert", str(src), *base, "--out", str(occupied), check=False)
    assert r.returncode == 65, r.stderr
    assert "forceable" not in json.loads(r.stdout)["error"]

    # 65: missing name
    noname = project_dir / "in" / "noname"
    noname.mkdir()
    (noname / "SKILL.md").write_text("---\ndescription: d\n---\nb\n", encoding="utf-8", newline="\n")
    r = runner.run("convert", str(noname), *base, "--out", str(project_dir / "o3"), check=False)
    assert r.returncode == 65, r.stderr

    # 65: unparseable frontmatter
    bad = project_dir / "in" / "bad"
    bad.mkdir()
    (bad / "SKILL.md").write_text("---\nname: [oops\n---\nb\n", encoding="utf-8", newline="\n")
    r = runner.run("convert", str(bad), *base, "--out", str(project_dir / "o4"), check=False)
    assert r.returncode == 65, r.stderr

    # 65: invalid name grammar
    upper = project_dir / "in" / "upper"
    upper.mkdir()
    (upper / "SKILL.md").write_text("---\nname: Bad_Name\ndescription: d\n---\nb\n", encoding="utf-8", newline="\n")
    r = runner.run("convert", str(upper), *base, "--out", str(project_dir / "o5"), check=False)
    assert r.returncode == 65, r.stderr
