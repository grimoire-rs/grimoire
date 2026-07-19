# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim completions <shell>`` acceptance tests.

Pure command: no project, no registry, no network — the completion script is
generated straight from the clap command tree.
"""
from __future__ import annotations

import pytest

from src.runner import GrimRunner


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish", "elvish", "powershell"])
def test_completions_prints_nonempty_script(grim: GrimRunner, shell: str) -> None:
    """Every supported shell exits 0 with a non-empty completion script."""
    result = grim.plain("completions", shell)

    assert result.returncode == 0, result.stderr
    assert result.stdout.strip(), f"{shell} completion script should not be empty"


def test_completions_bash_defines_grim_function(grim: GrimRunner) -> None:
    result = grim.plain("completions", "bash")

    assert "_grim" in result.stdout
    assert "complete" in result.stdout


def test_completions_zsh_starts_with_compdef(grim: GrimRunner) -> None:
    result = grim.plain("completions", "zsh")

    assert result.stdout.startswith("#compdef grim")


def test_completions_fish_registers_grim_completions(grim: GrimRunner) -> None:
    result = grim.plain("completions", "fish")

    assert "complete -c grim" in result.stdout


def test_completions_missing_shell_exits_usage_error(grim: GrimRunner) -> None:
    result = grim.plain("completions", check=False)

    assert result.returncode == 64, result.stderr


def test_completions_unknown_shell_exits_usage_error(grim: GrimRunner) -> None:
    result = grim.plain("completions", "bogus-shell", check=False)

    assert result.returncode == 64, result.stderr
