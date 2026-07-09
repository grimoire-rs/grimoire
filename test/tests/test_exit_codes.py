# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Exit-code contract tests (docs/src/commands.md "Exit codes").

Regression coverage for the 1.0 exit-code contract: a missing explicit
``--config <path>`` must exit 79 (NotFound), not 74 (IoError), on every
command path that loads the project config.
"""
from __future__ import annotations

from pathlib import Path

import pytest

from src.runner import GrimRunner

# Command paths verified to load the project config via --config.
CONFIG_COMMANDS = [
    ("config", "get", "options.default_registry"),
    ("config", "list"),
    ("status",),
    ("install",),
    ("lock",),
]


@pytest.mark.parametrize(
    "cmd", CONFIG_COMMANDS, ids=lambda c: "-".join(c)
)
def test_missing_explicit_config_exits_79(
    grim: GrimRunner, tmp_path: Path, cmd: tuple[str, ...]
) -> None:
    """Explicit ``--config`` pointing at a missing file exits 79 (NotFound).

    Docs contract (commands.md): "Explicit --config <path> not found, or
    required config absent | 79". Regression: these paths exited 74
    (IoError) because ConfigErrorKind::Io(NotFound) fell through to the
    generic I/O classification.
    """
    missing = tmp_path / "does-not-exist" / "grimoire.toml"
    result = grim.run("--config", str(missing), *cmd, check=False)
    assert result.returncode == 79, (
        f"grim {' '.join(cmd)} --config <missing> must exit 79 (NotFound); "
        f"got {result.returncode}\nstderr: {result.stderr}"
    )
