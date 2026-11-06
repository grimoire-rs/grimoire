# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
# doc: guide-share-settings
"""The "Share harness settings" guide, run from the page itself.

Every fence on the page is bound and read out of the markdown at test time.
Each ``bash-run`` fence runs in page order; a ``text`` fence after one is its
stderr change list, a ``json`` fence its stdout. Commands under the teammate
heading run in a fresh clone with its own ``GRIM_HOME`` and ``HOME``. The
page's ``localhost:5000/`` becomes a per-test repository on the suite registry.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import tomllib
from dataclasses import dataclass
from pathlib import Path

import pytest

from src import runner as grim_runner
from src.helpers import PROJECT_ROOT
from src.runner import GrimRunner

pytestmark = pytest.mark.skipif(os.name == "nt", reason="the page's fences are POSIX shell")

PAGE = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "guides" / "share-settings.md"
KEY = "guide-share-settings"
PAGE_REGISTRY = "localhost:5000/"
TEAMMATE_SECTION = "teammates"
LOCAL = Path(".claude") / "settings.local.json"
# One exit code per bound command, in page order. Only the run before `--force` refuses.
EXITS = [0] * 12 + [65] + [0] * 4
SHOWN = 8
MARKETPLACE = {"source": {"source": "github", "repo": "acme/plugins"}}
CODEX_MARKETPLACE = {"source": "https://github.com/acme/plugins.git", "source_type": "git"}

_SECTION = re.compile(r"^##\s.*\{#(?P<anchor>[a-z0-9-]+)\}\s*$")
_FENCE_OPEN = re.compile(r"^[ \t]*```(?P<info>\S*)\s*$")
_BINDING = re.compile(rf"^[ \t]*<!-- doc: {re.escape(KEY)} -->\s*$")
_ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
_WARNING = re.compile(r"\bWARN\s+\S+:\s(?P<message>.*)$")


@dataclass(slots=True)
class Fence:
    line: int
    lang: str
    body: str
    section: str
    bound: bool


@dataclass(slots=True)
class Step:
    command: str
    section: str
    shown: Fence | None = None


def _fences(text: str) -> list[Fence]:
    """Every fence on the page, with the `##` section it sits in and its binding."""
    lines = text.splitlines()
    fences: list[Fence] = []
    section, i = "", 0
    while i < len(lines):
        if heading := _SECTION.match(lines[i]):
            section = heading.group("anchor")
        opened = _FENCE_OPEN.match(lines[i])
        if not opened:
            i += 1
            continue
        start, body = i, []
        i += 1
        while i < len(lines) and lines[i].strip() != "```":
            body.append(lines[i])
            i += 1
        i += 1
        # List-item fences are indented; the reader copies them without it.
        indent = min((len(x) - len(x.lstrip()) for x in body if x.strip()), default=0)
        fences.append(
            Fence(
                line=start + 1,
                lang=opened.group("info"),
                body="\n".join(x[indent:] for x in body) + "\n",
                section=section,
                bound=start > 0 and _BINDING.match(lines[start - 1]) is not None,
            )
        )
    return fences


def _steps(fences: list[Fence]) -> list[Step]:
    """Commands in page order, each with the output fence that follows it, if any."""
    steps: list[Step] = []
    for fence in fences:
        if fence.lang == "bash-run":
            steps.append(Step(fence.body, fence.section))
            continue
        assert fence.lang in ("text", "json"), f"line {fence.line}: a {fence.lang!r} fence is neither run nor shown"
        assert steps and steps[-1].shown is None, f"line {fence.line}: shown output with no command of its own"
        steps[-1].shown = fence
    return steps


def _warnings(stderr: str) -> list[str]:
    """The messages of grim's warning lines, without the timestamp and target."""
    return [
        found.group("message").rstrip()
        for line in _ANSI.sub("", stderr).splitlines()
        if (found := _WARNING.search(line))
    ]


def _assert_shown(shown: Fence, result: subprocess.CompletedProcess[str]) -> None:
    if shown.lang == "json":
        assert json.loads(result.stdout) == json.loads(shown.body), result.stdout
        return
    expected = [line.strip() for line in shown.body.splitlines() if line.strip()]
    assert _warnings(result.stderr) == expected, result.stderr


def _teammate(binary: Path, project_dir: Path, tmp_path: Path) -> GrimRunner:
    """A clone on another machine: the committed files, and its own `GRIM_HOME` and `HOME`."""
    clone = tmp_path / "teammate" / "app"
    clone.mkdir(parents=True)
    for name in ("grimoire.toml", "grimoire.lock"):
        shutil.copy(project_dir / name, clone / name)
    grim_home = tmp_path / "teammate" / "grim-home"
    grim_home.mkdir()
    mate = GrimRunner(binary, grim_home, cwd=clone)

    # No terminal and no flag: the clone applies nothing. The page's own command is the positive control.
    quiet = mate.run("install")
    assert not (clone / LOCAL).exists(), quiet.stderr
    status = mate.run("status")
    assert "gated" in status.stdout and "workspace-not-consented" in status.stdout, status.stdout
    return mate


def test_the_guide_runs_end_to_end(
    grim_at, grim_binary: Path, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    if grim_runner.HOST_MANAGED_CLAUDE_CONFIG_DIR:
        pytest.skip(
            "HOST ISOLATION: this machine's managed Claude settings set CLAUDE_CONFIG_DIR, "
            "and the page's --global runs would write into that real directory"
        )
    fences = _fences(PAGE.read_text(encoding="utf-8"))
    unbound = [(fence.line, fence.lang) for fence in fences if not fence.bound]
    assert not unbound, f"fences without the {KEY} binding: {unbound}"
    steps = _steps(fences)
    assert len(fences) == len(EXITS) + SHOWN, f"the page gained or lost a fence: {[(f.line, f.lang) for f in fences]}"
    assert len(steps) == len(EXITS), [step.command for step in steps]
    assert sum(1 for step in steps if step.shown is not None) == SHOWN
    assert [step.section for step in steps].count(TEAMMATE_SECTION) == 1, "the teammate section runs one command"

    lead = grim_at(project_dir)
    lead.run("init")
    # The reader's three harnesses, each with its own root in the isolated home.
    for root in (".claude", ".codex", ".copilot"):
        (lead.home / root).mkdir()
    # The page types `grim`; give the shell that name, and nothing else of the host's.
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "grim").symlink_to(grim_binary)
    path = f"{bin_dir}{os.pathsep}{os.environ['PATH']}"
    target = f"{registry}/{unique_repo}/"
    mate: GrimRunner | None = None

    for step, exit_code in zip(steps, EXITS, strict=True):
        runner = lead
        if step.section == TEAMMATE_SECTION:
            mate = mate or _teammate(grim_binary, project_dir, tmp_path)
            runner = mate
        command = step.command.replace(PAGE_REGISTRY, target)
        result = subprocess.run(
            ["sh", "-ec", command],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env={**runner.env, "PATH": path},
            cwd=runner.cwd,
            check=False,
        )
        assert result.returncode == exit_code, f"{step.command}\nstdout: {result.stdout}\nstderr: {result.stderr}"
        if exit_code == 65:
            assert "rerun with --force to change them" in result.stderr, result.stderr
        if step.shown is not None:
            _assert_shown(step.shown, result)

    # The marketplace reaches each harness's own file, under that harness's key.
    home = lead.home
    claude = json.loads((home / ".claude" / "settings.json").read_text(encoding="utf-8"))
    copilot = json.loads((home / ".copilot" / "settings.json").read_text(encoding="utf-8"))
    codex = tomllib.loads((home / ".codex" / "config.toml").read_text(encoding="utf-8"))
    assert claude["extraKnownMarketplaces"] == {"acme": MARKETPLACE}
    assert copilot["extraKnownMarketplaces"] == {"acme": MARKETPLACE}
    assert codex["marketplaces"] == {"acme": CODEX_MARKETPLACE}

    # The teammate's clone holds the team's values once its own run consented.
    assert mate is not None
    cloned = json.loads((Path(mate.cwd) / LOCAL).read_text(encoding="utf-8"))
    assert cloned["model"] == "sonnet" and "Bash(rm -rf:*)" in cloned["permissions"]["deny"], cloned

    # Uninstall took the team's values and left the rule the user added.
    assert json.loads((project_dir / LOCAL).read_text(encoding="utf-8")) == {"permissions": {"allow": ["Bash(make:*)"]}}
