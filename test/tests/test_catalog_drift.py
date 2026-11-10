# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Catalog drift gates: grim-usage's Reference Syntax and the publish version.

Every reference form the skill documents is fed to the real binary. Config
values go through `grim context`, the cheapest command that parses
`grimoire.toml` declarations and never dials a registry; command arguments go
through each command the section names, offline.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest
import tomllib

from src.helpers import PROJECT_ROOT

_SKILL = PROJECT_ROOT / "catalog" / "skills" / "grim-usage" / "SKILL.md"
_CONSUME = _SKILL.parent / "references" / "consume.md"
_USAGE_ERROR = 64  # the command does not take this kind of argument
_DATA_ERROR = 65  # DataError (EX_DATAERR): the parser refused the value

_DIGEST = "sha256:" + "a" * 64
_ALL_TABLES = ("skills", "rules", "agents", "bundles", "mcp", "hooks", "settings")

_ALIAS, _ALIAS_OCI = "acme", "ghcr.io/acme"
_REGISTRY_ALIAS = f'[[registries]]\nalias = "{_ALIAS}"\noci = "{_ALIAS_OCI}"\n\n'

# Commands whose path handling is pinned in both directions: each one the
# Reference Syntax names must take a local path, and each one it leaves out
# must refuse it. A command the section starts naming needs a row here.
_PATH_COMMANDS: dict[str, tuple[str, ...]] = {
    "add": ("add", "--no-install"),
    "install": ("install",),
    "export plugin": ("export", "plugin", "-o", "{out}"),
    "describe": ("describe",),
    "fetch": ("fetch",),
}

# The same, for the short and qualified argument forms. `install` refuses
# them: its only positional is a dev-install path.
_REFERENCE_COMMANDS: dict[str, tuple[str, ...]] = {
    "add": ("--offline", "add", "--kind", "skill", "--no-install"),
    "describe": ("--offline", "describe"),
    "install": ("--offline", "install"),
}


def _reference_syntax() -> str:
    text = _SKILL.read_text(encoding="utf-8")
    start = text.index("## Reference Syntax")
    end = text.index("\n## ", start + 1)
    return " ".join(text[start:end].split())


def _sentence(pattern: str) -> str:
    match = re.search(pattern, _reference_syntax())
    assert match, f"Reference Syntax lacks a sentence matching {pattern!r}"
    return match.group(0)


def _tables(sentence_pattern: str) -> set[str]:
    """Tables named in the Reference Syntax sentence matching the pattern."""
    return set(re.findall(r"`\[(\w+)\]`", _sentence(sentence_pattern)))


def _commands(sentence_pattern: str) -> set[str]:
    """`grim` commands named in the Reference Syntax sentence matching the pattern."""
    return set(re.findall(r"`grim ([a-z][a-z ]*)`", _sentence(sentence_pattern)))


_PATH_SENTENCE = r"Path values are accepted in [^.]*\."
_ARGUMENT_SENTENCE = r"The short and qualified forms below are for [^.]*\."


def _path_accepting() -> set[str]:
    return _tables(_PATH_SENTENCE)


def _registry_only() -> set[str]:
    return _tables(r"[^.]*take registry references only")


def _declarable_forms() -> dict[str, str]:
    """Forms a `grimoire.toml` value may carry: always fully qualified."""
    return {
        "tagged": "ghcr.io/acme/code-review:1",
        "digest": f"ghcr.io/acme/code-review@{_DIGEST}",
        "bare": "ghcr.io/acme/code-review",
    }


def _argument_forms() -> dict[str, tuple[str, str]]:
    """Argument-only form -> (argument, the reference it expands to).

    Both come from the section's own text: its alias example and its
    built-in fallback registry.
    """
    section = _reference_syntax()
    fallback = re.search(r"built-in fallback registry `([^`]+)`", section)
    example = re.search(
        r"for example `(\w+)/([^`]+)` becomes `([^`]+)` when `\1` is aliased to `([^`]+)`",
        section,
    )
    assert fallback and example, "Reference Syntax lost its fallback or alias example"
    alias, rest, expanded, oci = example.groups()
    assert (alias, oci) == (_ALIAS, _ALIAS_OCI), (
        "the example must match _REGISTRY_ALIAS"
    )
    return {
        "short": (rest, f"{fallback.group(1)}/{rest}"),
        "qualified": (f"{alias}/{rest}", expanded),
    }


def _path_forms(tmp_path: Path) -> dict[str, str]:
    return {
        "dot": "./skills/x",
        "dotdot": "../shared/rule.md",
        "absolute": (tmp_path / "abs" / "x").as_posix(),
    }


def _context(grim_at, project_dir: Path, table: str, value: str):
    # JSON escapes quotes, backslashes and C0 controls as TOML does; TOML also bans a raw DEL.
    quoted = json.dumps(value, ensure_ascii=False).replace("\x7f", "\\u007f")
    (project_dir / "grimoire.toml").write_text(f"{_REGISTRY_ALIAS}[{table}]\nbinding = {quoted}\n")
    return grim_at(project_dir).run("context", check=False)


@pytest.mark.parametrize("table", _ALL_TABLES)
@pytest.mark.parametrize("form", list(_declarable_forms()))
def test_declarable_reference_forms_parse_in_every_table(
    grim_at, project_dir: Path, table: str, form: str
) -> None:
    value = _declarable_forms()[form]
    result = _context(grim_at, project_dir, table, value)
    assert result.returncode == 0, f"[{table}] {form} {value!r}: {result.stderr}"


@pytest.mark.parametrize("table", _ALL_TABLES)
@pytest.mark.parametrize("form", ["short", "qualified"])
def test_documented_config_values_are_fully_qualified(
    grim_at, project_dir: Path, table: str, form: str
) -> None:
    assert "A `grimoire.toml` value must be fully qualified" in _reference_syntax()
    value, _ = _argument_forms()[form]
    # The alias is declared, so a refusal is the parser's, not a lookup miss.
    result = _context(grim_at, project_dir, table, value)
    assert result.returncode == _DATA_ERROR, (
        f"[{table}] {form} {value!r} documented as argument-only but exited "
        f"{result.returncode}: {result.stderr}"
    )


@pytest.mark.parametrize("command", list(_REFERENCE_COMMANDS))
@pytest.mark.parametrize("form", ["short", "qualified"])
def test_short_and_qualified_forms_follow_the_documented_commands(
    grim_at, project_dir: Path, command: str, form: str
) -> None:
    config = "" if form == "short" else _REGISTRY_ALIAS
    (project_dir / "grimoire.toml").write_text(config)
    value, expanded = _argument_forms()[form]
    result = grim_at(project_dir).run(*_REFERENCE_COMMANDS[command], value, check=False)
    if command in _commands(_ARGUMENT_SENTENCE):
        # Parsed and expanded before the (blocked) network step.
        assert result.returncode not in (_USAGE_ERROR, _DATA_ERROR), result.stderr
        assert expanded in result.stderr, f"grim {command} {value!r} did not expand"
    else:
        assert result.returncode == _USAGE_ERROR, (
            f"grim {command} {value!r} is not documented to take it but exited "
            f"{result.returncode}: {result.stderr}"
        )


def test_documented_commands_have_a_pinned_row() -> None:
    assert _commands(_PATH_SENTENCE) <= set(_PATH_COMMANDS)
    assert _commands(_ARGUMENT_SENTENCE) <= set(_REFERENCE_COMMANDS)


def _skill_path(project_dir: Path, tmp_path: Path, form: str) -> str:
    """A minimal skill on disk, named by the path form a command receives."""
    target, argument = {
        "dot": (project_dir / "skills" / "x", "./skills/x"),
        "dotdot": (tmp_path / "shared" / "x", "../shared/x"),
        "absolute": (tmp_path / "abs" / "x", str(tmp_path / "abs" / "x")),
    }[form]
    target.mkdir(parents=True)
    (target / "SKILL.md").write_text("---\nname: x\ndescription: X.\n---\n# X\n")
    return argument


@pytest.mark.parametrize("form", ["dot", "dotdot", "absolute"])
@pytest.mark.parametrize("command", list(_PATH_COMMANDS))
def test_path_arguments_follow_the_documented_commands(
    grim_at, project_dir: Path, tmp_path: Path, command: str, form: str
) -> None:
    (project_dir / "grimoire.toml").write_text("")
    value = _skill_path(project_dir, tmp_path, form)
    argv = [a.format(out=tmp_path / "out") for a in _PATH_COMMANDS[command]]
    runner = grim_at(project_dir)
    runner.env["GRIM_OFFLINE"] = "1"
    result = runner.run(*argv, value, check=False)
    if command in _commands(_PATH_SENTENCE):
        assert result.returncode == 0, f"grim {command} {value!r}: {result.stderr}"
    else:
        assert result.returncode in (_USAGE_ERROR, _DATA_ERROR), (
            f"grim {command} {value!r} takes a path the Reference Syntax does "
            f"not name: exited {result.returncode}"
        )


@pytest.mark.parametrize("form", ["dot", "dotdot", "absolute"])
@pytest.mark.parametrize("table", _ALL_TABLES)
def test_path_values_follow_the_documented_tables(
    grim_at, project_dir: Path, tmp_path: Path, table: str, form: str
) -> None:
    value = _path_forms(tmp_path)[form]
    result = _context(grim_at, project_dir, table, value)
    if table in _path_accepting():
        assert result.returncode == 0, f"[{table}] {form} {value!r}: {result.stderr}"
    else:
        assert table in _registry_only()
        assert result.returncode == _DATA_ERROR, (
            f"[{table}] {form} {value!r} documented as refused but exited "
            f"{result.returncode}"
        )


def test_documented_table_split_is_exhaustive() -> None:
    assert _path_accepting() | _registry_only() == set(_ALL_TABLES)
    assert _path_accepting() & _registry_only() == set()


def test_consume_reference_agrees_with_the_reference_syntax() -> None:
    consume = " ".join(_CONSUME.read_text(encoding="utf-8").split())
    refused = re.search(r"refuses a path value under [^,]*(?:, [^,]*)*?\]`,", consume)
    assert refused, "consume.md lost its registry-only table sentence"
    assert set(re.findall(r"`\[(\w+)\]`", refused.group(0))) == _registry_only()
    assert "`grim install <path>`" in consume, "consume.md lost the dev-install form"


def test_publish_manifest_version_equals_workspace_version() -> None:
    cargo = tomllib.loads((PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    publish = tomllib.loads(
        (PROJECT_ROOT / "catalog" / "publish.toml").read_text(encoding="utf-8")
    )
    assert publish["version"] == cargo["workspace"]["package"]["version"]
