# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``--color`` acceptance tests.

Exercises the color-resolution precedence (flag > NO_COLOR > CLICOLOR_FORCE
> CLICOLOR=0 > TERM=dumb > stdout TTY) against real ``grim`` output.
``grim context --global`` is used as the JSON-emitting probe: it is
read-only, needs no project or registry setup, and never touches the
network. The acceptance suite always pipes stdout, so the default (``auto``)
mode is plain here — ``--color always``/``never`` are what exercise the
colored path.
"""
from __future__ import annotations

import json

from src.runner import GrimRunner

ANSI = "\x1b["


def test_color_flag_appears_in_help(grim: GrimRunner) -> None:
    result = grim.plain("--help")

    assert "--color" in result.stdout


def test_color_always_produces_ansi_json(grim: GrimRunner) -> None:
    result = grim.run("--color", "always", "context", "--global", format="json")

    assert ANSI in result.stdout


def test_color_never_and_default_are_plain_and_identical(grim: GrimRunner) -> None:
    never = grim.run("--color", "never", "context", "--global", format="json")
    default = grim.run("context", "--global", format="json")

    assert ANSI not in never.stdout
    assert ANSI not in default.stdout
    assert never.stdout == default.stdout, "never and piped auto must render byte-identical"
    json.loads(never.stdout)
    json.loads(default.stdout)


def test_no_color_env_forces_plain(grim: GrimRunner) -> None:
    grim.env["NO_COLOR"] = "1"
    result = grim.run("context", "--global", format="json")

    assert ANSI not in result.stdout


def test_clicolor_force_env_colors_even_when_piped(grim: GrimRunner) -> None:
    grim.env["CLICOLOR_FORCE"] = "1"
    result = grim.run("context", "--global", format="json")

    assert ANSI in result.stdout


def test_color_always_flag_beats_no_color_env(grim: GrimRunner) -> None:
    """An explicit ``--color always`` overrides ``NO_COLOR`` — deliberate:
    the flag is the strongest signal, ahead of every environment variable."""
    grim.env["NO_COLOR"] = "1"
    result = grim.run("--color", "always", "context", "--global", format="json")

    assert ANSI in result.stdout


def test_color_bogus_value_exits_usage_error(grim: GrimRunner) -> None:
    result = grim.run("--color", "bogus", "context", "--global", format="json", check=False)

    assert result.returncode == 64, result.stderr
