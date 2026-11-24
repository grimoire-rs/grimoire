# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""S-014: a Claude-dialect upstream skill mirrors into the agentskills shape and renders back for Claude."""
from __future__ import annotations

import json
import re
from pathlib import Path

from src.git_fixture import BASE_DATE, GitRepo
from src.helpers import make_artifact, write_config

MIT_TEXT = (Path(__file__).resolve().parents[2] / "crates" / "grim-mirror" / "licenses" / "MIT.txt").read_text(encoding="utf-8")

CLAUDE_SKILL = (
    "---\nname: pdf\ndescription: Work with PDFs.\nuser-invocable: false\nwhen_to_use: Use for PDF files.\n---\n\n"
    "# pdf\n\nRun ${CLAUDE_PLUGIN_ROOT}/scripts/x.sh on $ARGUMENTS.\n"
)


def _top_level_keys(skill_md: Path) -> set[str]:
    head = skill_md.read_text(encoding="utf-8").split("---")[1]
    return set(re.findall(r"^([A-Za-z_][\w-]*):", head, flags=re.MULTILINE))


def _mirror(grim_mirror, tmp_path: Path, registry: str, unique_repo: str) -> dict:
    repo = GitRepo.init(tmp_path / "up")
    repo.record("claude skill", {"skills/pdf/SKILL.md": CLAUDE_SKILL, "LICENSE": MIT_TEXT}, date=BASE_DATE)
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo.url}"\ndialect = "claude"\nskills = ["skills/*"]\n',
        encoding="utf-8",
        newline="\n",
    )
    report = json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)
    return next(i for i in report["items"] if i["name"] == "pdf")


def test_claude_dialect_skill_warns_on_body_tokens_and_publishes(grim_mirror, tmp_path, registry, unique_repo) -> None:
    item = _mirror(grim_mirror, tmp_path, registry, unique_repo)
    assert item["status"] == "published", item
    tokens = {w["message"] for w in item["warnings"] if w["code"] == "body-token"}
    assert any("${CLAUDE_PLUGIN_ROOT}" in m for m in tokens), item["warnings"]
    assert any("$ARGUMENTS" in m for m in tokens), item["warnings"]


def test_claude_dialect_skill_moves_native_fields_into_claude_metadata(grim_mirror, grim_at, tmp_path, registry, unique_repo) -> None:
    item = _mirror(grim_mirror, tmp_path, registry, unique_repo)
    text = grim_at(tmp_path).run("fetch", item["ref"]).stdout
    assert "claude.user-invocable" in text and "claude.when-to-use" in text, text
    head = text.split("---")[1]
    assert not re.search(r"^when_to_use:", head, flags=re.MULTILINE), text


def test_mirrored_claude_skill_installs_like_the_natively_published_equivalent(
    grim_mirror, grim_at, tmp_path, registry, unique_repo
) -> None:
    item = _mirror(grim_mirror, tmp_path, registry, unique_repo)
    native_md = (
        "---\nname: pdf\ndescription: Work with PDFs.\nmetadata:\n"
        '  claude.user-invocable: "false"\n  claude.when-to-use: "Use for PDF files."\n---\n\n# pdf\n'
    )
    native = make_artifact(f"{unique_repo}/native/pdf", "skill", {"pdf/SKILL.md": native_md}, tag="1.0.0")

    mirrored_dir = tmp_path / "mirrored-project"
    native_dir = tmp_path / "native-project"
    for project, ref in ((mirrored_dir, item["ref"]), (native_dir, native.fq)):
        (project / ".claude").mkdir(parents=True)
        write_config(project)
        grim_at(project).run("add", ref)

    got = _top_level_keys(mirrored_dir / ".claude" / "skills" / "pdf" / "SKILL.md")
    want = _top_level_keys(native_dir / ".claude" / "skills" / "pdf" / "SKILL.md")
    assert got == want
    assert {"user-invocable", "when_to_use"} <= got


def test_claude_dialect_agent_publishes_as_an_agent_keeping_its_fields(
    grim_mirror, grim_at, tmp_path, registry, unique_repo
) -> None:
    repo = GitRepo.init(tmp_path / "up")
    agent = (
        "---\nname: reviewer\ndescription: Reviews code.\nmodel: sonnet\ntools: Read, Grep\nmaxTurns: 5\n---\n\n"
        "Review the change.\n"
    )
    repo.record("claude agent", {"agents/review.md": agent, "LICENSE": MIT_TEXT}, date=BASE_DATE)
    mf = tmp_path / "mirror.toml"
    mf.write_text(
        f'schema = 1\n[mirror]\nregistry = "{registry}/{unique_repo}"\n'
        f'[[upstream]]\nname = "up"\nrepo = "{repo.url}"\ndialect = "claude"\nagents = ["agents/*.md"]\n',
        encoding="utf-8",
        newline="\n",
    )

    report = json.loads(grim_mirror.run("--file", str(mf), "--format", "json", "sync", check=False).stdout)

    (item,) = report["items"]
    assert (item["kind"], item["name"], item["status"]) == ("agent", "reviewer", "published"), item
    assert item["ref"].startswith(f"{registry}/{unique_repo}/up/agents/reviewer:")
    runner = grim_at(tmp_path)
    assert runner.json("describe", item["ref"])["kind"] == "agent"
    text = runner.run("fetch", item["ref"]).stdout
    head = text.split("---")[1]
    for key in ("name: reviewer", "description: Reviews code.", "model: sonnet", "tools: Read, Grep"):
        assert key in head, text
    assert "claude.max-turns" in head, text
    assert not re.search(r"^maxTurns:", head, flags=re.MULTILINE), text
