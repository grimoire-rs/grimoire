# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`[options.vendors.<name>]` config-surface acceptance tests.

The per-vendor table is a DYNAMIC config key family: one addressable key
per client name (``options.vendors.<name>.shared_skills``), parsed from the
remainder after a fixed prefix, exactly like ``registry.<alias>.<field>``.

Four distinct failure modes across three exit codes:

  - unknown vendor NAME (it is part of the key path)  → 64 (unknown key)
  - bad VALUE on a valid key                          → 65 (data error)
  - ``shared_skills = true`` on a client that does not
    read the shared ``.agents/skills`` pool, set via
    ``config set``                                    → 65 (data error)
  - anything wrong in a hand-written ``grimoire.toml``
    (unknown vendor, or the same non-pool opt-in)     → 78 (config error)

The last two are one checker with two exit mappings — the same split
``check_vendor_name`` already has (64 at the key boundary, 78 at load).

No consumer reads ``shared_skills`` yet, so these tests assert the config
plumbing and the capability gate, never install layout.
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


VENDOR_KEY = "options.vendors.cursor.shared_skills"


def _assert_error_envelope(result: object, code: str, exit_code: int) -> None:
    """Assert the documented ``--format json`` error document on stdout."""
    doc = json.loads(result.stdout)  # type: ignore[attr-defined]
    assert set(doc) == {"error"}, f"top-level error key marks the doc: {doc}"
    assert doc["error"]["code"] == code, f"error class must be {code!r}: {doc}"
    assert doc["error"]["exit"] == exit_code, f"exit must be {exit_code}: {doc}"
    assert doc["error"]["message"], f"message carries the rendered chain: {doc}"


def _write_config_with_vendor(project_dir: Path, name: str, shared_skills: bool) -> Path:
    """Write a ``grimoire.toml`` carrying a hand-authored vendor table."""
    cfg = write_config(project_dir)
    with cfg.open("a") as fh:
        fh.write(
            f"\n[options.vendors.{name}]\n"
            f"shared_skills = {'true' if shared_skills else 'false'}\n"
        )
    return cfg


# ---------------------------------------------------------------------------
# Round-trip — set / get / unset / list
# ---------------------------------------------------------------------------


def test_set_get_unset_round_trip(grim_at: object, project_dir: Path) -> None:
    """set → get → unset round-trips through the on-disk vendor table."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    # Unset by default: `get` exits 1 (git-compatible), no stdout value.
    before = runner.run("config", "get", VENDOR_KEY, check=False)
    assert before.returncode == 1, (
        f"unset vendor key must exit 1, got {before.returncode}; "
        f"stderr: {before.stderr.strip()}"
    )

    runner.run("config", "set", VENDOR_KEY, "true")

    body = (project_dir / "grimoire.toml").read_text()
    assert "[options.vendors.cursor]" in body, (
        f"set must persist the vendor table; config was:\n{body}"
    )
    assert "shared_skills = true" in body, (
        f"set must persist the field value; config was:\n{body}"
    )

    after = runner.run("config", "get", VENDOR_KEY)
    assert after.stdout.strip() == "true", (
        f"get must echo the stored value; got {after.stdout!r}"
    )

    runner.run("config", "unset", VENDOR_KEY)
    gone = runner.run("config", "get", VENDOR_KEY, check=False)
    assert gone.returncode == 1, (
        f"unset key must exit 1 again, got {gone.returncode}"
    )
    assert "[options.vendors.cursor]" not in (project_dir / "grimoire.toml").read_text(), (
        "unset must remove the vendor table from the written config"
    )


def test_set_false_collapses_to_unset(grim_at: object, project_dir: Path) -> None:
    """``false`` is the built-in default, so setting it reads back as unset.

    Mirrors ``options.show_deprecated`` / ``options.tui.group_by_type``: a
    value indistinguishable from the default collapses to unset across
    ``get`` / ``list``, and is not written to the config.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    runner.run("config", "set", VENDOR_KEY, "true")
    runner.run("config", "set", VENDOR_KEY, "false")

    body = (project_dir / "grimoire.toml").read_text()
    assert "[options.vendors.cursor]" not in body, (
        f"shared_skills = false must not be written (it is the default); config was:\n{body}"
    )
    result = runner.run("config", "get", VENDOR_KEY, check=False)
    assert result.returncode == 1, (
        f"a default-valued vendor key must read back as unset (exit 1), got {result.returncode}"
    )


def test_get_json_envelope(grim_at: object, project_dir: Path) -> None:
    """``get --format json`` carries the dotted key, value, and scope."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "set", VENDOR_KEY, "true")

    payload = runner.json("config", "get", VENDOR_KEY)
    assert payload["key"] == VENDOR_KEY, f"key must round-trip verbatim; got {payload!r}"
    assert payload["value"] == "true", f"value must be the stored value; got {payload!r}"
    assert payload["scope"] == "project", f"scope must be project; got {payload!r}"


def test_set_json_envelope(grim_at: object, project_dir: Path) -> None:
    """``set --format json`` reports the write action for the dynamic key."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    payload = runner.json("config", "set", VENDOR_KEY, "true")
    assert payload["key"] == VENDOR_KEY, f"key must round-trip verbatim; got {payload!r}"
    assert payload["value"] == "true", f"value must be reported; got {payload!r}"
    assert payload["dry_run"] is False, f"a real set is not a dry run; got {payload!r}"


def test_unset_json_envelope(grim_at: object, project_dir: Path) -> None:
    """``unset --format json`` reports the removal of the dynamic key."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "set", VENDOR_KEY, "true")

    payload = runner.json("config", "unset", VENDOR_KEY)
    assert payload["key"] == VENDOR_KEY, f"key must round-trip verbatim; got {payload!r}"
    assert payload["value"] is None, f"unset carries no value; got {payload!r}"
    assert payload["dry_run"] is False, f"unset has no dry-run surface; got {payload!r}"


def test_global_scope_round_trip(grim: object, grim_home: Path) -> None:
    """The table works in the global ``grimoire.toml``, not just the project one.

    Each command resolves exactly one scope — there is no merge — so the
    global surface needs its own coverage.
    """
    (grim_home / "grimoire.toml").write_text("[skills]\n\n[rules]\n")
    runner: GrimRunner = grim  # type: ignore[assignment]

    runner.run("config", "--global", "set", VENDOR_KEY, "true")

    body = (grim_home / "grimoire.toml").read_text()
    assert "[options.vendors.cursor]" in body, (
        f"global set must persist the vendor table; config was:\n{body}"
    )

    payload = runner.json("config", "--global", "get", VENDOR_KEY)
    assert payload["value"] == "true", f"global get must echo the value; got {payload!r}"
    assert payload["scope"] == "global", f"scope must be global; got {payload!r}"

    result = runner.run(
        "config", "--global", "set", "options.vendors.bogus.shared_skills", "true", check=False
    )
    assert result.returncode == 64, (
        f"the same key validation applies in global scope, got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )

    runner.run("config", "--global", "unset", VENDOR_KEY)
    assert "[options.vendors.cursor]" not in (grim_home / "grimoire.toml").read_text(), (
        "global unset must remove the vendor table"
    )


def test_list_surfaces_set_vendor_key(grim_at: object, project_dir: Path) -> None:
    """A set vendor key appears in ``config list`` (plain and JSON)."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.run("config", "set", VENDOR_KEY, "true")

    plain = runner.run("config", "list")
    assert VENDOR_KEY in plain.stdout, (
        f"plain list must show the vendor key; got:\n{plain.stdout}"
    )

    payload = runner.json("config", "list")
    row = next((i for i in payload["items"] if i["key"] == VENDOR_KEY), None)
    assert row is not None, f"json list must carry the vendor row; got {payload!r}"
    assert row["value"] == "true", f"row must carry the stored value; got {row!r}"
    assert row["type"] == "boolean", f"vendor field is a boolean key; got {row!r}"
    assert row["description"], f"row must carry a description; got {row!r}"


def test_list_all_on_vendorless_config_adds_no_vendor_rows(
    grim_at: object, project_dir: Path
) -> None:
    """``list --all`` must not enumerate the whole client set.

    The vendor key is dynamic: a row exists only for a client the config
    actually names, exactly as ``registry.<alias>.*`` rows exist only for
    declared registry entries. Widening ``--all`` to every known client
    would change the frozen row set of an existing command.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    payload = runner.json("config", "list", "--all")
    vendor_rows = [i["key"] for i in payload["items"] if i["key"].startswith("options.vendors.")]
    assert vendor_rows == [], (
        f"--all must add no vendor rows to a config that names no vendor; got {vendor_rows!r}"
    )


def test_list_all_surfaces_declared_but_default_vendor_entry(
    grim_at: object, project_dir: Path
) -> None:
    """``--all`` surfaces a declared-but-default vendor entry as an unset row."""
    _write_config_with_vendor(project_dir, "cursor", shared_skills=False)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    without_all = runner.json("config", "list")
    assert not [i for i in without_all["items"] if i["key"] == VENDOR_KEY], (
        f"a default-valued vendor row is omitted without --all; got {without_all!r}"
    )

    with_all = runner.json("config", "list", "--all")
    row = next((i for i in with_all["items"] if i["key"] == VENDOR_KEY), None)
    assert row is not None, f"--all must surface the declared entry; got {with_all!r}"
    assert row["value"] is None, f"a default-valued row is unset; got {row!r}"


# ---------------------------------------------------------------------------
# Exit codes — 64 (key), 65 (value), 78 (authored config)
# ---------------------------------------------------------------------------


def test_unknown_vendor_name_in_key_exits_64(grim_at: object, project_dir: Path) -> None:
    """The vendor name is part of the KEY, so an unknown one is exit 64."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.vendors.bogus.shared_skills", "true", check=False
    )
    assert result.returncode == 64, (
        f"unknown vendor name must exit 64 (UsageError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "bogus" in result.stderr, (
        f"error must name the offending vendor; got: {result.stderr!r}"
    )
    assert "cursor" in result.stderr, (
        f"error must list the valid client names; got: {result.stderr!r}"
    )

    # Same rejection under --format json carries the documented envelope.
    as_json = runner.run(
        "--format",
        "json",
        "config",
        "set",
        "options.vendors.bogus.shared_skills",
        "true",
        check=False,
    )
    assert as_json.returncode == 64
    _assert_error_envelope(as_json, "usage", 64)


def test_unknown_vendor_name_on_get_exits_64(grim_at: object, project_dir: Path) -> None:
    """``get`` rejects an unknown vendor name at the same boundary as ``set``."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "get", "options.vendors.bogus.shared_skills", check=False)
    assert result.returncode == 64, (
        f"unknown vendor name must exit 64 on get, got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    # Discriminating signal: 64 is also the generic unknown-key and clap usage
    # code, so pin the phrase only the vendor branch produces.
    assert "no client named" in result.stderr, (
        f"the rejection must come from the vendor-name check; got: {result.stderr!r}"
    )


def test_control_char_in_vendor_key_is_escaped_on_stderr(
    grim_at: object, project_dir: Path
) -> None:
    """A key segment carrying an ESC byte must never be echoed raw.

    ``grim config get options.vendors.<ESC>[2Jcursor.shared_skills`` quotes
    the segment back in its error; rendering it unescaped would inject a
    terminal control sequence into the caller's terminal.
    """
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    for key in (
        "options.vendors.\x1b[2Jcursor.shared_skills",
        "options.vendors.cursor.\x1b[2Jbogus",
        "options.vendors.\x1b[2Jcursor",
    ):
        result = runner.run("config", "get", key, check=False)
        assert result.returncode == 64, (
            f"a control character in the key must exit 64, got {result.returncode}"
        )
        assert "\x1b" not in result.stderr, (
            f"error for {key!r} must not embed the raw ESC byte; got: {result.stderr!r}"
        )


def test_unknown_vendor_field_exits_64(grim_at: object, project_dir: Path) -> None:
    """An unknown field under a valid vendor is an unknown key (64)."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "set", "options.vendors.cursor.bogus", "true", check=False)
    assert result.returncode == 64, (
        f"unknown vendor field must exit 64 (UsageError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "shared_skills" in result.stderr, (
        f"error must name the valid field; got: {result.stderr!r}"
    )


def test_invalid_bool_value_exits_65(grim_at: object, project_dir: Path) -> None:
    """A bad VALUE on a valid vendor key is a data error (65), not a key error."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "set", VENDOR_KEY, "yes", check=False)
    assert result.returncode == 65, (
        f"invalid boolean must exit 65 (DataError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "true or false" in result.stderr, (
        f"error must state the accepted values; got: {result.stderr!r}"
    )

    as_json = runner.run("--format", "json", "config", "set", VENDOR_KEY, "yes", check=False)
    assert as_json.returncode == 65
    _assert_error_envelope(as_json, "data", 65)


def test_shared_skills_on_a_non_pool_client_exits_65(grim_at: object, project_dir: Path) -> None:
    """Enabling ``shared_skills`` for a client that does not read the shared
    ``.agents/skills`` pool is a bad VALUE on a valid key — exit 65.

    Claude is the case that matters: it is both a verified non-reader AND the
    only vendor declaring ``skill_fields``, so pooling it would rewrite a
    directory three siblings already record."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.vendors.claude.shared_skills", "true", check=False
    )
    assert result.returncode == 65, (
        f"a non-pool client must exit 65 (DataError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    # Discriminating: 65 is also the bad-boolean code, so pin the phrase only
    # this branch produces.
    assert "does not read the shared .agents/skills pool" in result.stderr, (
        f"the rejection must come from the pool-capability check; got: {result.stderr!r}"
    )
    assert "cursor" in result.stderr, (
        f"the error must list the clients that DO read it; got: {result.stderr!r}"
    )

    as_json = runner.run(
        "--format",
        "json",
        "config",
        "set",
        "options.vendors.claude.shared_skills",
        "true",
        check=False,
    )
    assert as_json.returncode == 65
    _assert_error_envelope(as_json, "data", 65)

    # Nothing was written: the refusal is total, not partial.
    body = (project_dir / "grimoire.toml").read_text()
    assert "vendors" not in body, f"a refused set must persist nothing; config was:\n{body}"


def test_shared_skills_false_on_a_non_pool_client_is_accepted(
    grim_at: object, project_dir: Path
) -> None:
    """Only *enabling* is refused. ``false`` is every client's resting state,
    so setting it explicitly must stay a no-op success — otherwise a user
    could not clear a key they were allowed to reach."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.vendors.claude.shared_skills", "false", check=False
    )
    assert result.returncode == 0, (
        f"setting the default on any known client must succeed, got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )


def test_authored_shared_skills_on_a_non_pool_client_exits_78(
    grim_at: object, project_dir: Path
) -> None:
    """The load-time half of the same checker: a hand-written opt-in on a
    non-pool client fails at LOAD with 78 (an invalid ``[options.vendors]``
    table), the same class as an authored unknown client name.

    One checker, two exit codes — exactly the split ``check_vendor_name``
    already has (64 at the key boundary, 78 at load)."""
    _write_config_with_vendor(project_dir, "claude", shared_skills=True)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"an authored non-pool opt-in must exit 78 (ConfigError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "does not read the shared .agents/skills pool" in result.stderr, (
        f"the rejection must come from the pool-capability check; got: {result.stderr!r}"
    )


def test_authored_shared_skills_false_on_a_non_pool_client_loads(
    grim_at: object, project_dir: Path
) -> None:
    """An authored ``false`` on a non-pool client is pointless but valid, and
    must not turn a previously-loading config into an error."""
    _write_config_with_vendor(project_dir, "claude", shared_skills=False)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 0, (
        f"an authored default must still load, got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )


def test_authored_unknown_vendor_exits_78(grim_at: object, project_dir: Path) -> None:
    """A hand-written unknown vendor fails at LOAD time with 78.

    ``config set`` is bypassed here, so this is the load-time half of the
    shared validator — same accepted set, different exit class.
    """
    _write_config_with_vendor(project_dir, "vscode", shared_skills=True)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"authored unknown vendor must exit 78 (ConfigError), got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "vscode" in result.stderr, (
        f"error must name the offending vendor; got: {result.stderr!r}"
    )

    as_json = runner.run("--format", "json", "config", "list", check=False)
    assert as_json.returncode == 78
    _assert_error_envelope(as_json, "config", 78)


def test_authored_unknown_vendor_field_exits_78(grim_at: object, project_dir: Path) -> None:
    """``deny_unknown_fields`` covers the vendor table too (78)."""
    cfg = write_config(project_dir)
    with cfg.open("a") as fh:
        fh.write("\n[options.vendors.cursor]\nbogus_field = true\n")

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    result = runner.run("config", "list", check=False)
    assert result.returncode == 78, (
        f"unknown field under a vendor table must exit 78 (ConfigError), "
        f"got {result.returncode}; stderr: {result.stderr.strip()}"
    )


# ---------------------------------------------------------------------------
# --dry-run and the additive guarantee
# ---------------------------------------------------------------------------


def test_dry_run_validates_without_writing(grim_at: object, project_dir: Path) -> None:
    """``set --dry-run`` reports the write and leaves the config untouched."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    before = (project_dir / "grimoire.toml").read_text()

    payload = runner.json("config", "set", VENDOR_KEY, "true", "--dry-run")
    assert payload["dry_run"] is True, f"dry run must be reported; got {payload!r}"
    assert payload["key"] == VENDOR_KEY, f"key must round-trip; got {payload!r}"

    assert (project_dir / "grimoire.toml").read_text() == before, (
        "--dry-run must not write the config file"
    )


def test_dry_run_rejects_unknown_vendor_with_same_code(
    grim_at: object, project_dir: Path
) -> None:
    """Error parity: ``--dry-run`` uses the same validators, same 64."""
    write_config(project_dir)
    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]

    result = runner.run(
        "config", "set", "options.vendors.bogus.shared_skills", "true", "--dry-run", check=False
    )
    assert result.returncode == 64, (
        f"dry-run must reject an unknown vendor with 64, got {result.returncode}; "
        f"stderr: {result.stderr.strip()}"
    )
    assert "no client named" in result.stderr, (
        f"the rejection must come from the vendor-name check; got: {result.stderr!r}"
    )


def test_config_without_vendors_parses_and_stays_vendorless(
    grim_at: object, project_dir: Path
) -> None:
    """The additive guarantee: an old config parses unchanged and gains no table."""
    cfg = write_config(project_dir)
    with cfg.open("a") as fh:
        fh.write('\n[options]\nclients = ["claude"]\n')

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    payload = runner.json("config", "list")
    assert [i for i in payload["items"] if i["key"] == "options.clients"], (
        f"pre-existing keys must still load; got {payload!r}"
    )

    # A write path (set of an unrelated key) must not invent a vendor table.
    runner.run("config", "set", "options.show_deprecated", "true")
    assert "[options.vendors" not in cfg.read_text(), (
        "a config that declared no vendor table must not grow one"
    )


def test_vendor_table_preserved_through_add_remove_round_trip(
    grim_at: object,
    project_dir: Path,
    registry: str,
    unique_repo: str,
) -> None:
    """``grim add`` / ``grim remove`` re-serialize the config — the table survives.

    Same risk as ``[options.tui]``: a ``write_config`` that does not know
    about the vendor table would silently erase the user's settings on the
    next unrelated config write.
    """
    sk = make_artifact(
        f"{unique_repo}/vendor-options-probe",
        "skill",
        {
            "vendor-options-probe/SKILL.md": (
                "---\nname: vendor-options-probe\ndescription: probe\n---\n# probe\n"
            )
        },
        tag="v1",
    )
    _write_config_with_vendor(project_dir, "cursor", shared_skills=True)

    runner: GrimRunner = grim_at(project_dir)  # type: ignore[call-arg]
    runner.json("add", sk.fq)

    after_add = (project_dir / "grimoire.toml").read_text()
    assert "[options.vendors.cursor]" in after_add, (
        f"vendor table must survive grim add re-serialization; config was:\n{after_add}"
    )
    assert "shared_skills = true" in after_add, (
        f"vendor field must survive grim add re-serialization; config was:\n{after_add}"
    )

    runner.json("remove", "skill", "vendor-options-probe")

    after_remove = (project_dir / "grimoire.toml").read_text()
    assert "[options.vendors.cursor]" in after_remove, (
        f"vendor table must survive grim remove re-serialization; config was:\n{after_remove}"
    )
