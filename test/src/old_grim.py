# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The grim 0.15.1 binary the old-grim interop modules run against."""

from __future__ import annotations

import os
from pathlib import Path

from src.helpers import write_config
from src.runner import GrimRunner

OLD_BIN = os.environ.get("GRIM_OLD_BIN")

# `transitive` arrived after the tag, so 0.15.1 refuses it as an unknown field.
_LOCK_WITH_A_LATER_FIELD = (
    "[metadata]\n"
    "lock_version = 1\n"
    "declaration_hash_version = 1\n"
    f'declaration_hash = "sha256:{"0" * 64}"\n'
    'generated_by = "grim 0.16.0"\n'
    'generated_at = "2026-01-01T00:00:00Z"\n\n'
    '[[bundle]]\nname = "b"\nrepo = "localhost:5000/x/b"\ntag = "1"\n'
    f'pinned = "localhost:5000/x/b@sha256:{"2" * 64}"\n'
    "transitive = true\n"
)


def checked_old_grim(root: Path) -> Path:
    """`GRIM_OLD_BIN`, refused unless it behaves like the 0.15.1 release.

    `--version` alone cannot refuse a dev build: `Cargo.toml` stays at 0.15.1
    until the next release, so builds of later commits print the same string.
    A field added after the tag tells them apart, because 0.15.1 denies unknown
    fields. A wrong binary errors rather than skips, so it cannot read as a pass.
    """
    assert OLD_BIN is not None
    binary = Path(OLD_BIN)
    grim_home = root / "grim-home"
    grim_home.mkdir()
    project = root / "project"
    project.mkdir()
    runner = GrimRunner(binary, grim_home, cwd=project)

    version = runner.run("--version").stdout.strip()
    assert version == "grim 0.15.1", f"GRIM_OLD_BIN is not 0.15.1: {version!r}"

    write_config(project, bundles={"b": "localhost:5000/x/b:1"})
    (project / "grimoire.lock").write_text(_LOCK_WITH_A_LATER_FIELD, encoding="utf-8", newline="\n")
    refused = runner.run("status", check=False)
    assert refused.returncode == 78 and "transitive" in refused.stderr, (
        "GRIM_OLD_BIN prints 0.15.1 but accepts a field added after the tag, "
        f"so it is a dev build: exit {refused.returncode}, {refused.stderr!r}"
    )
    return binary
