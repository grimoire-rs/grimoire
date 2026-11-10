# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
# doc: guide-dependencies
"""The "Bundle or require" guide, run from the page itself.

Every fence the page binds with ``<!-- doc: guide-dependencies -->`` is read
out of the markdown at test time and each ``bash-run`` fence is executed in
order, so a reader copying the page runs what this test ran. The page's
registry, ``localhost:5000/``, is swapped for a per-test repository on the
suite's own registry, and a ``text`` fence that directly follows a bound
``grim why`` is compared with what the command printed.
"""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

from src.helpers import PROJECT_ROOT

pytestmark = pytest.mark.skipif(os.name == "nt", reason="the page's fences are POSIX shell")

PAGE = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "guides" / "dependencies.md"
KEY = "guide-dependencies"
PAGE_REGISTRY = "localhost:5000/"

_BOUND_FENCE = re.compile(
    rf"^[ \t]*<!-- doc: {re.escape(KEY)} -->\n[ \t]*```(?P<lang>[\w-]+)\n(?P<body>.*?)\n[ \t]*```"
    r"(?:\n+[ \t]*```text\n(?P<out>.*?)\n[ \t]*```)?",
    re.MULTILINE | re.DOTALL,
)


def _bound_fences(text: str) -> list[tuple[str, str, str | None]]:
    """``(lang, body, shown output)`` for every fence the page binds, in order."""
    fences = []
    for m in _BOUND_FENCE.finditer(text):
        lines = m.group("body").splitlines()
        indent = min((len(x) - len(x.lstrip()) for x in lines if x.strip()), default=0)
        fences.append((m.group("lang"), "\n".join(x[indent:] for x in lines) + "\n", m.group("out")))
    return fences


def test_the_guide_runs_end_to_end(
    grim_at, grim_binary: Path, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    fences = _bound_fences(PAGE.read_text(encoding="utf-8"))
    commands = [(body, out) for lang, body, out in fences if lang == "bash-run"]
    assert len(commands) == 12, f"the page gained or lost a bound command: {[c for c, _ in commands]}"
    assert len(commands) == len(fences), "a bound fence is not a bash-run fence"
    assert sum(1 for c, _ in commands if "grim why" in c) == 3

    runner = grim_at(project_dir)
    runner.run("init")
    # The page types `grim`; give the shell that name, and nothing else of the host's.
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "grim").symlink_to(grim_binary)
    env = {**runner.env, "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}
    target = f"{registry}/{unique_repo}/"

    compared = 0
    for body, shown in commands:
        command = body.replace(PAGE_REGISTRY, target)
        result = subprocess.run(
            ["sh", "-ec", command], capture_output=True, text=True, env=env, cwd=project_dir, check=False
        )
        assert result.returncode == 0, f"{body}\nstderr: {result.stderr}"
        if shown is not None:
            printed = [line.strip() for line in result.stdout.strip().splitlines()]
            assert printed == [line.strip() for line in shown.splitlines()], result.stdout
            compared += 1
    # A shown output that drifts away from its command is never compared.
    assert compared == 3, f"compared {compared} shown outputs, the page shows 3"

    # What the bundle and the requirement installed, read back from the lock.
    lock = (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    for name in ("plan", "design", "build", "test", "deploy", "maintain", "templates", "gdrive-sync"):
        assert f'name = "{name}"' in lock, name
    assert 'name = "security-reviewer"' in lock and 'name = "gdrive"' in lock
