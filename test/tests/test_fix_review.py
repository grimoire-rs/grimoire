# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Regressions found by the adversarial review of the state-drift branch."""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact

BAD_SKILL = (
    "---\n"
    "name: code-review\n"
    "description: Test skill.\n"
    "metadata:\n"
    '  claude.effort: "not-a-valid-effort"\n'
    "---\n"
    "# CR\n"
)


def test_fresh_partial_install_does_not_report_itself_installed(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A first install that fails on its last client must stay recoverable.

    OpenCode renders the skill fine (it drops the foreign ``claude.*``
    key); Claude tries to lift it, hits an unconvertible literal, and
    fails — after copy_tree has already written the canonical tree to
    Claude's destination. With no prior record to fall back on, recording
    that half-done destination at the NEW pin makes the retry's integrity
    gate see an intact, fully-covered record at the locked pin and answer
    `unchanged` — leaving the wrong bytes in place forever, at exit 0.
    """
    repo = f"{unique_repo}/code-review"
    make_artifact(repo, "skill", {"code-review/SKILL.md": BAD_SKILL}, tag="1.0.0")
    (project_dir / ".opencode").mkdir(exist_ok=True)
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["opencode", "claude"]\n\n'
        "[skills]\n"
        f'code-review = "{registry}/{repo}:1.0.0"\n'
    )
    runner = grim_at(project_dir)
    runner.run("lock")

    first = runner.run("install", check=False)
    assert first.returncode != 0, "the failing client must fail the command"
    claude_index = project_dir / ".claude/skills/code-review/SKILL.md"
    assert claude_index.exists(), (
        "this test only means something if the failing client left bytes behind"
    )

    retry = runner.run("install", check=False)
    rows = runner.json("status")["items"]
    row = next(r for r in rows if r["name"] == "code-review")
    assert not (retry.returncode == 0 and row["state"] == "installed"), (
        "the retry claimed a clean install over a destination grim never "
        f"finished writing: rc={retry.returncode} state={row['state']}"
    )
