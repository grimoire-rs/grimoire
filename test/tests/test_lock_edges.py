# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Lock writers at the edges unit tests cannot reach."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

from src.helpers import held_flock, write_config

_LOCK = (
    "[metadata]\n"
    "lock_version = 1\n"
    "declaration_hash_version = 1\n"
    f'declaration_hash = "sha256:{"0" * 64}"\n'
    'generated_by = "grim 0.15.1"\n'
    'generated_at = "2026-01-01T00:00:00Z"\n\n'
    '[[skill]]\nname = "alpha"\n'
    f'pinned = "localhost:5000/acme/alpha:1@sha256:{"1" * 64}"\n'
)


@pytest.mark.skipif(
    sys.platform == "win32", reason="flock(2) contention needs a Unix host"
)
@pytest.mark.parametrize(
    "argv",
    [
        ("lock",),
        ("remove", "skill", "alpha"),
        ("uninstall", "skill", "alpha"),
        ("update",),
        ("update", "--", "alpha"),
    ],
    ids=" ".join,
)
def test_a_lock_writer_refuses_while_another_holds_the_flock(
    grim_at, project_dir: Path, argv: tuple[str, ...]
) -> None:
    """RF1 concurrent writers: the flock these commands hold around a relock.

    `lock_io::save` trusts the `previous` it was handed, so a writer without the
    flock overwrites a lock another writer just saved. The flock comes ahead of
    any network, so no registry is needed.
    """
    write_config(project_dir, skills={"alpha": "localhost:5000/acme/alpha:1"})
    config = project_dir / "grimoire.toml"
    lock = project_dir / "grimoire.lock"
    lock.write_text(_LOCK, encoding="utf-8")
    config_bytes = config.read_bytes()

    with held_flock(project_dir / "grimoire.toml.lock"):
        result = grim_at(project_dir).run(*argv, check=False)

    assert result.returncode == 75, result.stderr
    assert config.read_bytes() == config_bytes
    assert lock.read_text(encoding="utf-8") == _LOCK
