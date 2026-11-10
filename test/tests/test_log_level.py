# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`--log-level` acceptance tests.

The flag sets the tracing filter for the run and wins over `GRIM_LOG`;
`GRIM_LOG` alone keeps its directive syntax. `grim --global hook list` is the
probe: it needs no registry, and with no hook declared grim itself logs one
DEBUG line. A line from the runtime beneath grim is no probe: the only one
`grim --global context` gets is the Unix signal driver's, so Windows prints none.
"""
from __future__ import annotations

from src.runner import GrimRunner

PROBE = ("--global", "hook", "list")
LEVELS = "off, error, warn, info, debug, trace"


def _run(grim: GrimRunner, *, flag: str | None = None, env: str | None = None):
    if env is None:
        grim.env.pop("GRIM_LOG", None)
    else:
        grim.env["GRIM_LOG"] = env
    return grim.run(*PROBE, log_level=flag)


def test_flag_alone_sets_the_level(grim: GrimRunner) -> None:
    result = _run(grim, flag="debug")

    assert "DEBUG" in result.stderr, result.stderr


def test_default_level_prints_no_trace(grim: GrimRunner) -> None:
    """Positive control: without flag or variable the run is quiet."""
    result = _run(grim)

    assert "TRACE" not in result.stderr
    assert "DEBUG" not in result.stderr


def test_grim_log_alone_still_applies_directives(grim: GrimRunner) -> None:
    result = _run(grim, env="warn,grim=debug")

    assert "DEBUG" in result.stderr, result.stderr


def test_flag_wins_over_a_noisier_grim_log(grim: GrimRunner) -> None:
    result = _run(grim, flag="error", env="debug")

    assert "DEBUG" not in result.stderr, result.stderr


def test_flag_wins_over_a_quieter_grim_log(grim: GrimRunner) -> None:
    result = _run(grim, flag="debug", env="error")

    assert "DEBUG" in result.stderr, result.stderr


def test_invalid_flag_is_reported_once_and_the_run_continues(grim: GrimRunner) -> None:
    result = _run(grim, flag="nonsense")

    assert result.returncode == 0
    lines = [line for line in result.stderr.splitlines() if "nonsense" in line]
    assert len(lines) == 1, result.stderr
    assert LEVELS in lines[0]
    assert "DEBUG" not in result.stderr
    assert "Hook" in result.stdout, "the command still ran"


def test_invalid_flag_falls_back_to_grim_log(grim: GrimRunner) -> None:
    result = _run(grim, flag="nonsense", env="debug")

    assert result.returncode == 0
    assert "DEBUG" in result.stderr, result.stderr
    assert len([line for line in result.stderr.splitlines() if "nonsense" in line]) == 1


def _env_notice(result, value: str) -> list[str]:
    return [line for line in result.stderr.splitlines() if value in line]


def test_unparsable_grim_log_is_reported_once_and_the_run_continues(grim: GrimRunner) -> None:
    result = _run(grim, env="foo=bar")

    assert result.returncode == 0
    lines = _env_notice(result, "foo=bar")
    assert len(lines) == 1, result.stderr
    assert "GRIM_LOG" in lines[0] and LEVELS in lines[0]
    assert "Hook" in result.stdout, "the command still ran"


def test_grim_log_level_typo_is_reported_once_and_the_run_continues(grim: GrimRunner) -> None:
    result = _run(grim, env="debgu")

    assert result.returncode == 0
    lines = _env_notice(result, "debgu")
    assert len(lines) == 1, result.stderr
    assert "GRIM_LOG" in lines[0] and "log target" in lines[0] and LEVELS in lines[0]
    assert "Hook" in result.stdout, "the command still ran"


def test_valid_grim_log_prints_no_notice(grim: GrimRunner) -> None:
    result = _run(grim, env="debug")

    assert "not a level" not in result.stderr
    assert "invalid GRIM_LOG" not in result.stderr
    assert "DEBUG" in result.stderr, result.stderr


def test_valid_flag_over_a_bad_grim_log_prints_no_notice(grim: GrimRunner) -> None:
    result = _run(grim, flag="debug", env="foo=bar")

    assert not _env_notice(result, "foo=bar"), result.stderr
    assert "DEBUG" in result.stderr
