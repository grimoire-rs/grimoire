# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The real hook launcher, installed by ``grim install`` and fired as a client does.

The launcher is ``$GRIM_HOME/hooks/grim-hook``, written by ``launcher::ensure``
the first time a hook arms.  A test that needs it arms one hook with a real
``grim install`` and takes the path back; it never copies a ``target/`` artifact,
so the bytes under test are the bytes a user gets.

Arming also writes a ``dispatch.json`` beside the launcher.  A test that wants a
table of its own overwrites that file: the launcher reads the table beside itself
and nothing else, so the arming row is gone the moment the file is replaced.

Imports ``src.helpers`` (and through it ``src.registry``), so it must be imported
by a test module, never from ``src/__init__.py``.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner

HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes Bash tool calls before they run."

[[hooks]]
id = "guard"
event = "PreToolUse"
tier = "observer"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

GUARD_SH = "#!/bin/sh\ncat > /dev/null\nprintf '{}'\n"


def publish_hook(unique_repo: str):
    """Push the one-hook artifact the arming install declares."""
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def install_launcher(runner: GrimRunner, unique_repo: str) -> Path:
    """Arm one hook in ``runner``'s workspace and return ``$GRIM_HOME/hooks/grim-hook``.

    ``runner`` must have a project directory as its cwd (``grim_at(project_dir)``).
    The path is asserted to be an executable regular file, so a build that stopped
    writing the launcher fails here and not as a confusing ``FileNotFoundError``
    in whichever test fires it first.
    """
    assert runner.cwd is not None, "install_launcher needs a runner inside a workspace"
    hook = publish_hook(unique_repo)
    write_config(runner.cwd, hooks={"shell-guard": hook.fq})
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--trust-hooks")
    launcher = Path(runner.grim_home) / "hooks" / "grim-hook"
    assert launcher.is_file() and os.access(launcher, os.X_OK), (
        f"`grim install --trust-hooks` armed a hook but left no executable launcher at {launcher}"
    )
    return launcher


def run_launcher(
    launcher: Path | str,
    *operands: str,
    stdin: str = "",
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 60,
) -> subprocess.CompletedProcess[str]:
    """Fire ``grim-hook <operands>`` as a client's exec-form registration does.

    The operands are normally ``client event root``; they are passed through
    unchecked so a test can send the malformed shapes the launcher must refuse.

    ``check`` is never on: the exit code is usually the assertion.  The default
    environment is just ``PATH`` (the payload fixtures are ``sh`` scripts), with no
    ``GRIM_HOME`` — the launcher resolves its table from its own location, and a
    test that passed only because the variable was set would prove nothing.
    """
    return subprocess.run(
        [str(launcher), *operands],
        input=stdin,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env={"PATH": os.environ.get("PATH", "")} if env is None else env,
        cwd=str(cwd) if cwd else None,
        timeout=timeout,
        check=False,
    )
