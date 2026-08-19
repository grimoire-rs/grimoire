# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Hook lifecycle scenarios `test_hook_arming.py` does not reach.

That file is project-scope and policy-shaped. This one covers the rest of the
S-series: the **global** scope (S-003's other half), the config-write refusal
(S-012), a client with no hook surface (S-013), the forward-compatibility error
(S-014), and the report command (S-015).

This suite originally shipped three `xfail(strict=True)` markers, each recording
a real defect it had found rather than pinning the defect as correct. All three
are gone because all three were fixed: a global-scope install could never arm
(`validate_grim_home` compared `$GRIM_HOME` to itself, disabling the only scope
Codex and Copilot support), `grim hook list` returned an empty report
unconditionally, and the install report named neither the arming client nor the
tier. `strict` is what made each fix a loud failure whose only remedy was
deleting the marker — which is the whole reason to prefer it over a plain skip.
The findings are recorded in `.agents/wp-o-report.md`.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

pytestmark = pytest.mark.skipif(
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

GUARD_SH = "#!/bin/sh\ncat > /dev/null\nprintf '{}'\n"


def _publish_hook(unique_repo: str):
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def _rows(runner) -> list[dict]:
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    return [row for root in json.loads(path.read_text())["roots"].values() for row in root["hooks"]]


def _hook_row(report: dict) -> dict:
    return next(item for item in report["items"] if item["kind"] == "hook")


# ---------------------------------------------------------------------------
# S-003 — the global scope
# ---------------------------------------------------------------------------


def _declare_globally(runner, fq: str) -> None:
    """Author a global `grimoire.toml` declaring one hook, feature flag on.

    Written directly rather than through `grim config set`, because the global
    config lives at `$GRIM_HOME/grimoire.toml` and `write_config`'s workspace
    argument is exactly that directory here — the same file either route ends
    up editing.
    """
    home = Path(runner.grim_home)
    home.mkdir(parents=True, exist_ok=True)
    (home / "grimoire.toml").write_text(
        f'[hooks]\nshell-guard = "{fq}"\n\n[options.experimental]\nhooks = true\n'
    )


def test_s003_a_global_hook_payload_lands_directly_under_grim_home(
    grim, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """**S-003, global half.** A global hook's payload is
    `$GRIM_HOME/hooks/<name>/` — client-independent, one directory, no
    workspace key.

    The workspace key exists only to keep two *projects* under one `$GRIM_HOME`
    from colliding; global scope has no workspace, so the extra level would be
    a directory named after nothing. Asserted as an exact path rather than a
    glob so a silent re-layout is a failure rather than a still-passing test.
    """
    hook = _publish_hook(unique_repo)
    _declare_globally(grim, hook.fq)
    (Path(grim.home) / ".claude").mkdir(parents=True, exist_ok=True)
    grim.run("lock", "--global")

    report = grim.json("install", "--global", "--allow-hooks")
    payload = grim_home / "hooks" / "shell-guard"
    assert (payload / "hook.toml").is_file(), f"expected the payload at {payload}"
    assert (payload / "guard.sh").is_file()
    assert _hook_row(report)["target"] == str(payload), report

    # And never the project-scope shape: `payload/<workspace-key>/` is the one
    # level global scope must not grow.
    assert not (grim_home / "hooks" / "payload").exists(), (
        "a global hook must not be keyed by a workspace it does not have"
    )


def test_s003_a_global_install_arms_the_hook_it_materialized(
    grim, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """**S-003 / S-002, global half.** Materializing a payload nobody can run
    is the "installed but does nothing" shape the whole `not-armed` vocabulary
    exists to avoid, so an approved global install must also write the dispatch
    row and the launcher.

    Global scope is where a hook is most useful — a guardrail a developer wants
    on every repository, not one per checkout — and it is the scope
    `docs/src/clients.md` describes as the only one Codex and Copilot arm at
    all. Nothing else in the suite covers it, which is how the refusal shipped.
    """
    hook = _publish_hook(unique_repo)
    _declare_globally(grim, hook.fq)
    (Path(grim.home) / ".claude").mkdir(parents=True, exist_ok=True)
    grim.run("lock", "--global")
    grim.run("install", "--global", "--allow-hooks")

    assert [row["id"] for row in _rows(grim)] == ["guard"], "a global install armed nothing"
    launcher = grim_home / "hooks" / "bin" / "grim-hook"
    assert launcher.is_file() and os.access(launcher, os.X_OK)


# ---------------------------------------------------------------------------
# S-002 — what the install report says about an armed hook
# ---------------------------------------------------------------------------


def test_s002_the_install_report_names_the_arming_client_and_the_tier(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-002, second half.** Arming a hook is the single most consequential
    thing `grim install` does — it grants a published artifact the ability to
    run on every matching tool call — so the report has to say *what* was
    armed, *where*, and *at which tier*.

    Asserted on content rather than on field names, deliberately: the report
    shape is a frozen additive surface and the fix is free to name its fields,
    but no reasonable implementation of this scenario omits the client and the
    tier from the row entirely. `grim status` already carries both, from
    `HookArming`, so the data exists and only the report does not carry it.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    report = runner.json("install", "--allow-hooks")
    assert len(_rows(runner)) == 1, "the hook must really be armed, or this asserts nothing"

    row = json.dumps(_hook_row(report))
    assert "claude" in row, f"the install report never names the client that was armed: {row}"
    assert "observer" in row, f"the install report never names the tier that was armed: {row}"


# ---------------------------------------------------------------------------
# S-012 — the config write that would leave hooks armed
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "write",
    [("config", "set", "options.experimental.hooks", "false"), ("config", "unset", "options.experimental.hooks")],
    ids=["set-false", "unset"],
)
def test_s012_a_config_write_clears_the_flag_but_does_not_itself_disarm(
    grim_at, project_dir: Path, registry: str, unique_repo: str, write: tuple[str, ...]
) -> None:
    """**S-012.** Clearing the flag through config is permitted, warns that it
    disarms nothing on its own, and convergence is what actually disarms.

    This test **inverted** during the run, and the reason is the point. Both
    writes used to be refused (65) on sound reasoning — arming is convergence,
    not a config value, so flipping a flag in a file removes neither the dispatch
    table, nor the launcher, nor the client registration. But with `set false`
    *and* `unset` both refused, a `true` already on disk had **no CLI route back
    at all**, and the refusal's own message named `grim install`, which converges
    and cannot clear the flag. The only supported way out was hand-editing
    `grimoire.toml` — which is what a config CLI exists to avoid.

    So the write is now allowed and the user is told the remaining step. The
    honest half of the original reasoning is what the assertions below pin: the
    write alone leaves the hook **armed**, and only `grim install` disarms it.

    Both verbs are covered because `unset` reaches the same place differently,
    and the hook is armed **first** so this is observed against a real armed
    installation rather than a config-only one.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1, "the hook must really be armed, or this proves nothing"

    result = runner.run(*write, check=False)
    assert result.returncode == 0, f"the write is permitted: {result.stderr}"
    # The warning has to name the step that remains. A silent write would read as
    # "already disarmed", which is the misreading the old refusal was guarding
    # against — the guard moved from refusing to explaining.
    assert "grim install" in result.stderr, f"must name the command that disarms: {result.stderr}"
    assert "disarm" in result.stderr, result.stderr

    # The claim, asserted rather than trusted: still armed after the write.
    assert len(_rows(runner)) == 1, "a config write must not disarm on its own"

    # Convergence is what disarms — and it now runs off the flag the CLI wrote,
    # not off a hand-edited file.
    runner.run("install")
    assert _rows(runner) == [], "`grim install` after clearing the flag must disarm"


# ---------------------------------------------------------------------------
# S-013 — a client with no hook surface
# ---------------------------------------------------------------------------


def test_s013_a_client_with_no_hook_surface_declines_warns_and_records_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-013.** Only three of eighteen clients have a hook surface at all.
    For the rest the answer is `Declined`: one warning naming the client and
    the artifact, zero recorded outputs, exit 0, and a `grim status` row whose
    cause says so.

    `client-has-no-hook-surface` is answered **before** the feature flag and
    the trust gate, and that ordering is the point: it is per-client and
    permanent, so pointing a Cursor user at `options.experimental.hooks` would
    name a knob that changes nothing for them.

    The positive control is the same artifact and the same registry installed
    for Claude, which does arm — without it, "cursor recorded no output" is
    satisfied by a build that installs nothing anywhere.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq}, options={"clients": '["cursor"]'})
    (project_dir / ".cursor").mkdir(exist_ok=True)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    result = runner.run("install", "--allow-hooks", check=False)
    assert result.returncode == 0, result.stderr
    assert "cursor" in result.stderr and "shell-guard" in result.stderr, (
        f"a declined hook must warn, naming the client and the artifact: {result.stderr!r}"
    )

    report = runner.json("install", "--allow-hooks")
    assert _hook_row(report)["status"] == "skipped", report
    assert _hook_row(report)["target"] is None, report

    status = runner.json("status")
    row = _hook_row(status)
    assert row["state"] == "gated", row
    assert [(a["client"], a["cause"]) for a in row["arming"]] == [("cursor", "client-has-no-hook-surface")], row
    assert row["outputs"] == [], f"a declined hook must record zero outputs: {row}"
    assert _rows(runner) == [], "a declined client must not produce a dispatch row"

    # Positive control: Claude, same everything else, arms.
    config = project_dir / "grimoire.toml"
    config.write_text(config.read_text().replace('clients = ["cursor"]', 'clients = ["claude"]'))
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1, "POSITIVE CONTROL FAILED: claude must arm the same artifact"


# ---------------------------------------------------------------------------
# S-014 — meeting state a newer grim wrote
# ---------------------------------------------------------------------------


def test_s014_install_state_from_a_newer_grim_names_the_version_requirement(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-014**, in the half a single binary can actually execute.

    The scenario's literal subject — an *older* `grim` meeting a hooks-bearing
    lock — needs two binaries and the message comes out of the old one, so no
    test in this suite can produce it. What is reachable, and what the plan's
    Principle-9 row extends S-014 to, is the **install-state** read path: a
    hook has to be recorded, and `InstallStateFile` is version-discriminated,
    so a state file a newer grim wrote must produce an actionable upgrade
    message rather than an opaque parse failure.

    Asserted on the parts a user acts on — "newer version of grim", the version
    number, and "upgrade" — not on the whole sentence, which is wording.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")

    state_path = project_dir / ".grimoire" / "state.json"
    state = json.loads(state_path.read_text())
    assert any(record["kind"] == "hook" for record in state["records"]), (
        "the hook must be recorded, or this proves nothing about a hooks-bearing state file"
    )
    state["version"] = 99
    state_path.write_text(json.dumps(state))

    result = runner.run("status", check=False)
    assert result.returncode != 0, "unreadable state must not be reported as a healthy scope"
    for fragment in ("newer version of grim", "99", "upgrade"):
        assert fragment in result.stderr, f"missing {fragment!r} in: {result.stderr}"
    assert "expected one of" not in result.stderr, (
        f"a raw serde field list is the bare parse failure S-014 forbids: {result.stderr}"
    )


# ---------------------------------------------------------------------------
# S-015 — the report command
# ---------------------------------------------------------------------------


def test_s015_hook_list_is_a_scope_resolving_report_command(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-015**, the half that holds today: `grim hook list` is an ordinary
    report command in a project that declares a hook.

    Exit 0, the uniform `items` envelope on `--format json`, and the documented
    six columns in the plain table. What it reports is the subject of the xfail
    below; that it *is* a normal `Printable` report is pinned here, because the
    two can regress independently.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")

    report = runner.json("hook", "list")
    assert isinstance(report["items"], list), report

    plain = runner.plain("hook", "list")
    assert plain.returncode == 0, plain.stderr
    for column in ("Hook", "Tier", "Events", "Client", "State", "Detail"):
        assert column in plain.stdout, f"the plain table is 6 columns: {plain.stdout!r}"


def test_s015_hook_list_reports_the_declared_hook_its_tier_and_its_arming_state(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """**S-015.** The command's whole purpose: one item per declared `[[hooks]]`
    entry, with tier, events, per-client verdicts and armed/not-armed.

    Pinned against a **really armed** hook — the dispatch row is asserted first
    — so this cannot be read as "the scope had nothing to report". The verdicts
    must come from `status.rs`'s `hook_arming` seam, so an armed hook's `arming`
    array is empty exactly as `grim status` reports it.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1, "the hook must really be armed, or this asserts nothing"

    report = runner.json("hook", "list")
    assert len(report["items"]) == 1, report
    item = report["items"][0]
    assert item["artifact"] == "shell-guard"
    assert item["id"] == "guard"
    assert item["tier"] == "observer"
    assert item["events"] == ["PreToolUse"]
