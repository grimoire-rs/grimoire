# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Fixtures local to the tests/ suite.

Fixtures shared across the whole session (``grim_binary``, ``grim_home``,
``grim``, ``registry``) live in the top-level ``conftest.py``.
"""
from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest

from src.runner import GrimRunner


@pytest.fixture()
def project_dir(tmp_path: Path) -> Path:
    """A project workspace `grim` runs inside of, marked as a Claude project.

    The `.claude/` marker makes the workspace a **detected** Claude
    workspace, so the default client set is a stable `["claude"]`. Without
    any marker, detection finds nothing and grim targets the generic
    `agents` client — one skills-only copy into `.agents/skills` — which is
    correct behaviour but not what a test about install, update, uninstall,
    or state mechanics wants to assert paths against.

    A test that exercises client *selection* needs a genuinely undetected
    workspace: use `bare_project_dir`.
    """
    d = tmp_path / "project"
    (d / ".claude").mkdir(parents=True)
    return d


@pytest.fixture()
def bare_project_dir(tmp_path: Path) -> Path:
    """A project workspace carrying no vendor marker at all.

    Nothing is detected here, so `grim install` resolves to the generic
    `agents` client. This is the fixture for client-selection tests; every
    other suite wants `project_dir`.
    """
    d = tmp_path / "bare-project"
    d.mkdir()
    return d


@pytest.fixture()
def grim_at(
    grim_binary: Path, grim_home: Path
) -> Callable[[Path], GrimRunner]:
    """Factory: a ``GrimRunner`` whose CWD is the given project dir.

    Project-scope commands (`init`, `lock`, `install`, ...) discover the
    config by walking up from the process CWD, so the runner must start
    inside the workspace.
    """

    def _make(cwd: Path) -> GrimRunner:
        return GrimRunner(grim_binary, grim_home, cwd=cwd)

    return _make
