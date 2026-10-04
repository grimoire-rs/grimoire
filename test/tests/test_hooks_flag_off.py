# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Principle 9 gates for the hook kind with the feature flag off
(`design_hooks_pr98_revival.md` § Principle 9 invariants, § UX scenarios).

* **C-152(a)** — from a state that never armed, hooks declared and
  `options.experimental.hooks` unset: `add`, `install`, `update` at both
  scopes write nothing hook-related anywhere, and `grim status` says `gated`.
* **C-152(b)** — on→off: after a genuine arming, flag off + `grim install`
  reaps every registration element and dispatch row, and **keeps** the
  payload, the launcher and the consent record.
* **S-101** — a hook-free project's JSON output gains exactly `arming: []`
  (status) and `armed: null` (install rows), and a re-install writes nothing.
* **S-102** — declaring with the flag off locks, warns once naming the flag,
  and reports `gated`.

The cause literal is `feature-flag-off` (the shipped literal; the design
record's prose says `feature-off`).

Surfaces checked are every hook surface of every hook client: Claude at both
scopes, Codex `hooks.json`, Copilot `hooks/grim.json`, Qoder `settings.json`.
The armed client set is read from the gated `status --global` (clients with a
hook surface), never hard-coded: Qoder has no surface until WP-02 lands, and
from then on it must arm and be reaped like the others, with no edit here.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact

# Only the tests that actually arm and inspect the launcher/registration are
# POSIX-only in v1 — the flag-off, hook-free-upgrade and S-102 gate-and-warn
# scenarios in this module write no hook surface at all, so the skip is
# per-test rather than module-wide (matches `test_bundle_hook_members.py`).
_POSIX_ONLY = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

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

GUARD_SH = "#!/bin/sh\ncat > /dev/null\necho '{}'\n"
FLAG = "options.experimental.hooks"
MARKER_KEY = "com.grimoire.managed"
LAUNCHER = "grim-hook"
# Global-scope detection markers (Copilot detects on its skills root).
HOOK_CLIENTS = (".claude", ".codex", ".copilot/skills", ".qoder")


def _publish_hook(unique_repo: str):
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def _runner(grim_at, project_dir: Path):
    """A project runner whose isolated `$HOME` has every hook client present,
    so global scope detects all of them."""
    runner = grim_at(project_dir)
    for client in HOOK_CLIENTS:
        (runner.home / client).mkdir(parents=True, exist_ok=True)
    return runner


def _surfaces(runner, project_dir: Path) -> list[Path]:
    home = runner.home
    return [
        project_dir / ".claude" / "settings.local.json",
        project_dir / ".claude" / "settings.json",
        home / ".claude" / "settings.json",
        home / ".codex" / "hooks.json",
        home / ".copilot" / "hooks" / "grim.json",
        home / ".qoder" / "settings.json",
    ]


def _grim_elements(node) -> list:
    """Every JSON object that is grim's: marked, or executing grim's launcher."""
    found = []
    if isinstance(node, dict):
        if MARKER_KEY in node or LAUNCHER in str(node.get("command", "")):
            found.append(node)
        for value in node.values():
            found += _grim_elements(value)
    elif isinstance(node, list):
        for value in node:
            found += _grim_elements(value)
    return found


def _registrations(runner, project_dir: Path) -> dict[str, int]:
    """Grim elements per surface file (surfaces holding none are omitted)."""
    counts = {}
    for path in _surfaces(runner, project_dir):
        if path.is_file():
            n = len(_grim_elements(json.loads(path.read_text() or "{}")))
            if n:
                counts[path.relative_to(runner.home.parent).as_posix()] = n
    return counts


# Where each client's global-scope grim registration lives.
GLOBAL_SURFACE = {
    "claude": "home/.claude/settings.json",
    "codex": "home/.codex/hooks.json",
    "copilot": "home/.copilot/hooks/grim.json",
    "qoder": "home/.qoder/settings.json",
}


def _gated_global_clients(runner) -> set[str]:
    """The global clients that WOULD arm: gated only by the flag.

    Derived from the binary, not hard-coded, so a client that gains a hook
    surface (Qoder with WP-02) joins every assertion below without an edit.
    Clients reporting `client-has-no-hook-surface` are excluded.
    """
    (item,) = _hook_items(runner, "--global")
    return {a["client"] for a in item["arming"] if a["cause"] == "feature-flag-off"}


def _rows(runner) -> list[dict]:
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    return [
        row
        for root in json.loads(path.read_text())["roots"].values()
        for row in root["hooks"]
    ]


def _hook_items(runner, *scope: str) -> list[dict]:
    result = runner.run("status", *scope, format="json")
    assert result.returncode == 0, result.stderr
    return [i for i in json.loads(result.stdout)["items"] if i["kind"] == "hook"]


def _assert_gated(runner, *scope: str) -> None:
    items = _hook_items(runner, *scope)
    assert items, f"status {scope} lists no hook row"
    for item in items:
        assert item["state"] == "gated", item
        causes = {a["cause"] for a in item["arming"]}
        # A client with no hook surface at all says so; every other says the flag.
        assert "feature-flag-off" in causes and causes <= {
            "feature-flag-off",
            "client-has-no-hook-surface",
        }, item


# ---------------------------------------------------------------------------
# C-152(a) — clean state, flag off, nothing hook-related written
# ---------------------------------------------------------------------------


def test_c152a_flag_off_from_clean_writes_nothing_hook_related(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`add`, `install`, `update` at both scopes with hooks declared and the
    flag unset: no `$GRIM_HOME/hooks/` at all (so no launcher, dispatch table,
    payload, consent record or ownership record), no grim element in any hook
    surface, `status` gated at both scopes, every command exit 0.

    Positive control: the same declarations arm once the flag is on.
    """
    hook = _publish_hook(unique_repo)
    runner = _runner(grim_at, project_dir)
    runner.run("init")

    for scope in ((), ("--global",)):
        runner.run("add", "--kind", "hook", hook.fq, *scope)
        runner.run("install", *scope)
        runner.run("update", *scope)

    hooks_dir = Path(runner.grim_home) / "hooks"
    assert not hooks_dir.exists(), sorted(p.name for p in hooks_dir.rglob("*"))
    assert not (project_dir / ".grimoire" / "hooks").exists()
    assert _registrations(runner, project_dir) == {}
    _assert_gated(runner)
    _assert_gated(runner, "--global")
    expected = _gated_global_clients(runner)
    assert expected >= {"claude", "codex", "copilot"}, expected

    # ── positive control: the same declarations arm with the flag on ──
    runner.run("config", "set", FLAG, "true", "--global")
    runner.run("install", "--global")
    if os.name == "nt":
        # Nothing arms off Unix (platform-unsupported); the payload landing still
        # proves the flag-off half above was not vacuous.
        assert hooks_dir.is_dir()
        return
    assert {r["client"] for r in _rows(runner)} == expected, _rows(runner)
    armed = _registrations(runner, project_dir)
    assert {GLOBAL_SURFACE[c] for c in expected} <= set(armed), armed


# ---------------------------------------------------------------------------
# C-152(b) — on→off: registrations and rows reaped, payload/launcher/consent kept
# ---------------------------------------------------------------------------


@_POSIX_ONLY
def test_c152b_flag_off_reaps_registrations_and_keeps_payload_launcher_consent(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Genuinely armed at both scopes (project consented by `grim hook allow`,
    global always consented), then flag off + `grim install` at both scopes.

    Removed: every grim element in every hook surface, every dispatch row.
    Retained: the payload trees, the launcher, the project consent record —
    other roots may still use the launcher, and re-enabling the flag must not
    need a fresh consent gesture (asserted at the end).
    """
    hook = _publish_hook(unique_repo)
    runner = _runner(grim_at, project_dir)
    runner.run("init")
    # Declare globally while gated, so the binary names every armable client.
    runner.run("add", "--kind", "hook", hook.fq, "--global")
    expected = _gated_global_clients(runner)
    assert expected >= {"claude", "codex", "copilot"}, expected

    runner.run("config", "set", FLAG, "true")
    runner.run("config", "set", FLAG, "true", "--global")
    runner.run("add", "--kind", "hook", hook.fq, "--no-trust-hooks")
    runner.run("hook", "allow")
    runner.run("install")
    runner.run("install", "--global")

    grim_home = Path(runner.grim_home)
    # Project scope arms claude only; global arms every client the gate named.
    assert sorted(r["client"] for r in _rows(runner)) == sorted(
        ["claude", *expected]
    ), _rows(runner)
    armed = _registrations(runner, project_dir)
    assert {"project/.claude/settings.local.json"} | {
        GLOBAL_SURFACE[c] for c in expected
    } <= set(armed), armed
    launcher = grim_home / "hooks" / "bin" / LAUNCHER
    consent = sorted((grim_home / "hooks" / "consent").glob("*.json"))
    payloads = sorted(grim_home.glob("hooks/**/shell-guard/hook.toml"))
    assert launcher.is_file() and len(consent) == 1 and len(payloads) == 2, (
        launcher,
        consent,
        payloads,
    )

    runner.run("config", "set", FLAG, "false")
    runner.run("config", "set", FLAG, "false", "--global")
    runner.run("install")
    runner.run("install", "--global")

    assert _rows(runner) == [], "every dispatch row must go"
    assert _registrations(runner, project_dir) == {}, (
        "every registration element must go"
    )
    assert launcher.is_file(), "the launcher is retained"
    assert sorted((grim_home / "hooks" / "consent").glob("*.json")) == consent, (
        "consent is retained"
    )
    assert sorted(grim_home.glob("hooks/**/shell-guard/hook.toml")) == payloads, (
        "payloads are retained"
    )
    _assert_gated(runner)
    _assert_gated(runner, "--global")
    for scope in ((), ("--global",)):
        (item,) = _hook_items(runner, *scope)
        # Kept on disk is not enough: the install record must still name the
        # payload, or no later `grim uninstall` can ever reach it.
        assert item["outputs"], (
            f"status {scope}: the record lost its payload outputs {item}"
        )

    # Retained consent is live: re-enabling re-arms with no new gesture.
    runner.run("config", "set", FLAG, "true")
    runner.run("install")
    assert any(r["client"] == "claude" for r in _rows(runner)), (
        "re-enabling must re-arm on the kept consent"
    )


# ---------------------------------------------------------------------------
# S-101 — hook-free stdout gains exactly `arming: []` / `armed: null`
# ---------------------------------------------------------------------------

# Key sets main emits at 3194d0dd (the `p9:diff` base), captured from its binary.
# The C-153 closed list allows exactly one addition to each of the first two.
MAIN_STATUS_ITEM_KEYS = {
    "clients_extra",
    "clients_missing",
    "clients_unresolved",
    "deprecated",
    "kind",
    "name",
    "outputs",
    "outputs_pending",
    "pinned",
    "replaced_by",
    "source",
    "state",
    "update_available",
}
MAIN_INSTALL_ROW_KEYS = {"kind", "name", "status", "target"}
MAIN_UPDATE_ROW_KEYS = {
    "abandoned_entries",
    "action",
    "kept_modified_clients",
    "kind",
    "name",
    "new",
    "old",
    "plugin",
    "reaped_clients",
    "refused",
    "retained",
}
MAIN_ADD_KEYS = {"kind", "name", "pinned", "status"}


def _tree(*roots: Path) -> dict[str, bytes]:
    # Bytes, not mtimes: main itself rewrites `.grimoire/state.json` with
    # identical bytes on an unchanged install, so an mtime is not the contract.
    return {
        str(p): p.read_bytes()
        for root in roots
        for p in sorted(root.rglob("*"))
        if p.is_file()
    }


def test_hook_free_upgrade_is_unchanged(grim_at, project_dir: Path) -> None:
    """S-101: a converged hook-free project re-installs `unchanged` with no
    byte moved; install rows gain exactly `armed: null`, status items
    exactly `arming: []`, `add` and `update` output nothing new (C-153 closed
    list). The binary-swap half (main's bytes, then this binary) is
    `task p9:diff`, which compares both binaries' trees after every command.
    """
    skill = project_dir / "skills" / "demo"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(
        "---\nname: demo\ndescription: A demo skill.\n---\n\nBody.\n"
    )
    runner = grim_at(project_dir)
    runner.run("init")
    added = runner.json("add", "./skills/demo")
    assert set(added) == MAIN_ADD_KEYS, sorted(added)

    roots = (project_dir, Path(runner.grim_home), runner.home)
    before = _tree(*roots)
    rows = runner.json("install")["items"]
    assert rows and [r["status"] for r in rows] == ["unchanged"] * len(rows), rows
    assert _tree(*roots) == before, (
        "a re-install of a converged hook-free project wrote"
    )
    assert not (Path(runner.grim_home) / "hooks").exists()

    for row in rows:
        assert row["armed"] is None, row
        assert set(row) == MAIN_INSTALL_ROW_KEYS | {"armed"}, sorted(row)
    updated = runner.json("update")["items"]
    assert updated, "update emitted no rows to check"
    for row in updated:
        assert set(row) == MAIN_UPDATE_ROW_KEYS, sorted(row)
    items = runner.json("status")["items"]
    assert items
    for item in items:
        assert item["arming"] == [], item
        assert set(item) == MAIN_STATUS_ITEM_KEYS | {"arming"}, sorted(item)


# ---------------------------------------------------------------------------
# S-102 — declare with the flag off
# ---------------------------------------------------------------------------


def test_s102_declaring_a_hook_with_the_flag_off_locks_warns_once_and_gates(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim add` of a hook with the flag off: declared and locked, exit 0,
    **one** warning naming the flag, install row `skipped`, status `gated`,
    and no payload, launcher or dispatch table."""
    hook = _publish_hook(unique_repo)
    runner = grim_at(project_dir)
    runner.run("init")

    result = runner.run("add", "--kind", "hook", hook.fq)
    assert result.returncode == 0
    warnings = [line for line in result.stderr.splitlines() if FLAG in line]
    assert len(warnings) == 1, result.stderr
    assert "shell-guard" in (project_dir / "grimoire.toml").read_text()
    assert "shell-guard" in (project_dir / "grimoire.lock").read_text()

    install = runner.run("install", format="json")
    rows = json.loads(install.stdout)["items"]
    assert [(r["kind"], r["status"]) for r in rows] == [("hook", "skipped")], rows
    assert len([ln for ln in install.stderr.splitlines() if FLAG in ln]) == 1, (
        install.stderr
    )
    _assert_gated(runner)
    assert not (Path(runner.grim_home) / "hooks").exists()
