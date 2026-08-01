# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Regressions for the managed-config splices and `grim status` verdicts.

Five independent defects, all in the same blast radius — grim editing a
user-owned config file, and grim reporting what it did:

* the Codex TOML splice deleted a hand-authored bare ``[mcp_servers]``
  header (and the comment attached to it) on upsert;
* the OpenCode ``instructions`` sync reserialized the *whole*
  ``opencode.json``/``.jsonc`` through serde — dropping comments,
  alphabetizing every key, reflowing the file — while MCP entries into the
  same file went through the byte-preserving splice engine;
* the global OpenCode MCP anchor was derived from ``$OPENCODE_CONFIG_DIR``
  while the file was written wherever ``$OPENCODE_CONFIG`` / XDG resolved,
  so either variable made install skip and uninstall orphan a live entry;
* ``clients_missing`` permanently listed a configured client whose vendor
  *declines* the artifact's kind — drift no grim command can clear;
* ``status`` classified from live client *detection* rather than from the
  recorded outputs on disk, reporting a healthy install as ``missing`` and
  hiding a hand-edited one behind ``missing`` too.
"""
from __future__ import annotations

import json
import tomllib  # stdlib (Python 3.11+)
from pathlib import Path

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner

MCP_DESCRIPTOR = """\
description = "Grimoire catalog search and install status over MCP."
summary = "grim as an MCP server"

[server]
transport = "stdio"
command = "grim"
args = ["mcp"]
"""

RULE_BODY = """\
---
description: A rule
---

# Style

Body.
"""


def _release_mcp(runner: GrimRunner, src_dir: Path, registry: str, unique_repo: str) -> str:
    """Publish the shared `grim-mcp` descriptor and return its reference."""
    descriptor = src_dir / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(MCP_DESCRIPTOR)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


# ── (a) Codex TOML splice: a hand-authored container header survives ───────


def test_codex_config_toml_hand_authored_container_header_survives_install_and_uninstall(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A user's bare ``[mcp_servers]`` header, and the comment attached to
    it, must survive an MCP install/uninstall round trip byte-for-byte
    outside the managed member.

    `toml_edit` drops a table header the moment it is marked implicit, and
    grim marked the *parsed* container implicit unconditionally — so the
    header and its comment vanished on the first install, and `remove` could
    not put back a header it never saw.
    """
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir / "src", registry, unique_repo)
    (project_dir / ".codex").mkdir()  # detect Codex alongside the fixture's Claude marker
    user_toml = (
        "# --- My MCP servers ---\n"
        "[mcp_servers]\n"
        "\n"
        '[mcp_servers.mine]\n'
        'command = "x"\n'
    )
    cfg = project_dir / ".codex" / "config.toml"
    cfg.write_text(user_toml)
    write_config(project_dir)
    runner.json("add", "--no-install", ref)

    rows = runner.json("install")["items"]
    assert rows[0]["status"] == "installed", rows

    text = cfg.read_text()
    assert "# --- My MCP servers ---" in text, f"the comment on the user's header must survive: {text}"
    assert "\n[mcp_servers]\n" in text, f"the user's bare container header must survive: {text}"
    doc = tomllib.loads(text)
    assert doc["mcp_servers"]["mine"]["command"] == "x", "foreign entry untouched"
    assert doc["mcp_servers"]["grim-mcp"]["command"] == "grim"

    out = runner.json("uninstall", "mcp", "grim-mcp")
    assert out["status"] in ("uninstalled", "removed"), out
    assert cfg.read_text() == user_toml, "uninstall must restore the original bytes exactly"


def test_codex_config_toml_repeat_install_leaves_status_not_modified(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Renderer self-heal (Principle 9) for the TOML splice: two installs in
    a row are byte-stable and leave `status` reporting `installed`, never
    `modified`."""
    runner = grim_at(project_dir)
    ref = _release_mcp(runner, project_dir / "src", registry, unique_repo)
    (project_dir / ".codex").mkdir()  # detect Codex alongside the fixture's Claude marker
    cfg = project_dir / ".codex" / "config.toml"
    cfg.write_text("# keep me\n[mcp_servers]\n\n[mcp_servers.mine]\ncommand = \"x\"\n")
    write_config(project_dir)
    runner.json("add", ref)

    first = cfg.read_text()
    second_rows = runner.json("install")["items"]
    assert second_rows[0]["status"] == "unchanged", second_rows
    assert cfg.read_text() == first, "a repeat install must be byte-stable"

    row = next(r for r in runner.json("status")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row


# ── (b) OpenCode instructions splice: comments + key order survive ─────────


def _opencode_project(project_dir: Path, registry: str, unique_repo: str, runner: GrimRunner) -> str:
    """Publish a rule and point a project at it with OpenCode detected."""
    rule = make_artifact(f"{unique_repo}/rules/style", "rule", {"style.md": RULE_BODY}, tag="1.0.0")
    (project_dir / ".opencode").mkdir(exist_ok=True)
    write_config(project_dir, rules={"style": rule.fq})
    runner.json("lock")
    return rule.fq


def test_opencode_jsonc_comments_and_key_order_survive_rule_install_and_uninstall(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Registering (and unregistering) the managed ``instructions`` glob must
    leave every byte outside that one array element untouched.

    The sync used to `serde_json::to_vec_pretty` the whole document, so a
    single rule install dropped the user's JSONC comments, alphabetized all
    their keys (`serde_json::Map` is a `BTreeMap` here), and reflowed the
    file — a destructive rewrite of a config grim only meant to add one glob
    to, and one that happened *while* MCP entries into the same file were
    already going through the byte-preserving splice engine.
    """
    runner = grim_at(project_dir)
    original = (
        "{\n"
        "  // which model to use\n"
        '  "model":   "anthropic/claude",\n'
        '  "zulu": true,\n'
        '  "instructions": [\n'
        '    "CONTRIBUTING.md"\n'
        "  ],\n"
        '  "alpha": 1\n'
        "}\n"
    )
    cfg = project_dir / "opencode.jsonc"
    cfg.write_text(original)
    _opencode_project(project_dir, registry, unique_repo, runner)

    rows = runner.json("install", "--client", "opencode")["items"]
    assert rows[0]["status"] == "installed", rows

    added = cfg.read_text()
    assert "// which model to use" in added, f"JSONC comment must survive: {added}"
    assert added.index('"zulu"') < added.index('"alpha"'), f"authored key order must survive: {added}"
    assert '"model":   "anthropic/claude"' in added, f"formatting must survive: {added}"
    assert '"CONTRIBUTING.md"' in added, "the user's own instructions entry must survive"
    assert ".opencode/rules/*.md" in added, "the managed glob must be registered"

    out = runner.json("uninstall", "rule", "style")
    assert out["status"] in ("uninstalled", "removed"), out
    assert cfg.read_text() == original, "uninstall must restore the original bytes exactly"


def test_opencode_repeat_rule_install_is_byte_stable_and_not_modified(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Renderer self-heal for the array splice: a second install writes
    nothing and leaves `status` at `installed`."""
    runner = grim_at(project_dir)
    cfg = project_dir / "opencode.json"
    cfg.write_text('{"model": "anthropic/claude"}\n')
    _opencode_project(project_dir, registry, unique_repo, runner)

    runner.json("install", "--client", "opencode")
    first = cfg.read_text()
    assert json.loads(first)["instructions"] == [".opencode/rules/*.md"]

    runner.json("install", "--client", "opencode")
    assert cfg.read_text() == first, "a repeat install must not rewrite the vendor config"

    row = next(r for r in runner.json("status")["items"] if r["name"] == "style")
    assert row["state"] == "installed", row


# ── (c) Global OpenCode MCP: anchor follows the write path ─────────────────


def test_global_opencode_mcp_installs_and_uninstalls_under_opencode_config_dir(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """With ``$OPENCODE_CONFIG_DIR`` set, the global MCP entry must still
    install into the config file the write path resolves — and uninstall
    must remove that entry, not orphan it.

    The anchor was derived from ``$OPENCODE_CONFIG_DIR`` (via the skills
    root) while the file was written to ``$XDG_CONFIG_HOME/opencode/
    opencode.json``, so `from_target` returned `UnknownAnchor`: install
    warned and recorded nothing, and an uninstall after the fact resolved
    the record to the wrong path, tolerated the `NotFound`, and dropped the
    record while the real entry stayed live and unremovable.
    """
    runner = GrimRunner(grim_binary, grim_home)
    ref = _release_mcp(runner, tmp_path / "src", registry, unique_repo)

    scan_dir = grim_home.parent / "opencode-scan"
    scan_dir.mkdir(parents=True, exist_ok=True)
    runner.env["OPENCODE_CONFIG_DIR"] = str(scan_dir)

    # The file the write path targets (XDG default; GrimRunner pins
    # XDG_CONFIG_HOME at <home>/.config).
    cfg = Path(runner.env["XDG_CONFIG_HOME"]) / "opencode" / "opencode.json"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    cfg.write_text('{"model": "anthropic/claude"}\n')

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert rows[0]["status"] == "installed", rows
    assert rows[0]["target"] is not None, "the recorded target must resolve, not be skipped as unanchorable"

    doc = json.loads(cfg.read_text())
    # OpenCode's stdio shape is a `command` array, not a bare string.
    assert doc["mcp"]["grim-mcp"]["command"] == ["grim", "mcp"], doc
    assert doc["model"] == "anthropic/claude", "foreign keys preserved"

    row = next(r for r in runner.json("status", "--global")["items"] if r["name"] == "grim-mcp")
    assert row["state"] == "installed", row

    out = runner.json("uninstall", "--global", "mcp", "grim-mcp")
    assert out["status"] in ("uninstalled", "removed"), out
    doc = json.loads(cfg.read_text())
    assert "grim-mcp" not in doc.get("mcp", {}), f"uninstall must remove the live entry, not orphan it: {doc}"
    assert doc["model"] == "anthropic/claude", "the config file itself must survive uninstall"


def test_global_opencode_mcp_follows_opencode_config_file_override(
    grim_binary, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """The other direction: ``$OPENCODE_CONFIG`` moves the config *file* to
    an arbitrary path while the skills root stays at the XDG default. The
    entry must land there and uninstall must remove it."""
    runner = GrimRunner(grim_binary, grim_home)
    ref = _release_mcp(runner, tmp_path / "src", registry, unique_repo)

    cfg = grim_home.parent / "custom" / "oc.json"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    cfg.write_text('{"model": "anthropic/claude"}\n')
    runner.env["OPENCODE_CONFIG"] = str(cfg)

    (grim_home / "grimoire.toml").write_text(f'[mcp]\ngrim-mcp = "{ref}"\n')
    runner.json("lock", "--global")
    rows = runner.json("install", "--global", "--client", "opencode")["items"]
    assert rows[0]["status"] == "installed", rows
    assert rows[0]["target"] is not None, rows

    doc = json.loads(cfg.read_text())
    # OpenCode's stdio shape is a `command` array, not a bare string.
    assert doc["mcp"]["grim-mcp"]["command"] == ["grim", "mcp"], doc

    runner.json("uninstall", "--global", "mcp", "grim-mcp")
    doc = json.loads(cfg.read_text())
    assert "grim-mcp" not in doc.get("mcp", {}), f"uninstall must remove the live entry: {doc}"


# ── (d) status: a client that declines the kind is not `clients_missing` ───


def test_clients_missing_skips_a_client_that_declines_the_kind(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``[options].clients = ["claude", "codex"]`` plus a declared **rule**
    is an ordinary pairing that installs cleanly — Codex declines rules, so
    the installer drops it before any write and records no output for it.

    `clients_missing` used to list `codex` on that row forever: neither
    `install`, `install --force`, nor `update` can clear it, because Codex
    structurally cannot host a rule. A CI gate asserting
    `clients_missing == []` failed permanently on a correct install.
    """
    runner = grim_at(project_dir)
    rule = make_artifact(f"{unique_repo}/rules/style", "rule", {"style.md": RULE_BODY}, tag="1.0.0")
    skill = make_artifact(
        f"{unique_repo}/skills/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\ndescription: A skill\n---\n\nBody.\n"},
        tag="1.0.0",
    )
    cfg = project_dir / "grimoire.toml"
    cfg.write_text(
        "[options]\n"
        'clients = ["claude", "codex"]\n'
        "\n"
        "[rules]\n"
        f'style = "{rule.fq}"\n'
        "\n"
        "[skills]\n"
        f'helper = "{skill.fq}"\n'
    )
    runner.json("lock")
    runner.json("install")

    items = {r["name"]: r for r in runner.json("status")["items"]}
    rule_row = items["style"]
    assert rule_row["state"] == "installed", rule_row
    assert rule_row["clients_missing"] == [], (
        "codex declines rules — it can never be installed there, so it is not actionable drift"
    )
    # The same client on a kind it DOES host is still reported when absent.
    assert items["helper"]["clients_missing"] == [], items["helper"]
    assert items["helper"]["clients_extra"] == [], items["helper"]

    # And it stays clear across a re-install (the old report never cleared).
    runner.json("install", "--force")
    rule_row = next(r for r in runner.json("status")["items"] if r["name"] == "style")
    assert rule_row["clients_missing"] == [], rule_row


def test_clients_missing_still_reports_a_genuinely_uninstalled_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The kind filter must not swallow real drift: a configured client that
    *can* host the kind but has no recorded output is still `missing`."""
    runner = grim_at(project_dir)
    skill = make_artifact(
        f"{unique_repo}/skills/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\ndescription: A skill\n---\n\nBody.\n"},
        tag="1.0.0",
    )
    cfg = project_dir / "grimoire.toml"
    cfg.write_text(f"[options]\nclients = [\"claude\"]\n\n[skills]\nhelper = \"{skill.fq}\"\n")
    runner.json("lock")
    runner.json("install")

    # Widen the configured set to a client that hosts skills but was never
    # installed to.
    cfg.write_text(
        "[options]\n"
        'clients = ["claude", "cursor"]\n'
        "\n"
        "[skills]\n"
        f'helper = "{skill.fq}"\n'
    )
    row = next(r for r in runner.json("status")["items"] if r["name"] == "helper")
    assert row["clients_missing"] == ["cursor"], row


# ── (e) status classifies from the recorded footprint, not detection ───────


def test_status_reports_installed_and_modified_for_a_client_that_does_not_detect(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Copilot writes project skills to ``.github/skills`` but detects on
    ``.github/copilot-instructions.md`` / ``.github/instructions/``. A
    copilot-only install therefore leaves nothing for detection to find.

    `status` used to filter the recorded outputs through live detection
    before classifying, so this healthy, byte-intact install reported
    ``missing`` with an empty ``outputs`` list — and a *hand-edited* one
    reported ``missing`` too, telling the user there was nothing to lose
    immediately before `grim install` refused the same file as `modified`
    and pointed them at `--force`.
    """
    runner = grim_at(project_dir)
    skill = make_artifact(
        f"{unique_repo}/skills/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\ndescription: A skill\n---\n\nBody.\n"},
        tag="1.0.0",
    )
    # The `project_dir` fixture already carries a `.claude/` marker, so
    # *something* detects and the permissive all-clients fallback never fires.
    write_config(project_dir, skills={"helper": skill.fq})
    runner.json("lock")
    rows = runner.json("install", "--client", "copilot")["items"]
    assert rows[0]["status"] == "installed", rows

    installed = project_dir / ".github" / "skills" / "helper" / "SKILL.md"
    assert installed.is_file(), "copilot project skills live under .github/skills"

    row = next(r for r in runner.json("status")["items"] if r["name"] == "helper")
    assert row["state"] == "installed", row
    assert [o["client"] for o in row["outputs"]] == ["copilot"], (
        f"the row must name the file it installed, not report an empty outputs list: {row}"
    )

    # A hand edit must surface as `modified` — the same verdict `install`
    # reaches on the same bytes.
    installed.write_text("hand edited\n")
    row = next(r for r in runner.json("status")["items"] if r["name"] == "helper")
    assert row["state"] == "modified", row
    refused = runner.run("install", "--client", "copilot", check=False)
    assert refused.returncode == 65, refused.stderr
    assert installed.read_text() == "hand edited\n", "a refused install must not overwrite the edit"


def test_status_reports_missing_once_the_installed_files_are_gone(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The complement of the above: with nothing left on disk the row is
    `missing`, whatever detection says. Classifying from the footprint must
    not turn `missing` into a state that can never be reached."""
    runner = grim_at(project_dir)
    skill = make_artifact(
        f"{unique_repo}/skills/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\ndescription: A skill\n---\n\nBody.\n"},
        tag="1.0.0",
    )
    write_config(project_dir, skills={"helper": skill.fq})
    runner.json("lock")
    runner.json("install", "--client", "copilot")

    import shutil

    shutil.rmtree(project_dir / ".github" / "skills" / "helper")
    row = next(r for r in runner.json("status")["items"] if r["name"] == "helper")
    assert row["state"] == "missing", row


# ── (f) status must not silently swallow an install-state load failure ─────


def test_status_surfaces_an_unreadable_install_state_instead_of_reporting_missing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A corrupt ``.grimoire/state.json`` used to be discarded with
    ``unwrap_or_else(|_| empty)``: `status` reported a fully-installed
    project as entirely `missing`, exit 0, with nothing on stderr — while
    every mutating command hard-failed on the identical bytes. Two commands,
    opposite verdicts, and the silent one pushed the user away from the one
    that names the real problem.

    It now fails the way the adjacent corrupt-lock case already did, and the
    way `grim install` does on the same file.
    """
    runner = grim_at(project_dir)
    skill = make_artifact(
        f"{unique_repo}/skills/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\ndescription: A skill\n---\n\nBody.\n"},
        tag="1.0.0",
    )
    write_config(project_dir, skills={"helper": skill.fq})
    runner.json("lock")
    runner.json("install")

    state = project_dir / ".grimoire" / "state.json"
    assert state.is_file(), "the install must have written project state"
    state.write_text("{ not json at all")

    result = runner.run("status", format="json", check=False)
    assert result.returncode != 0, (
        f"a corrupt state file must not be reported as an exit-0 all-missing project: {result.stdout}"
    )
    assert "state.json" in result.stderr, f"the failure must name the file: {result.stderr}"

    # `grim install` fails on the same file, so the two commands agree.
    install = runner.run("install", check=False)
    assert install.returncode != 0, install.stdout


def test_status_absent_install_state_is_not_a_failure(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Guard on the fix above: an *absent* state file is the ordinary
    fresh-project case and must stay exit 0 with `missing` rows."""
    runner = grim_at(project_dir)
    skill = make_artifact(
        f"{unique_repo}/skills/helper",
        "skill",
        {"helper/SKILL.md": "---\nname: helper\ndescription: A skill\n---\n\nBody.\n"},
        tag="1.0.0",
    )
    write_config(project_dir, skills={"helper": skill.fq})
    runner.json("lock")

    assert not (project_dir / ".grimoire" / "state.json").exists()
    row = next(r for r in runner.json("status")["items"] if r["name"] == "helper")
    assert row["state"] == "missing", row
