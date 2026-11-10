# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Guard: every text read or write under ``test/`` names its encoding and newline.

A bare ``read_text()`` decodes with the locale codec and a bare ``write_text()``
turns ``\\n`` into ``\\r\\n`` on Windows, so a fixture written on one platform
differs byte for byte from the same fixture on another. ``ruff``'s PLW1514
cannot check ``newline=``, which is why this is a test.
"""

from __future__ import annotations

import ast
from collections.abc import Iterator
from pathlib import Path

TEST_DIR = Path(__file__).resolve().parent.parent
# test/data holds frozen fixture copies; .venv is third-party code.
SKIPPED_TOP_DIRS = {".venv", "data"}


def _python_files() -> Iterator[Path]:
    for path in sorted(TEST_DIR.rglob("*.py")):
        if path.relative_to(TEST_DIR).parts[0] not in SKIPPED_TOP_DIRS:
            yield path


def _open_mode(call: ast.Call) -> str | None:
    """The literal mode of a builtin ``open`` call, ``"r"`` when absent, None when dynamic."""
    node: ast.expr | None = call.args[1] if len(call.args) > 1 else None
    for keyword in call.keywords:
        if keyword.arg == "mode":
            node = keyword.value
    if node is None:
        return "r"
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _missing(call: ast.Call) -> list[str]:
    """The keywords a text read or write call lacks, empty when it is fine."""
    given = {keyword.arg for keyword in call.keywords}
    func = call.func
    if isinstance(func, ast.Attribute) and func.attr == "read_text":
        return [] if "encoding" in given or call.args else ["encoding"]
    if isinstance(func, ast.Attribute) and func.attr == "write_text":
        if len(call.args) > 1:  # encoding passed positionally
            return []
        return [name for name in ("encoding", "newline") if name not in given]
    if isinstance(func, ast.Name) and func.id == "open":
        mode = _open_mode(call)
        if mode is None or "b" in mode or len(call.args) > 2:
            return []
        wanted = ("encoding", "newline") if any(c in mode for c in "wax+") else ("encoding",)
        return [name for name in wanted if name not in given]
    return []


def bare_text_io(root: Path = TEST_DIR) -> list[str]:
    """``file:line: call lacks kw`` for every bare text read or write under ``root``."""
    found: list[str] = []
    for path in _python_files():
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and (missing := _missing(node)):
                rel = path.relative_to(root.parent)
                found.append(f"{rel}:{node.lineno}: missing {', '.join(missing)}")
    return sorted(found)


def test_no_bare_text_io() -> None:
    found = bare_text_io()
    assert not found, "text I/O without encoding=/newline=:\n" + "\n".join(found)
