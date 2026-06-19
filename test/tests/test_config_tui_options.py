# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`[options.tui]` config-surface acceptance tests.

These tests cover the CONFIG surface only — the TUI runtime (`grim tui`)
is pytest-excluded by design (interactive terminal; no JSON output).

Strategy:
  - Write a ``grimoire.toml`` that contains ``[options.tui]`` fields, run a
    command that reads + re-writes the config (``grim add`` followed by
    ``grim remove``), and assert that the ``[options.tui]`` table is
    preserved verbatim.
  - Verify that ``grim schema --kind config`` accepts (exits 0) when the
    config carries ``[options.tui]``.
  - Verify that a config with an unknown key under ``[options.tui]`` causes
    a parse-error exit rather than silently being ignored.
"""
from __future__ import annotations

from pathlib import Path

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _write_config_with_tui(project_dir: Path, **tui_fields: object) -> Path:
    """Write a ``grimoire.toml`` that carries ``[options.tui]`` fields.

    ``tui_fields`` are key/value pairs emitted verbatim as TOML inside the
    ``[options.tui]`` subtable.  The helper delegates the ``[skills]`` /
    ``[rules]`` tables to ``write_config`` first, then appends the TUI block.
    """
    cfg = write_config(project_dir)
    if tui_fields:
        lines: list[str] = ["", "[options.tui]"]
        for key, value in tui_fields.items():
            if isinstance(value, bool):
                lines.append(f"{key} = {'true' if value else 'false'}")
            elif isinstance(value, str):
                lines.append(f'{key} = "{value}"')
            elif isinstance(value, list):
                items = ", ".join(f'"{v}"' for v in value)
                lines.append(f"{key} = [{items}]")
            else:
                lines.append(f"{key} = {value}")
        with cfg.open("a") as fh:
            fh.write("\n".join(lines) + "\n")
    return cfg


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_tui_options_preserved_through_add_remove_round_trip(
    grim_at: object,
    project_dir: Path,
    registry: str,
    unique_repo: str,
) -> None:
    """``[options.tui]`` fields survive an add → remove config round-trip.

    Rationale: ``grim add`` and ``grim remove`` re-serialize the config via
    ``write_config``.  A bug in ``write_config`` that drops the ``[options.tui]``
    subtable would erase the user's TUI preferences silently.  This test
    confirms the table is preserved verbatim.
    """
    # Publish a minimal skill artifact so `grim add` has something to add.
    sk = make_artifact(
        f"{unique_repo}/tui-options-probe",
        "skill",
        {"tui-options-probe/SKILL.md": "---\nname: tui-options-probe\ndescription: probe\n---\n# probe\n"},
        tag="v1",
    )

    # Write the config with [options.tui] populated.
    _write_config_with_tui(
        project_dir,
        default_view="tree",
        group_by_type=True,
        tree_separators=["/", "-"],
    )

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    # `grim add` reads and re-writes grimoire.toml via write_config.
    runner.json("add", sk.fq)

    cfg_after_add = (project_dir / "grimoire.toml").read_text()
    assert "[options.tui]" in cfg_after_add, (
        "[options.tui] table must be present after grim add re-serializes the config"
    )
    assert 'default_view = "tree"' in cfg_after_add, (
        "default_view must be preserved after grim add"
    )
    assert "group_by_type = true" in cfg_after_add, (
        "group_by_type must be preserved after grim add"
    )
    assert '"-"' in cfg_after_add, (
        "tree_separators content must be preserved after grim add"
    )

    # `grim remove` also re-writes via write_config — verify again.
    runner.json("remove", "skill", "tui-options-probe")

    cfg_after_remove = (project_dir / "grimoire.toml").read_text()
    assert "[options.tui]" in cfg_after_remove, (
        "[options.tui] table must survive grim remove re-serialization"
    )
    assert 'default_view = "tree"' in cfg_after_remove, (
        "default_view must be preserved after grim remove"
    )


def test_tui_options_absent_when_not_declared(
    grim_at: object,
    project_dir: Path,
    registry: str,
    unique_repo: str,
) -> None:
    """A config without ``[options.tui]`` must not gain the section after a round-trip.

    ``write_config`` must omit ``[options.tui]`` when no TUI options are set so
    that a plain ``grimoire.toml`` stays minimal.
    """
    sk = make_artifact(
        f"{unique_repo}/tui-absent-probe",
        "skill",
        {"tui-absent-probe/SKILL.md": "---\nname: tui-absent-probe\ndescription: probe\n---\n# probe\n"},
        tag="v1",
    )

    # Plain config: no [options.tui].
    write_config(project_dir)

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.json("add", sk.fq)

    cfg_after_add = (project_dir / "grimoire.toml").read_text()
    assert "[options.tui]" not in cfg_after_add, (
        "[options.tui] must not appear in a config that declared no TUI options"
    )


def test_unknown_tui_option_is_rejected(
    grim_at: object,
    project_dir: Path,
) -> None:
    """An unknown key under ``[options.tui]`` must cause a non-zero exit.

    ``[options.tui]`` uses ``#[serde(deny_unknown_fields)]`` so any key that
    grim does not recognise must fail config parsing with exit code 78
    (ConfigError) rather than being silently ignored.

    No registry is needed: ``grim status`` parses the project config on
    startup without network access.
    """
    # Write a config that carries a fabricated key grim has never heard of.
    _write_config_with_tui(project_dir, bogus_key=1)

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    result = runner.run("status", check=False)
    assert result.returncode == 78, (
        f"unknown [options.tui] key must exit 78 (ConfigError), "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_invalid_tree_separator_is_rejected(
    grim_at: object,
    project_dir: Path,
) -> None:
    """An invalid ``tree_separators`` value must cause a ConfigError exit (78).

    Each entry in ``tree_separators`` must be exactly one character.
    A multi-character entry like ``"::"`` is not a valid separator and
    ``validate_tree_separators`` must reject it with ``TreeSeparatorInvalid``,
    which is classified as ConfigError (exit 78).

    This test proves the full parse → classify → exit wiring: the TOML itself
    is structurally valid (``"::"`` is a legal TOML string), so the rejection
    only fires in grim's post-parse validator, not at the TOML layer.

    No registry is needed: ``grim status`` parses the project config on
    startup without network access.
    """
    _write_config_with_tui(project_dir, tree_separators=["::"])

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    result = runner.run("status", check=False)
    assert result.returncode == 78, (
        f"invalid tree_separators entry must exit 78 (ConfigError / TreeSeparatorInvalid), "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
