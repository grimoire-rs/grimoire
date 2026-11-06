# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
# doc: settings-reference
"""The settings reference page, run from the page itself.

The page binds two fences with ``<!-- doc: settings-reference -->``: the
example document and the ``grim build`` that validates it. This module reads
both out of the markdown at test time, writes the document where the page
says to save it, and runs the command, so a reader copying the page runs what
this test ran. Every other fence on the page sits inside a ``doc-norun`` block
that states why it does not run, and a second test holds that split.
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

from src.helpers import PROJECT_ROOT

pytestmark = pytest.mark.skipif(os.name == "nt", reason="the page's fences are POSIX shell")

PAGE = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "settings.md"
KEY = "settings-reference"
DOCUMENT_PATH = Path("settings") / "team-defaults.toml"

_BOUND_FENCE = re.compile(
    rf"^[ \t]*<!-- doc: {re.escape(KEY)} -->\n[ \t]*```(?P<lang>[\w-]+)\n(?P<body>.*?)\n[ \t]*```",
    re.MULTILINE | re.DOTALL,
)
_FENCE = re.compile(r"^[ \t]*```(?P<info>\S*)")
_NORUN_OPEN = re.compile(r"<!--\s*doc-norun:\s*\S.*-->")
_NORUN_CLOSE = re.compile(r"<!--\s*/doc-norun\s*-->")
_BINDING = re.compile(rf"^[ \t]*<!-- doc: {re.escape(KEY)} -->\s*$")


def _bound_fences(text: str) -> list[tuple[str, str]]:
    """``(lang, body)`` for every fence the page binds, in order."""
    fences = []
    for m in _BOUND_FENCE.finditer(text):
        lines = m.group("body").splitlines()
        indent = min((len(x) - len(x.lstrip()) for x in lines if x.strip()), default=0)
        fences.append((m.group("lang"), "\n".join(x[indent:] for x in lines) + "\n"))
    return fences


def _fence_marks(text: str) -> list[tuple[int, str, str]]:
    """``(line, info, mark)`` for every fence, where mark is bound, norun or none."""
    lines = text.splitlines()
    marks, in_norun, i = [], False, 0
    while i < len(lines):
        line = lines[i]
        if _NORUN_OPEN.search(line):
            in_norun = True
        if _NORUN_CLOSE.search(line):
            in_norun = False
        m = _FENCE.match(line)
        if not m:
            i += 1
            continue
        bound = i > 0 and _BINDING.match(lines[i - 1]) is not None
        marks.append((i + 1, m.group("info"), "bound" if bound else "norun" if in_norun else "none"))
        i += 1
        while i < len(lines) and not _FENCE.match(lines[i]):
            i += 1
        i += 1
    return marks


def test_the_page_example_builds(grim_at, grim_binary: Path, project_dir: Path, tmp_path: Path) -> None:
    fences = _bound_fences(PAGE.read_text(encoding="utf-8"))
    assert [lang for lang, _ in fences] == ["toml", "bash-run"], f"the page's bound fences changed: {fences}"
    (_, document), (_, command) = fences
    assert str(DOCUMENT_PATH) in command, command

    target = project_dir / DOCUMENT_PATH
    target.parent.mkdir(parents=True)
    target.write_text(document, encoding="utf-8")

    runner = grim_at(project_dir)
    # The page types `grim`; give the shell that name, and nothing else of the host's.
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "grim").symlink_to(grim_binary)
    env = {**runner.env, "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}
    result = subprocess.run(
        ["sh", "-ec", command],
        capture_output=True,
        text=True,
        encoding="utf-8",
        env=env,
        cwd=project_dir,
        check=False,
    )
    assert result.returncode == 0, f"{command}\nstderr: {result.stderr}"
    for word in ("settings", "team-defaults", "built"):
        assert word in result.stdout, result.stdout


def test_every_unbound_fence_says_why_it_does_not_run() -> None:
    marks = _fence_marks(PAGE.read_text(encoding="utf-8"))
    assert [info for _, info, mark in marks if mark == "bound"] == ["toml", "bash-run"], marks
    unmarked = [(line, info) for line, info, mark in marks if mark == "none"]
    assert not unmarked, f"fences with neither a binding nor a doc-norun reason: {unmarked}"
    assert sum(1 for *_, mark in marks if mark == "norun") > 0, "the page shows no unrun example"
