# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim config` settings acceptance tests (get/set/unset/list).

Additional coverage (review-fix round 1):
- D2: group_by_type set→get→unset, tree_separators round-trip, list on empty.
- F4: Concurrency smoke — two simultaneous ``config set`` calls.

``options.clients`` is a ``string-set`` (closed vocabulary, unordered
semantically): each comma segment must name a supported ``ClientTarget``
(``claude``/``opencode``/``copilot``); an unknown or duplicate name exits
65 (DataError). Input order is preserved on store/echo; the JSON ``values``
metadata field always lists the canonical ``ClientTarget::ALL`` order.


Specification-phase suite: every test expresses expected behavior from
``adr_grim_config_command.md`` and ``plan_grim_config.md``.  All tests
FAIL against the Phase-3 stubs (``run`` body is ``unimplemented!()``).

Behaviors covered:
- ``set`` → ``get`` round-trips for ``options.clients``,
  ``options.tui.default_view``, and ``options.default_registry`` at both
  project scope and ``--global`` (writes ``$GRIM_HOME/grimoire.toml``).
- ``get`` of a valid-but-unset key exits 1 with no stdout (git-compatible).
- Unknown dotted key (typo) exits 64 (UsageError).
- Invalid enum value exits 65 (DataError).
- ``unset`` removes a key (subsequent ``get`` exits 1).
- ``list`` outputs ``key=value`` lines; ``--all`` widens the row set to
  include supported-but-unset keys (metadata shape unchanged either way).
- ``--format json`` shapes for ``set`` write-confirmation and ``list``.
- ``--global`` writes to ``$GRIM_HOME/grimoire.toml``, never the project
  config (scope separation invariant).
"""
from __future__ import annotations

import json
import subprocess
import threading
from pathlib import Path

import tomllib  # stdlib (Python 3.11+)

from src.helpers import write_config
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


# The 7 fixed option keys ``--all`` must surface unset (I3 frozen spec).
FIXED_OPTION_KEYS = [
    "options.default_registry",
    "options.clients",
    "options.show_deprecated",
    "options.tui.default_view",
    "options.tui.group_by_type",
    "options.tui.tree_separators",
    "options.tui.expand_levels",
]

_ALLOWED_TYPES = {"string", "boolean", "integer", "enum", "string-list", "string-set"}

# The canonical ClientTarget::ALL order — every ``options.clients`` "values"
# JSON field pins this order regardless of the order the user supplied.
CLIENT_VALUE_NAMES = ["claude", "opencode", "copilot", "codex"]


def _minimal_global_config(grim_home: Path) -> None:
    """Write a minimal valid ``grimoire.toml`` in ``$GRIM_HOME``.

    ``grim config --global set`` requires the global config file to exist
    (scope resolution fails with NotFound 79 when absent).  This helper
    provides the minimal valid skeleton so tests focus on config-command
    behavior, not on config file creation.
    """
    (grim_home / "grimoire.toml").write_text("[skills]\n\n[rules]\n")


# ---------------------------------------------------------------------------
# Round-trip tests — project scope
# ---------------------------------------------------------------------------


def test_set_get_round_trip_options_clients_project_scope(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set`` then ``get`` returns the value for ``options.clients`` at
    project scope.

    The value is a comma-separated client list (``claude,opencode``).  The
    plain ``get`` output must be the bare value — no key name, no table —
    on stdout with exit 0.

    Traces to ADR: key-namespace table row ``options.clients``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "claude,opencode")

    result = runner.plain("config", "get", "options.clients")
    assert result.returncode == 0, (
        f"get of a set key must exit 0; got {result.returncode}\n"
        f"stderr: {result.stderr.strip()}"
    )
    # Plain output is the bare value; both client names must appear.
    assert "claude" in result.stdout, (
        f"plain get must include 'claude' in output; got: {result.stdout!r}"
    )
    assert "opencode" in result.stdout, (
        f"plain get must include 'opencode' in output; got: {result.stdout!r}"
    )


def test_set_clients_multi_valid_round_trips_preserving_input_order(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.clients claude,copilot`` round-trips through ``get`` and
    ``list``, preserving the exact input order.

    ``options.clients`` is a ``string-set`` (unordered, closed vocabulary),
    but storage/echo preserves what the user typed rather than reordering
    to the canonical ``ClientTarget::ALL`` order (``claude, opencode,
    copilot``) — only the metadata ``values`` field uses that canonical
    order.

    Traces to the frozen contract: "Input order preserved on store".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "claude,copilot")

    get_result = runner.plain("config", "get", "options.clients")
    assert get_result.returncode == 0, (
        f"get of set options.clients must exit 0; got {get_result.returncode}\n"
        f"stderr: {get_result.stderr.strip()}"
    )
    assert get_result.stdout.strip() == "claude,copilot", (
        f"get must echo the input order verbatim, not the canonical "
        f"ClientTarget::ALL order; got: {get_result.stdout!r}"
    )

    list_items = runner.json("config", "list")["items"]
    entry = next(i for i in list_items if i["key"] == "options.clients")
    assert entry["value"] == "claude,copilot", (
        f"list value must preserve the input order; got: {entry!r}"
    )


def test_set_get_round_trip_tui_default_view_project_scope(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set`` then ``get`` returns the value for ``options.tui.default_view``.

    ``tree`` is a valid enum variant.  The plain ``get`` output must
    contain the string ``tree`` and exit 0.

    Traces to ADR: key-namespace table row ``options.tui.default_view``,
    valid values ``flat`` | ``tree``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.tui.default_view", "tree")

    result = runner.plain("config", "get", "options.tui.default_view")
    assert result.returncode == 0, (
        f"get of set tui.default_view must exit 0; got {result.returncode}\n"
        f"stderr: {result.stderr.strip()}"
    )
    assert "tree" in result.stdout, (
        f"plain get must return 'tree'; got: {result.stdout!r}"
    )


def test_set_get_round_trip_options_default_registry_project_scope(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set`` then ``get`` returns the value for ``options.default_registry``.

    The legacy ``[options].default_registry`` field is string-valued;
    get/set must be allowed per the ADR (though ``registry use`` is
    the preferred modern path).

    Traces to ADR: key-namespace table row ``options.default_registry``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.default_registry", "ghcr.io/acme")

    result = runner.plain("config", "get", "options.default_registry")
    assert result.returncode == 0, (
        f"get of set default_registry must exit 0; got {result.returncode}\n"
        f"stderr: {result.stderr.strip()}"
    )
    assert "ghcr.io/acme" in result.stdout, (
        f"plain get must return the registry URL; got: {result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# Round-trip tests — global scope
# ---------------------------------------------------------------------------


def test_set_get_round_trip_options_clients_global_scope(
    grim_binary: Path,
    grim_home: Path,
) -> None:
    """``--global set`` then ``--global get`` round-trips ``options.clients``.

    The global config file is ``$GRIM_HOME/grimoire.toml``.  Both the set
    and get must target it, not any project config.

    Traces to ADR: ``--global`` flag selects ``$GRIM_HOME/grimoire.toml``.
    """
    _minimal_global_config(grim_home)
    runner = GrimRunner(grim_binary, grim_home)

    runner.run("config", "--global", "set", "options.clients", "claude")

    result = runner.plain("config", "--global", "get", "options.clients")
    assert result.returncode == 0, (
        f"--global get of set key must exit 0; got {result.returncode}\n"
        f"stderr: {result.stderr.strip()}"
    )
    assert "claude" in result.stdout, (
        f"plain --global get must return the set value; got: {result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# Error-path tests
# ---------------------------------------------------------------------------


def test_get_valid_but_unset_key_exits_1_with_empty_stdout(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``get`` of a valid-but-unset key exits 1 and emits nothing on stdout.

    This is the git-compatible script contract:
    ``grim config get options.clients || echo default``

    Traces to ADR: "get of a valid-but-unset key → exit 1 (Failure), no
    stdout".
    """
    write_config(project_dir)  # no options set — all keys unset
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "get", "options.clients", check=False)

    assert result.returncode == 1, (
        f"get of valid-but-unset key must exit 1, got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert result.stdout.strip() == "", (
        f"get of unset key must produce no stdout; got: {result.stdout!r}"
    )


def test_get_unknown_key_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``get`` of a key with an unknown root segment exits 64 (UsageError).

    ``optins.clients`` is a typo; the valid root is ``options``.  The
    command must reject it before attempting a config file read.

    Traces to ADR: "Unknown key name … → UsageError 64".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "get", "optins.clients", check=False)

    assert result.returncode == 64, (
        f"unknown key must exit 64 (UsageError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )


def test_set_invalid_tui_default_view_value_exits_65(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.tui.default_view bogus`` exits 65 (DataError).

    ``bogus`` is not a valid ``DefaultView`` enum variant (valid: ``flat``,
    ``tree``).  The command must reject bad enum values before writing.

    Traces to ADR: "Invalid value format (bad enum …) → DataError 65".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.tui.default_view", "bogus", check=False
    )

    assert result.returncode == 65, (
        f"invalid enum value must exit 65 (DataError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# Unset test
# ---------------------------------------------------------------------------


def test_unset_removes_previously_set_key(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``unset`` of a key makes the subsequent ``get`` exit 1 (unset contract).

    Sequence: set → get (exit 0) → unset → get (exit 1, no stdout).

    Traces to ADR: ``grim config unset <key>`` removes a key; subsequent
    get of the now-absent key must return exit 1.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "claude")

    # Verify the key is set before unset.
    set_result = runner.run("config", "get", "options.clients", check=False)
    assert set_result.returncode == 0, "pre-condition: key must be set before unset"

    runner.run("config", "unset", "options.clients")

    # After unset the key must behave as if it was never set.
    unset_result = runner.run("config", "get", "options.clients", check=False)
    assert unset_result.returncode == 1, (
        f"get after unset must exit 1, got {unset_result.returncode}; "
        f"stderr: {unset_result.stderr.strip()}"
    )
    assert unset_result.stdout.strip() == "", (
        f"get after unset must produce no stdout; got: {unset_result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# List tests
# ---------------------------------------------------------------------------


def test_list_plain_contains_key_and_value(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``list`` in plain mode shows all effective key=value pairs.

    After setting ``options.clients``, ``grim config list`` must include
    the key name and its value in stdout.  Exit must be 0.

    Traces to ADR: "list: plain ``key=value`` lines; one table per
    invocation".
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "claude")

    result = runner.plain("config", "list")
    assert result.returncode == 0, (
        f"list must exit 0; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "options.clients" in result.stdout, (
        f"list output must contain the key name; got:\n{result.stdout}"
    )
    assert "claude" in result.stdout, (
        f"list output must contain the value; got:\n{result.stdout}"
    )


# ---------------------------------------------------------------------------
# JSON output shape tests
# ---------------------------------------------------------------------------


def test_set_json_write_confirmation_carries_action_key_value_scope(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set`` with ``--format json`` returns a write-confirmation object
    with ``action``, ``key``, ``value``, ``scope``, and ``dry_run`` fields.

    Traces to ADR: ConfigWriteReport JSON shape
    ``{"action":"…","key":"…","value":"…","scope":"…","dry_run":bool}``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    # runner.json() fails against stub (binary panics) → test fails.
    # After implementation it returns the write-confirmation object.
    result = runner.json("config", "set", "options.clients", "claude")

    assert "action" in result, (
        f"write-confirmation JSON must have 'action' field; got: {result!r}"
    )
    assert result.get("key") == "options.clients", (
        f"'key' field must be the dotted key; got: {result.get('key')!r}"
    )
    assert "claude" in str(result.get("value", "")), (
        f"'value' field must contain the new value; got: {result.get('value')!r}"
    )
    assert result.get("scope") == "project", (
        f"'scope' field must be 'project' for project-scope set; "
        f"got: {result.get('scope')!r}"
    )
    assert result.get("dry_run") is False, (
        f"a real (non-dry-run) set must report dry_run:false; got: {result.get('dry_run')!r}"
    )


def test_get_json_format_when_key_is_set(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``get --format json`` returns an object with ``key``, ``value``,
    ``set``, and ``scope`` when the key is set.

    Traces to ADR / F1/W1: ConfigGetReport JSON shape
    ``{"key":"…","value":"…","set":true,"scope":"…"}``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "opencode")

    result = runner.json("config", "get", "options.clients")

    assert result.get("key") == "options.clients", (
        f"JSON get must carry the queried key; got: {result!r}"
    )
    value = result.get("value")
    assert value is not None and "opencode" in str(value), (
        f"JSON get must carry the value when set; got value={value!r}"
    )
    assert result.get("set") is True, (
        f"JSON get must have 'set': true when key is set; got: {result!r}"
    )
    assert result.get("scope") == "project", (
        f"JSON get must have 'scope': 'project' for project-scope get; "
        f"got: {result!r}"
    )


def test_list_json_format_is_parseable_array(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``list --format json`` returns a JSON array of key/value entry objects.

    Traces to ADR / ConfigListReport doc: JSON format is an array of
    ``{"key":"…","value":"…"}`` objects (not wrapped in a parent object).
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "claude")

    result = runner.json("config", "list")["items"]

    assert isinstance(result, list), (
        f"list --format json must return a JSON array; got: {type(result)}"
    )
    keys = [e.get("key") for e in result if isinstance(e, dict)]
    assert "options.clients" in keys, (
        f"JSON list must contain 'options.clients' entry; keys: {keys}"
    )


# ---------------------------------------------------------------------------
# Scope isolation test
# ---------------------------------------------------------------------------


def test_global_flag_writes_grim_home_config_not_project_config(
    grim_at: object,
    project_dir: Path,
    grim_home: Path,
) -> None:
    """``--global`` writes to ``$GRIM_HOME/grimoire.toml``, never the project.

    Setting a value at global scope must not appear in the project config.
    The project and global configs are distinct files, never merged.

    Traces to ADR: "Two scopes, **never merged**"; "``--global`` selects
    ``$GRIM_HOME/grimoire.toml``".
    """
    write_config(project_dir)
    _minimal_global_config(grim_home)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "--global", "set", "options.clients", "opencode")

    # Global config must contain the new value.
    global_cfg = (grim_home / "grimoire.toml").read_text()
    assert "opencode" in global_cfg, (
        f"$GRIM_HOME/grimoire.toml must contain the globally-set value; "
        f"got:\n{global_cfg}"
    )

    # Project config must be unchanged — it must NOT contain "opencode".
    project_cfg = (project_dir / "grimoire.toml").read_text()
    assert "opencode" not in project_cfg, (
        f"project grimoire.toml must not be modified by --global set; "
        f"got:\n{project_cfg}"
    )


# ---------------------------------------------------------------------------
# D2: group_by_type coverage
# ---------------------------------------------------------------------------


def test_group_by_type_set_get_unset(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.tui.group_by_type true`` → ``get`` exits 0 with 'true';
    ``unset`` → subsequent ``get`` exits 1 (treated as unset when false).

    Traces to ADR / F2/D1: ``group_by_type`` returns ``None`` when ``false``
    so ``get`` and ``list`` treat it the same as an absent key.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.tui.group_by_type", "true")

    result = runner.plain("config", "get", "options.tui.group_by_type")
    assert result.returncode == 0, (
        f"get of set group_by_type must exit 0; got {result.returncode}"
    )
    assert "true" in result.stdout, (
        f"get must return 'true'; got: {result.stdout!r}"
    )

    runner.run("config", "unset", "options.tui.group_by_type")

    after = runner.run("config", "get", "options.tui.group_by_type", check=False)
    assert after.returncode == 1, (
        f"get of unset group_by_type must exit 1; got {after.returncode}"
    )
    assert after.stdout.strip() == "", (
        f"get after unset must produce no stdout; got: {after.stdout!r}"
    )


def test_show_deprecated_set_get_unset(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.show_deprecated true`` → ``get`` exits 0 with 'true';
    ``unset`` → subsequent ``get`` exits 1 (treated as unset when false).

    Mirrors ``group_by_type``: the top-level ``show_deprecated`` bool returns
    ``None`` when ``false`` so ``get`` and ``list`` treat it as an absent key.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.show_deprecated", "true")

    result = runner.plain("config", "get", "options.show_deprecated")
    assert result.returncode == 0, (
        f"get of set show_deprecated must exit 0; got {result.returncode}"
    )
    assert "true" in result.stdout, f"get must return 'true'; got: {result.stdout!r}"

    listed = runner.plain("config", "list")
    assert "options.show_deprecated" in listed.stdout, (
        f"list must show the set key; got: {listed.stdout!r}"
    )

    runner.run("config", "unset", "options.show_deprecated")

    after = runner.run("config", "get", "options.show_deprecated", check=False)
    assert after.returncode == 1, (
        f"get of unset show_deprecated must exit 1; got {after.returncode}"
    )
    assert after.stdout.strip() == "", (
        f"get after unset must produce no stdout; got: {after.stdout!r}"
    )


# ---------------------------------------------------------------------------
# D2: tree_separators round-trip
# ---------------------------------------------------------------------------


def test_tree_separators_set_get_round_trip(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.tui.tree_separators /,-`` → ``get`` returns the value.

    Traces to ADR / D2: tree_separators round-trip via config get/set.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.tui.tree_separators", "/,-")

    result = runner.plain("config", "get", "options.tui.tree_separators")
    assert result.returncode == 0, (
        f"get of set tree_separators must exit 0; got {result.returncode}"
    )
    assert "/" in result.stdout, (
        f"get must include '/'; got: {result.stdout!r}"
    )
    assert "-" in result.stdout, (
        f"get must include '-'; got: {result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# FIX 3: empty/whitespace client segment rejected (exit 65)
# ---------------------------------------------------------------------------


def test_set_clients_empty_segment_exits_65(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set options.clients`` with an empty segment exits 65 (DataError).

    ``claude, ,opencode`` splits on ``,`` and trims to ``['claude', '', 'opencode']``.
    The empty segment must be rejected before writing so the config never
    holds a blank client name that silently installs nothing.

    Traces to FIX 3: reject any empty/whitespace-only segment → exit 65.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.clients", "claude, ,opencode",
        check=False,
    )
    assert result.returncode == 65, (
        f"empty segment in clients must exit 65 (DataError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# options.clients is a closed string-set: unknown/duplicate names rejected
# ---------------------------------------------------------------------------


def test_set_clients_unknown_name_exits_65(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set options.clients`` with an unrecognized client name exits
    65 (DataError); the message names the bad value and lists the valid set.

    ``options.clients`` is a ``string-set`` drawn from the closed
    ``ClientTarget`` vocabulary — each comma segment must parse via
    ``ClientTarget::from_str``. ``vscode`` is not a supported client.

    Traces to the frozen validation: unknown name → DataError 65, message
    ``"invalid value for options.clients: '<name>'; valid values: claude,
    opencode, copilot"``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.clients", "claude,vscode", check=False
    )
    assert result.returncode == 65, (
        f"unknown client name must exit 65 (DataError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "invalid value for options.clients: 'vscode'" in result.stderr, (
        f"message must name the unknown value (parse_default_view template); "
        f"got: {result.stderr!r}"
    )
    assert "valid values: claude, opencode, copilot" in result.stderr, (
        f"message must list the valid client names; got: {result.stderr!r}"
    )


def test_set_clients_duplicate_exits_65(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set options.clients`` with a duplicate segment exits 65
    (DataError); the message names the duplicated client.

    ``options.clients`` is a set of *unique* values — repeating a client
    name in the comma-separated list is rejected rather than silently
    de-duplicated.

    Traces to the frozen validation: duplicate segment → DataError 65,
    message ``"options.clients: duplicate client '<name>'"``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.clients", "claude,opencode,claude",
        check=False,
    )
    assert result.returncode == 65, (
        f"duplicate client must exit 65 (DataError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "duplicate client 'claude'" in result.stderr, (
        f"message must name the duplicated client; got: {result.stderr!r}"
    )
    assert "each client may appear once" in result.stderr, (
        f"message must carry the remediation hint; got: {result.stderr!r}"
    )


# ---------------------------------------------------------------------------
# Load-time options.clients validation (hand-authored TOML)
#
# Set-time validation (config set) already rejects unknown/duplicate clients
# at exit 65. A hand-edited grimoire.toml bypasses that path entirely, so an
# unknown or duplicate client would previously load clean and only surface as
# a confusing failure at install time. validate_clients runs in the config
# parser (beside validate_tree_separators), so ANY config-loading command
# rejects it up front — a typed ConfigError (exit 78), never a panic.
# ---------------------------------------------------------------------------


def _assert_not_a_panic(result: subprocess.CompletedProcess[str]) -> None:
    """A clean typed rejection, never a Rust panic (exit 101 / SIGABRT)."""
    assert "panicked" not in result.stderr.lower(), (
        f"config load must reject cleanly, not panic; got: {result.stderr!r}"
    )
    assert result.returncode not in (101, 134, 139), (
        f"exit code must be a typed error, not a panic/abort/segfault; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_load_clients_unknown_name_project_exits_78(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A hand-authored project config with an unknown client exits 78.

    ``clients = ["vscode"]`` is structurally valid TOML, so the rejection
    fires only in grim's post-parse validator (``validate_clients``), which
    classifies as ConfigError (exit 78) — the same class and exit code an
    invalid authored ``tree_separators`` produces (test_config_tui_options
    ``test_invalid_tree_separator_is_rejected``).
    """
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["vscode"]\n'
    )
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"authored unknown client must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "vscode" in result.stderr, (
        f"error must name the offending client; got: {result.stderr!r}"
    )
    _assert_not_a_panic(result)


def test_load_clients_duplicate_project_exits_78(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A hand-authored project config with a duplicate client exits 78.

    ``options.clients`` is a set of unique values; a repeated entry in the
    authored array is rejected at load time, not silently de-duplicated.
    """
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["claude", "claude"]\n'
    )
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"authored duplicate client must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "claude" in result.stderr, (
        f"error must name the duplicated client; got: {result.stderr!r}"
    )
    _assert_not_a_panic(result)


def test_load_clients_blank_project_exits_78(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A hand-authored project config with a blank client entry exits 78.

    ``clients = ["claude", ""]`` is structurally valid TOML; the empty
    string is rejected by the same shared ``check_clients`` validator that
    guards unknown and duplicate entries.
    """
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["claude", ""]\n'
    )
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"authored blank client must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    _assert_not_a_panic(result)


def test_load_clients_control_char_project_exits_78_no_raw_escape(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A hand-authored client name with a control byte is rejected safely.

    The name is authored via the TOML ``\\u001b`` escape (a raw control byte
    is rejected by the TOML parser itself, so the escape is the real vector),
    which decodes to ESC + ``[2J`` — a terminal clear-screen sequence. It must
    exit 78 like any other invalid authored client, but the load-time error
    must NOT echo the raw ESC byte to stderr — otherwise merely running a
    config command inside an untrusted repo injects a control sequence into
    the user's terminal.
    """
    (project_dir / "grimoire.toml").write_text(
        '[options]\nclients = ["\\u001b[2Jvscode"]\n'
    )
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"authored control-char client must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "\x1b" not in result.stderr, (
        f"error must not echo the raw ESC byte to the terminal; "
        f"got: {result.stderr!r}"
    )
    _assert_not_a_panic(result)


def test_load_clients_unknown_name_global_exits_78(
    grim_binary: Path,
    grim_home: Path,
) -> None:
    """A hand-authored *global* config with an unknown client exits 78.

    The global config (``$GRIM_HOME/grimoire.toml``) routes through the same
    shared parser, so ``validate_clients`` fires for ``config --global list``
    exactly as it does for the project scope.
    """
    (grim_home / "grimoire.toml").write_text(
        '[options]\nclients = ["vscode"]\n'
    )
    runner = GrimRunner(grim_binary, grim_home)

    result = runner.run("config", "--global", "list", check=False)
    assert result.returncode == 78, (
        f"authored unknown client (global) must exit 78 (ConfigError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert "vscode" in result.stderr, (
        f"error must name the offending client; got: {result.stderr!r}"
    )
    _assert_not_a_panic(result)


# ---------------------------------------------------------------------------
# FIX A regression: zero-width separator rejected at CLI (no lockout)
# ---------------------------------------------------------------------------


def test_set_tree_separators_zero_width_char_exits_65(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config set options.tui.tree_separators`` with U+200B exits 65 (DataError).

    U+200B ZERO WIDTH SPACE passes the single-char and control/whitespace checks
    but has display width 0. Before FIX A the CLI accepted it, wrote the config,
    and every subsequent ``grim`` invocation failed with ConfigError 78 — with
    no CLI recovery path because ``config unset`` also reads the config first.

    The fix mirrors the load-time ``validate_tree_separators`` check, so the
    CLI parser and the loader accept exactly the same set.

    Traces to FIX A: parse_tree_separators mirrors validate_tree_separators.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    zwsp = "​"  # U+200B ZERO WIDTH SPACE — invisible, display width 0
    result = runner.run(
        "config", "set", "options.tui.tree_separators", zwsp,
        check=False,
    )
    assert result.returncode == 65, (
        f"zero-width separator must exit 65 (DataError); "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    # Config must not have been written with the bad separator.
    cfg_path = project_dir / "grimoire.toml"
    if cfg_path.exists():
        with cfg_path.open("rb") as f:
            data = tomllib.load(f)
        seps = data.get("options", {}).get("tui", {}).get("tree_separators", [])
        assert zwsp not in seps, (
            f"zero-width char must not be written to config; got seps={seps!r}"
        )


# ---------------------------------------------------------------------------
# D2: list on empty config
# ---------------------------------------------------------------------------


def test_list_on_empty_config_exits_0_with_empty_output(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config list`` on a config with no options set exits 0 with no rows.

    Traces to ADR / D2: list on empty config → exit 0, empty.
    """
    write_config(project_dir)  # minimal config, no [options] table
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.json("config", "list")["items"]

    assert result == [], f"list on empty config must be empty array; got {result!r}"
    assert len(result) == 0, (
        f"list on empty config must have zero entries; got: {result!r}"
    )


# ---------------------------------------------------------------------------
# I2/I3: --all + extended JSON metadata
# ---------------------------------------------------------------------------


def test_list_all_on_empty_config_lists_every_supported_key_unset(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config list --all`` on an empty config surfaces all 7 fixed option
    keys as unset rows, each carrying full metadata.

    Traces to I3: fixed keys unset -> row only under ``--all``, with
    ``value: null`` / ``set: false``.  Traces to I2: metadata fields
    (``title``, ``description``, ``type``) always present.
    """
    write_config(project_dir)  # minimal config, no [options] table
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("--format", "json", "config", "list", "--all", check=False)
    assert result.returncode == 0, (
        f"list --all on empty config must exit 0; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    items = json.loads(result.stdout)["items"]

    by_key = {i["key"]: i for i in items if isinstance(i, dict)}
    for key in FIXED_OPTION_KEYS:
        assert key in by_key, f"--all must list unset fixed key {key}; got keys: {sorted(by_key)}"
        item = by_key[key]
        assert item["value"] is None, f"{key} unset row must have value None; got {item!r}"
        assert item["set"] is False, f"{key} unset row must have set False; got {item!r}"
        assert item["title"], f"{key} must carry a non-empty title; got {item!r}"
        assert item["description"], f"{key} must carry a non-empty description; got {item!r}"
        assert item["type"] in _ALLOWED_TYPES, (
            f"{key} type must be one of {_ALLOWED_TYPES}; got {item['type']!r}"
        )


def test_list_all_plain_on_empty_config_exits_0_with_rows(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config list --all`` in plain mode exits 0 and lists every fixed
    option key name, even though none is set.

    Traces to I3: plain output stays a single table (static ``Key``/
    ``Value`` headers); ``--all`` only widens the row set.
    """
    write_config(project_dir)  # minimal config, no [options] table
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.plain("config", "list", "--all")
    assert result.returncode == 0, (
        f"list --all plain on empty config must exit 0; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    for key in FIXED_OPTION_KEYS:
        assert key in result.stdout, (
            f"plain --all output must contain {key}; got:\n{result.stdout}"
        )


def test_list_json_entries_carry_type_title_description_default(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A ``config list`` JSON entry for a set enum key carries its full
    extended metadata, and the same fields appear with or without ``--all``
    — the flag only widens the row set, never the row shape.

    Traces to I2: frozen JSON shape, pinned metadata table row for
    ``options.tui.default_view`` (enum, values ``["flat","tree"]``,
    default ``"tree"``).
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.tui.default_view", "flat")

    def _get_entry(*extra_args: str) -> dict:
        items = runner.json("config", "list", *extra_args)["items"]
        by_key = {i["key"]: i for i in items if isinstance(i, dict)}
        assert "options.tui.default_view" in by_key, (
            f"list must contain options.tui.default_view; keys: {sorted(by_key)}"
        )
        return by_key["options.tui.default_view"]

    for args, label in ((("--all",), "with --all"), ((), "without --all")):
        entry = _get_entry(*args)
        assert entry["type"] == "enum", f"{label}: type must be 'enum'; got {entry!r}"
        assert entry["values"] == ["flat", "tree"], (
            f"{label}: values must be ['flat','tree']; got {entry!r}"
        )
        assert entry["default"] == "tree", f"{label}: default must be 'tree'; got {entry!r}"
        assert entry["set"] is True, f"{label}: set must be True; got {entry!r}"
        assert entry["value"] == "flat", f"{label}: value must be 'flat'; got {entry!r}"
        assert entry["title"], f"{label}: title must be non-empty; got {entry!r}"
        assert entry["description"], f"{label}: description must be non-empty; got {entry!r}"


def test_list_json_entry_clients_is_string_set_with_canonical_values(
    grim_at: object,
    project_dir: Path,
) -> None:
    """The ``config list`` JSON entry for ``options.clients`` carries
    ``type: "string-set"`` and ``values`` in the canonical
    ``ClientTarget::ALL`` order (``["claude","opencode","copilot","codex"]``) —
    regardless of the order the user supplied to ``set``, and whether the
    key is unset or set. ``default`` stays ``null`` (no fixed default).

    Traces to the frozen ``StringSet`` variant: ``as_str()`` ->
    ``"string-set"``; ``values()`` returns ``Some(values)`` for
    ``StringSet``; ``default_str()`` maps ``default.map(|s| s.join(","))``
    (``None`` here).
    """
    write_config(project_dir)  # no [options] table — clients unset
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    unset_items = runner.json("config", "list", "--all")["items"]
    unset_entry = next(i for i in unset_items if i["key"] == "options.clients")
    assert unset_entry["type"] == "string-set", (
        f"options.clients type must be 'string-set'; got: {unset_entry!r}"
    )
    assert unset_entry["values"] == CLIENT_VALUE_NAMES, (
        f"options.clients values must be the canonical ClientTarget::ALL "
        f"order; got: {unset_entry!r}"
    )
    assert unset_entry["default"] is None, (
        f"options.clients has no fixed default; got: {unset_entry!r}"
    )
    assert unset_entry["set"] is False, f"unset row must have set False; got {unset_entry!r}"
    assert unset_entry["value"] is None, f"unset row must have value None; got {unset_entry!r}"

    # Supplied in non-canonical order — the "values" metadata still pins
    # the canonical order; only "value" echoes what the user typed.
    runner.run("config", "set", "options.clients", "copilot,claude")
    set_items = runner.json("config", "list")["items"]
    set_entry = next(i for i in set_items if i["key"] == "options.clients")
    assert set_entry["type"] == "string-set", (
        f"options.clients type must stay 'string-set' once set; got: {set_entry!r}"
    )
    assert set_entry["values"] == CLIENT_VALUE_NAMES, (
        f"options.clients values must stay the canonical order once set; "
        f"got: {set_entry!r}"
    )
    assert set_entry["value"] == "copilot,claude", (
        f"value must echo the input order verbatim; got: {set_entry!r}"
    )


def test_list_all_includes_unset_registry_locator_row(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``config list --all`` includes an unset ``registry.<alias>.index``
    row for a registry added without an index locator; without ``--all``
    that key is absent. ``registry.<alias>.default`` is always a row
    (today's behavior, no unset state).

    Traces to I3: registry rows — ``.oci``/``.index`` absent -> row only
    under ``--all`` (``value: null``, ``set: false``); ``.default`` ALWAYS
    a row, effective value, ``set: true``.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "registry", "add", "acme", "--oci", "ghcr.io/acme")

    all_items = runner.json("config", "list", "--all")["items"]
    all_by_key = {i["key"]: i for i in all_items if isinstance(i, dict)}

    assert "registry.acme.index" in all_by_key, (
        f"--all must list the unset registry.acme.index row; keys: {sorted(all_by_key)}"
    )
    index_entry = all_by_key["registry.acme.index"]
    assert index_entry["value"] is None, f"unset index row must have value None; got {index_entry!r}"
    assert index_entry["set"] is False, f"unset index row must have set False; got {index_entry!r}"

    plain_items = runner.json("config", "list")["items"]
    plain_by_key = {i["key"]: i for i in plain_items if isinstance(i, dict)}
    assert "registry.acme.index" not in plain_by_key, (
        f"without --all, unset registry.acme.index must be absent; got keys: {sorted(plain_by_key)}"
    )

    assert "registry.acme.default" in all_by_key, (
        "registry.acme.default must always be a row (with --all)"
    )
    assert all_by_key["registry.acme.default"]["value"] == "false", (
        f"registry.acme.default must be 'false'; got {all_by_key['registry.acme.default']!r}"
    )
    assert "registry.acme.default" in plain_by_key, (
        "registry.acme.default must always be a row (without --all)"
    )
    assert plain_by_key["registry.acme.default"]["value"] == "false", (
        f"registry.acme.default must be 'false'; got {plain_by_key['registry.acme.default']!r}"
    )


def test_list_json_empty_string_value_is_set_not_unset(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.default_registry ""`` is a *set* empty-string value:
    the JSON row disambiguates it from unset via ``set: true, value: ""``.

    Pins the documented plain-output caveat (empty Value cell is ambiguous;
    JSON is not) — the only key where empty string is a reachable set value.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.default_registry", "")

    items = runner.json("config", "list")["items"]
    by_key = {i["key"]: i for i in items if isinstance(i, dict)}
    assert "options.default_registry" in by_key, (
        f"empty-string default_registry must be listed; keys: {sorted(by_key)}"
    )
    entry = by_key["options.default_registry"]
    assert entry["set"] is True, f"empty-string value must report set True; got {entry!r}"
    assert entry["value"] == "", f"value must be the empty string, not null; got {entry!r}"


def test_list_all_shows_bool_set_to_false_as_unset(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set options.show_deprecated false`` collapses to unset: under
    ``--all`` the row reports ``set: false, value: null`` with the default
    carried separately in ``default`` — never echoed into ``value``.

    Pins the documented false-is-unset collapse for bool keys.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.show_deprecated", "false")

    items = runner.json("config", "list", "--all")["items"]
    by_key = {i["key"]: i for i in items if isinstance(i, dict)}
    entry = by_key["options.show_deprecated"]
    assert entry["set"] is False, (
        f"show_deprecated set to false must collapse to unset; got {entry!r}"
    )
    assert entry["value"] is None, f"collapsed row must have value None; got {entry!r}"
    assert entry["default"] == "false", (
        f"default must carry 'false' separately; got {entry!r}"
    )


# ---------------------------------------------------------------------------
# F4: Concurrency smoke
# ---------------------------------------------------------------------------


def test_concurrent_config_set_produces_valid_toml(
    grim_at: object,
    project_dir: Path,
    grim_binary: Path,
    grim_home: Path,
) -> None:
    """Two simultaneous ``config set`` calls leave ``grimoire.toml`` valid.

    Spawns two subprocesses in parallel; after both finish asserts the
    config file is parseable TOML and contains at least one of the two
    expected values (last-writer-wins is acceptable, but the file must
    never be corrupted).

    Traces to ADR / F4: concurrency smoke — file-lock prevents partial-write
    corruption; ``ConfigFileLock`` must ensure at-most-one writer at a time.
    """
    import os

    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    errors: list[str] = []

    # Exit codes: 0 = success, 75 = TempFail (lock contention — expected when
    # the other writer holds the advisory flock). Any other non-zero is a failure.
    LOCK_CONTENTION = 75
    successes: list[str] = []
    hard_errors: list[str] = []

    def run_set(key: str, value: str) -> None:
        cmd = [str(grim_binary), "config", "set", key, value]
        r = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=runner.env,
            cwd=str(project_dir),
        )
        if r.returncode == 0:
            successes.append(f"{key}={value}")
        elif r.returncode != LOCK_CONTENTION:
            hard_errors.append(
                f"{key}={value}: rc={r.returncode} stderr={r.stderr.strip()}"
            )

    t1 = threading.Thread(target=run_set, args=("options.default_registry", "ghcr.io/a"))
    t2 = threading.Thread(target=run_set, args=("options.clients", "claude"))
    t1.start()
    t2.start()
    t1.join()
    t2.join()

    assert not hard_errors, f"concurrent set commands failed unexpectedly: {hard_errors}"

    # The file must be valid TOML regardless of which writer won.
    cfg_text = (project_dir / "grimoire.toml").read_text()
    try:
        parsed = tomllib.loads(cfg_text)
    except tomllib.TOMLDecodeError as exc:
        raise AssertionError(
            f"grimoire.toml is not valid TOML after concurrent writes:\n{cfg_text}\nError: {exc}"
        ) from exc

    # At least one writer must have succeeded (lock contention = one winner).
    assert successes, "at least one concurrent set must have succeeded"
    options = parsed.get("options", {})
    has_registry = options.get("default_registry") == "ghcr.io/a"
    has_clients = options.get("clients") == ["claude"]
    assert has_registry or has_clients, (
        f"the winning writer's value must appear in the config; options={options!r}"
    )


# ---------------------------------------------------------------------------
# #:schema directive preservation
# ---------------------------------------------------------------------------


def test_schema_directive_survives_config_rewrites(
    grim_at: object,
    project_dir: Path,
) -> None:
    """A leading ``#:schema`` editor directive survives every rewrite
    through the shared write_config seam (set, unset)."""
    directive = "#:schema https://grimoire.rs/schemas/grimoire-config.schema.json"
    (project_dir / "grimoire.toml").write_text(
        f"{directive}\n\n[skills]\n\n[rules]\n"
    )
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", "options.clients", "claude")
    body = (project_dir / "grimoire.toml").read_text()
    assert body.startswith(directive), f"set must keep the directive first: {body}"

    runner.run("config", "unset", "options.clients")
    body = (project_dir / "grimoire.toml").read_text()
    assert body.startswith(directive), f"unset must keep the directive first: {body}"
    tomllib.loads(body)  # still valid TOML


# ---------------------------------------------------------------------------
# `config set --dry-run`
# ---------------------------------------------------------------------------


def test_set_dry_run_valid_value_exits_0_and_leaves_file_unchanged(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set --dry-run`` with a valid value exits 0, reports ``dry_run:
    true`` in the write confirmation, and does not touch ``grimoire.toml``
    on disk (byte-for-byte).

    A subsequent ``get`` must still report the key unset — nothing was
    written.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    before = (project_dir / "grimoire.toml").read_bytes()

    result = runner.json("config", "set", "options.clients", "claude", "--dry-run")

    assert result.get("action") == "set"
    assert result.get("key") == "options.clients"
    assert "claude" in str(result.get("value", "")), (
        f"dry-run value must still report the parsed stored value; got: {result!r}"
    )
    assert result.get("dry_run") is True, (
        f"dry-run write confirmation must carry dry_run:true; got: {result!r}"
    )

    after = (project_dir / "grimoire.toml").read_bytes()
    assert after == before, (
        f"grimoire.toml must be byte-for-byte unchanged after a dry run; "
        f"before={before!r} after={after!r}"
    )

    # Nothing was actually written — get still reports unset (exit 1).
    get_result = runner.run("config", "get", "options.clients", check=False)
    assert get_result.returncode == 1, (
        f"dry-run set must not persist the value; get must still exit 1; "
        f"got {get_result.returncode}"
    )


def test_set_dry_run_invalid_value_exits_65_matching_real_set_envelope(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set --dry-run`` of an invalid enum value exits 65 with the same
    structured error envelope a real (non-dry-run) ``set`` produces.

    Error parity is by construction: both paths run the exact same
    ``apply_set`` validator before any write is attempted.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    real = runner.run(
        "--format", "json", "config", "set", "options.tui.default_view", "bogus",
        check=False,
    )
    dry = runner.run(
        "--format", "json", "config", "set", "options.tui.default_view", "bogus",
        "--dry-run", check=False,
    )

    assert real.returncode == 65, f"real invalid set must exit 65; got {real.returncode}"
    assert dry.returncode == 65, f"dry-run invalid set must exit 65; got {dry.returncode}"

    real_doc = json.loads(real.stdout)
    dry_doc = json.loads(dry.stdout)
    assert real_doc == dry_doc, (
        f"dry-run error envelope must match the real set's envelope exactly; "
        f"real={real_doc!r} dry={dry_doc!r}"
    )


def test_set_dry_run_unknown_key_exits_64(
    grim_at: object,
    project_dir: Path,
) -> None:
    """``set --dry-run`` of an unknown key exits 64 before any resolution
    or validation — same as a real ``set``."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "optins.clients", "claude", "--dry-run", check=False
    )

    assert result.returncode == 64, (
        f"unknown key must exit 64 (UsageError) even under --dry-run; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


def test_set_dry_run_outside_project_exits_79(
    grim_at: object,
    tmp_path: Path,
) -> None:
    """``set --dry-run`` outside a project (no ``grimoire.toml`` found by
    walking up) exits 79 — scope resolution runs before the dry-run
    short-circuit."""
    outside = tmp_path / "empty"
    outside.mkdir()
    runner: GrimRunner = grim_at(outside)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.clients", "claude", "--dry-run", check=False
    )

    assert result.returncode == 79, (
        f"set --dry-run outside a project must exit 79; got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )


def test_set_dry_run_global_scope_with_no_existing_config_creates_no_file(
    grim_binary: Path,
    grim_home: Path,
) -> None:
    """``--global set --dry-run`` with no existing ``$GRIM_HOME/grimoire.toml``
    exits 0 and creates no file.

    Contrast with a real ``--global set``, which creates the file (global
    scope resolves to empty defaults when absent, and a real write always
    persists them) — the dry-run path must skip the write entirely.
    """
    runner = GrimRunner(grim_binary, grim_home)
    assert not (grim_home / "grimoire.toml").exists(), "test precondition: no global config yet"

    result = runner.run(
        "config", "--global", "set", "options.clients", "claude", "--dry-run"
    )

    assert result.returncode == 0, (
        f"dry-run --global set must succeed even with no existing global config; "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )
    assert not (grim_home / "grimoire.toml").exists(), (
        "--dry-run must never create the global config file"
    )


def test_set_dry_run_plain_table_shows_dry_run_true(
    grim_at: object,
    project_dir: Path,
) -> None:
    """Plain ``set --dry-run`` output includes a ``Dry Run`` column showing
    ``true``."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.plain("config", "set", "options.clients", "claude", "--dry-run")

    assert "Dry Run" in result.stdout, f"plain output must have a Dry Run column; got: {result.stdout!r}"
    assert "true" in result.stdout, f"dry-run row must show true; got: {result.stdout!r}"
