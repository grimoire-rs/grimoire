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
    """An empty project workspace `grim` runs inside of."""
    d = tmp_path / "project"
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
