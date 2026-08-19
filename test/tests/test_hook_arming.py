# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Hook **arming** through the real binary — the composition WP-R exists for.

`test_hook_run_runtime.py` proves the dispatcher runs what a dispatch table
tells it to. This file proves the table and the client registration get
*written* in the first place, and only when policy says so:

* **S-001** — gated (the default) ⇒ `grim add` skips the hook with a warning,
  `grim status` reports `gated`, exit 0, and **nothing** is armed: no dispatch
  table, no launcher, no registration.
* **S-002** — flag on, registry untrusted, no TTY ⇒ declines (C-023);
  `--allow-hooks` ⇒ arms, with the dispatch row keyed on the arming client.
* **S-008** — `grim uninstall hook` reaps the row *and* the registration, and
  leaves a user-authored hook in the same config untouched.
* **Principle 9 self-heal** — a second `grim install` rewrites nothing.
* ⛔ **`grim status`, `grim search` and `grim context` never prompt.** That is
  the failure mode the policy/consent split exists to prevent, and it is the
  easiest thing in the feature to break silently: a hook policy derived inside
  `InstallTarget::parse` would compile and pass everything else.

Every negative here carries a positive control in the same test function. A
build in which nothing arms at all satisfies every "nothing was armed"
assertion — which is exactly the state four waves of this plan shipped in, so a
bare negative is green against a feature that does not exist.

The whole suite is non-interactive by construction: pytest gives the child no
TTY, which *is* C-023's condition. `--allow-hooks` is therefore the only way a
test can arm, and that is the shape CI has too.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

# POSIX-only, matching the launcher's own v1 scope: the shim is a `sh` script
# and the registered command is a POSIX one-liner.
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

GUARD_SH = "#!/bin/sh\ncat > /dev/null\necho '{}'\n"

MARKER_KEY = "com.grimoire.managed"
MARKER_VALUE = "hook-dispatcher"


def _publish_hook(unique_repo: str):
    """Push a real one-hook artifact; the payload tree is rooted at `<name>/`."""
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH},
        tag="1",
    )


def _declare(project_dir: Path, fq: str) -> None:
    write_config(project_dir, hooks={"shell-guard": fq})


def _dispatch(runner) -> dict:
    """The dispatch table, or an empty one when grim never wrote it."""
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return {"roots": {}}
    return json.loads(path.read_text())


def _rows(runner) -> list[dict]:
    """Every dispatch row across every root — the flat arming truth."""
    return [row for root in _dispatch(runner)["roots"].values() for row in root["hooks"]]


def _payload_root(runner) -> Path:
    """`$GRIM_HOME/hooks/payload` — where every project-scope payload tree lives.

    A hook payload is machine-local at both scopes (invariant I1); a project-scope
    one is nested one level deeper, under a per-workspace key, so two workspaces
    sharing one `$GRIM_HOME` cannot collide. The exact key formula is pinned once,
    by `test_a_project_hook_arms_with_nothing_armable_in_the_workspace`; every
    other test here goes through this helper so the layout is stated in one place.
    """
    return Path(runner.grim_home) / "hooks" / "payload"


def _payload(runner, name: str) -> Path | None:
    """`name`'s project-scope payload directory, or `None` when nothing is there."""
    root = _payload_root(runner)
    if not root.is_dir():
        return None
    return next((d / name for d in sorted(root.iterdir()) if (d / name).is_dir()), None)


def _settings(project_dir: Path) -> dict:
    path = project_dir / ".claude" / "settings.local.json"
    return json.loads(path.read_text()) if path.is_file() else {}


def _managed_elements(project_dir: Path) -> list[dict]:
    """Claude's grim-owned handler elements, across every event and group."""
    hooks = _settings(project_dir).get("hooks", {})
    return [
        element
        for groups in hooks.values()
        for group in groups
        for element in group.get("hooks", [])
        if element.get(MARKER_KEY) == MARKER_VALUE
    ]


# ---------------------------------------------------------------------------
# S-001 — gated is the default, and it arms nothing
# ---------------------------------------------------------------------------


def test_s001_gated_add_skips_with_a_warning_and_arms_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The feature flag is off by default (I4), so `grim add` declares, locks,
    and **skips** — with a warning naming the flag, and exit 0.

    The positive control is the second half: the same artifact, same registry,
    with the flag on and `--allow-hooks`, does arm. Without it this test passes
    against a build that cannot arm anything.
    """
    hook = _publish_hook(unique_repo)
    runner = grim_at(project_dir)
    runner.run("init")

    result = runner.run("add", "--kind", "hook", hook.fq, check=True)
    assert result.returncode == 0
    # The warning has to name the remedy, or the user is told "no" with nothing
    # to act on. `grim config set …` is the only way to turn the feature on —
    # there is deliberately no environment form.
    assert "options.experimental.hooks" in result.stderr, result.stderr

    # Declared and locked: declaring is not arming, so neither is refused.
    assert "shell-guard" in (project_dir / "grimoire.toml").read_text()
    assert "shell-guard" in (project_dir / "grimoire.lock").read_text()

    # Nothing armed, on any of the three surfaces that could arm one.
    assert _rows(runner) == []
    assert _managed_elements(project_dir) == []
    assert not (Path(runner.grim_home) / "hooks" / "bin" / "grim-hook").exists()

    # `grim status` says `gated`, and it is not a failure.
    status = runner.json("status")
    row = next(i for i in status["items"] if i["kind"] == "hook")
    assert row["state"] == "gated"
    assert [a["cause"] for a in row["arming"]] == ["feature-flag-off"]

    # ── positive control: the same inputs DO arm once policy allows it ──
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("install", "--allow-hooks")
    assert [r["id"] for r in _rows(runner)] == ["guard"]
    assert len(_managed_elements(project_dir)) == 1


def test_s001_the_payload_is_not_materialized_while_gated(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A gated hook is *skipped*, not installed-and-inert.

    S-003 puts the payload on disk for an **approved** install; writing handler
    scripts into the workspace for a feature the user has not enabled would be
    the "installed but does nothing" shape the whole `not-armed` vocabulary
    exists to avoid.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("lock")

    rows = runner.json("install")["items"]
    assert [r["status"] for r in rows] == ["skipped"], rows
    assert _payload(runner, "shell-guard") is None
    assert not (project_dir / ".grimoire" / "hooks").exists()

    # Positive control: enabling the feature materializes the very same payload.
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("install", "--allow-hooks")
    payload = _payload(runner, "shell-guard")
    assert payload is not None and (payload / "hook.toml").is_file()


# ---------------------------------------------------------------------------
# S-002 — trust, the no-TTY contract, and the flag escape
# ---------------------------------------------------------------------------


def test_s002_no_tty_never_arms_and_never_asks(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C-023: with the flag on and the registry untrusted, a non-interactive run
    declines — it does not block, does not prompt, and exits 0.

    pytest's child has no TTY, so this is the real condition rather than a
    simulated one.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    result = runner.run("install")
    assert result.returncode == 0
    assert "--allow-hooks" in result.stderr, result.stderr
    # The prompt's own text must be absent: grim asked nobody.
    assert "[y/N]" not in result.stderr
    assert _rows(runner) == []
    assert _managed_elements(project_dir) == []

    # Positive control.
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1


def test_s002_allow_hooks_arms_and_the_row_names_the_arming_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The arming proof: one dispatch row per `(hook, client)`, an executable
    launcher, and a marked registration whose command carries the **absolute**
    launcher path and an **opaque** root token.

    Every assertion here is an invariant, not a formatting detail:
    `${GRIM_HOME}` in the command would be CWE-426 (I1), and `--root global` or
    an absolute workspace path would be the two values B3 forbids on the wire.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")

    rows = _rows(runner)
    assert len(rows) == 1, rows
    assert rows[0]["artifact"] == "shell-guard"
    assert rows[0]["id"] == "guard"
    assert rows[0]["client"] == "claude"
    assert rows[0]["event"] == "PreToolUse"
    assert rows[0]["matcher"] == "Bash"

    # The root key is opaque: not `global`, and not the workspace path.
    (token,) = _dispatch(runner)["roots"].keys()
    assert token != "global"
    assert str(project_dir) not in token

    launcher = Path(runner.grim_home) / "hooks" / "bin" / "grim-hook"
    assert launcher.is_file()
    assert os.access(launcher, os.X_OK), "a non-executable shim makes [ -x ] false and the hook never fires"

    (element,) = _managed_elements(project_dir)
    command = element["command"]
    assert str(launcher) in command
    assert "--client claude" in command
    assert "--event PreToolUse" in command
    assert f"--root {token}" in command
    assert "GRIM_HOME" not in command, "an env-derived executed path is CWE-426 (I1)"
    assert "--root global" not in command


def test_a_trust_hooks_entry_in_global_config_arms_with_no_flag(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C-022's actual design: configuring the registry **in global config** is
    the consent act, so no prompt and no flag are needed.

    A project entry is asserted not to grant in the same function — that
    asymmetry (B4) is what stops a hostile clone granting itself trust in four
    committed lines, and it is invisible to any test that only checks the happy
    path.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    # A PROJECT entry, granted as hard as a project file can: still no arming.
    runner.run("config", "registry", "add", "acme", "--oci", f"{registry}/{unique_repo}")
    runner.run("config", "set", "registry.acme.trust_hooks", "true")
    result = runner.run("install")
    assert result.returncode == 0
    assert _rows(runner) == [], "a repository file may restrict hook trust, never grant it (B4)"

    # The same entry in GLOBAL config arms.
    runner.run("config", "registry", "add", "acme", "--oci", f"{registry}/{unique_repo}", "--global")
    runner.run("config", "set", "registry.acme.trust_hooks", "true", "--global")
    runner.run("install")
    assert len(_rows(runner)) == 1


def test_trust_hooks_false_in_the_project_config_beats_a_global_grant(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The deny rule is not a precedence rule: `trust_hooks = false` in **any**
    scope beats every grant, and a project file is allowed to move the answer in
    that one direction.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("config", "registry", "add", "acme", "--oci", f"{registry}/{unique_repo}", "--global")
    runner.run("config", "set", "registry.acme.trust_hooks", "true", "--global")
    # Positive control first: the global grant does arm on its own.
    runner.run("install")
    assert len(_rows(runner)) == 1

    runner.run("config", "registry", "add", "acme", "--oci", f"{registry}/{unique_repo}")
    runner.run("config", "set", "registry.acme.trust_hooks", "false")
    runner.run("install")
    assert _rows(runner) == [], "a project opt-out must disarm despite the global grant"


# ---------------------------------------------------------------------------
# Convergence: self-heal, the reap, and the user's own bytes
# ---------------------------------------------------------------------------


def test_re_installing_an_armed_hook_rewrites_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Principle 9's self-heal obligation: re-materializing leaves `status`
    not-modified and both armed surfaces byte-identical.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")

    table = Path(runner.grim_home) / "hooks" / "dispatch.json"
    settings = project_dir / ".claude" / "settings.local.json"
    before = (table.read_bytes(), settings.read_bytes())

    rows = runner.json("install", "--allow-hooks")["items"]
    assert [r["status"] for r in rows] == ["unchanged"], rows
    assert (table.read_bytes(), settings.read_bytes()) == before

    status = runner.json("status")
    row = next(i for i in status["items"] if i["kind"] == "hook")
    assert row["state"] != "modified", row


def test_s008_uninstall_reaps_the_row_and_the_registration(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim uninstall hook` removes the dispatch row, the registration, and the
    payload — and leaves a **user-authored** hook in the same config untouched.

    The user's own handler is the point: grim owns one marked *element*, never
    the file, so the reap has to be surgical.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    # A hook the user wrote themselves, in the same config grim splices into.
    settings = project_dir / ".claude" / "settings.local.json"
    settings.write_text(
        json.dumps(
            {
                "permissions": {"allow": ["Read"]},
                "hooks": {"PreToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "echo mine"}]}]},
            },
            indent=2,
        )
    )
    authored = json.loads(settings.read_text())

    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1
    assert len(_managed_elements(project_dir)) == 1
    assert "echo mine" in settings.read_text(), "arming must not disturb the user's own handler"

    runner.run("uninstall", "hook", "shell-guard")
    assert _rows(runner) == [], "the dispatch row must go"
    assert _managed_elements(project_dir) == [], "the registration must go"
    assert _payload(runner, "shell-guard") is None, "the payload must go"
    # The user's document comes back exactly as they wrote it.
    assert json.loads(settings.read_text()) == authored


def test_turning_the_feature_flag_off_disarms_an_armed_hook(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Turning the flag off is the same code path as uninstalling: the desired
    set empties and both surfaces are reaped.

    The payload and the record are deliberately **kept** — overwriting the
    record with zero outputs would orphan the files on disk beyond the reach of
    `grim uninstall` forever.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1

    # Turning the flag off goes through the CLI, which is the whole point of
    # this half of the test: `config set … false` used to be refused (65),
    # leaving a `true` on disk with no route back except hand-editing the file.
    # The write is now permitted and warns that it does not itself disarm —
    # the convergence below is what actually disarms, and asserting through the
    # supported route is what keeps that warning honest.
    disarm = runner.run("config", "set", "options.experimental.hooks", "false")
    assert "run `grim install` to disarm" in disarm.stderr, (
        "clearing the flag must say what still has to happen; a silent write "
        f"would read as 'already disarmed'\nstderr: {disarm.stderr}"
    )
    # The warning's claim, asserted rather than trusted: the config write alone
    # leaves the hook armed. If a future change made `config set` converge, this
    # fails and the warning becomes a lie that nothing else would catch.
    assert len(_rows(runner)) == 1, "a config write must not disarm on its own"

    runner.run("install")
    assert _rows(runner) == []
    assert _managed_elements(project_dir) == []
    payload = _payload(runner, "shell-guard")
    assert payload is not None and (payload / "hook.toml").is_file(), (
        "the payload must survive so `grim uninstall` can still reach it"
    )


def test_s007_a_digest_change_cannot_arm_an_untrusted_registry_on_update(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """S-007's re-prompt half, in its post-reversal form.

    Per-hook digest approval was reversed on 2026-08-14, so a *trusted*
    registry does not re-ask when a digest moves — and an **untrusted** one
    cannot arm at any digest, which is what "re-prompt before it can run again"
    reduces to once consent is per registry. `grim update` re-resolves the tag,
    then runs the same consent pass `install` does.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1

    # Move the tag to different bytes: a new digest under the same reference.
    make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {"shell-guard/hook.toml": HOOK_TOML, "shell-guard/guard.sh": GUARD_SH + "# rev 2\n"},
        tag="1",
    )
    # `update` without the flag: the registry is still untrusted, so the new
    # digest cannot arm, and nothing prompts.
    result = runner.run("update")
    assert result.returncode == 0
    assert "[y/N]" not in result.stderr
    assert _rows(runner) == [], "a moved digest must not stay armed on an untrusted registry"

    # Positive control: with the escape, the new digest arms.
    runner.run("update", "--allow-hooks")
    rows = _rows(runner)
    assert len(rows) == 1


def test_trust_hooks_round_trips_through_config_get(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`trust_hooks` is readable through `grim config get` — the one report
    surface that carries it today.

    Pinned because it is the *only* way a user can audit the state of a
    consent-bearing field: `grim config registry show` and `grim context` both
    omit it while carrying the analogous per-registry security field `insecure`
    (verified by execution — see `.agents/wp-r-report.md` § F-5). Whichever way
    that asymmetry is resolved, this route must keep working.
    """
    runner = grim_at(project_dir)
    runner.run("init")
    runner.run("config", "registry", "add", "acme", "--oci", f"{registry}/{unique_repo}")

    # Absent is absent, not `false`: the tri-state is what stops a later
    # `grim add` from silently dropping an authored opt-out.
    assert runner.run("config", "get", "registry.acme.trust_hooks", check=False).returncode == 1

    for value in ("true", "false"):
        runner.run("config", "set", "registry.acme.trust_hooks", value)
        assert runner.run("config", "get", "registry.acme.trust_hooks").stdout.strip() == value


# ---------------------------------------------------------------------------
# ⛔ The negative the split exists for
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("command", [("status",), ("search", ""), ("context",)])
def test_read_only_commands_never_prompt_for_hook_consent(
    grim_at, project_dir: Path, registry: str, unique_repo: str, command: tuple[str, ...]
) -> None:
    """`grim status`, `grim search` and `grim context` must never ask about hook
    trust — with a declared, un-trusted hook present and the feature flag **on**,
    which is the exact state that makes `grim install` prompt.

    They all resolve through `InstallTarget::parse`, so a policy derived there
    would put a terminal question inside a read-only report: a UX defect and an
    I3 violation. The prompt lives above that seam, in
    `command::hook_consent`, which only mutating commands call.

    Asserted on the prompt's own strings rather than on "did it hang", so the
    test fails loudly on a *silent* consent evaluation too.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    result = runner.run(*command, check=False)
    assert result.returncode == 0, result.stderr
    for fragment in ("[y/N]", "are not trusted yet", "Trust hooks from"):
        assert fragment not in result.stderr, f"{command} prompted: {result.stderr}"
        assert fragment not in result.stdout, f"{command} prompted on stdout: {result.stdout}"
    # And none of them may arm as a side effect of reporting.
    assert _rows(runner) == []
    assert _managed_elements(project_dir) == []

    # Positive control, in the same function: the very same state DOES arm
    # through the mutating boundary, so the negatives above are not vacuous.
    runner.run("install", "--allow-hooks")
    assert len(_rows(runner)) == 1


def test_read_only_commands_write_no_hook_trust_into_global_config(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The second half of the same negative: a read-only command must not
    *persist* a grant either.

    A prompt is the visible failure; a silent `trust_hooks = true` appearing in
    global config because a report evaluated consent would be the invisible one.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")

    global_config = Path(runner.grim_home) / "grimoire.toml"
    before = global_config.read_text() if global_config.is_file() else None
    for command in (("status",), ("search", ""), ("context",)):
        runner.run(*command, check=False)
    after = global_config.read_text() if global_config.is_file() else None
    assert after == before, "a read-only command must never touch global config"
    assert "trust_hooks" not in (after or "")


# ---------------------------------------------------------------------------
# T3 — a cloned repository's own committed hook state
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("replant_payload", [False, True], ids=["as-grim-wrote-it", "payload-replanted"])
def test_a_cloned_workspaces_own_committed_hook_state_must_not_arm(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str, replant_payload: bool
) -> None:
    """⛔ **SEC-1.** T3: the victim never reviewed this repository — they cloned it.

    The clone carries a complete `.grimoire/` (state record, lock, and a project
    `grimoire.toml` that turns the feature flag on for itself). The only thing
    between it and arming is the global-only trust rule, and that rule is keyed on
    the registry *the committed record names* — which the attacker chose. So the
    victim's ordinary "I trust this one publisher" global grant is enough, and
    `GRIM_OFFLINE=1` proves no fetch happens: the integrity gate compares the
    *recorded* hash against the on-disk payload, and the attacker supplies both.

    Two variants, because the fix has two halves and only the second variant
    tests the load-bearing one:

    * **as-grim-wrote-it** — a verbatim commit of the donor's `.grimoire/`. The
      record now anchors at `$GRIM_HOME`, which on the victim's machine holds
      nothing, so the gate falls through and offline resolution fails.
    * **payload-replanted** — the attacker also ships the payload *inside the
      repository* and rewrites the record to point at it, which is exactly what
      the pre-fix layout produced and what a hostile author would author by hand.
      This variant fails against a build that merely moved the payload without
      moving the **read**: convergence must derive the payload directory from
      `$GRIM_HOME` and never from the record.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    donor = grim_at(project_dir)
    donor.run("config", "set", "options.experimental.hooks", "true")
    donor.run("lock")
    donor.run("install", "--allow-hooks")
    donor_payload = _payload(donor, "shell-guard")
    assert donor_payload is not None, "the donor must really be armed, or this test proves nothing"

    # The "commit": everything under `.grimoire/`, plus the config and lock.
    clone = tmp_path / "clone"
    import shutil

    shutil.copytree(project_dir, clone)
    shutil.rmtree(clone / ".claude", ignore_errors=True)
    (clone / ".claude").mkdir()

    state_path = clone / ".grimoire" / "state.json"
    if replant_payload:
        planted = clone / ".grimoire" / "hooks" / "shell-guard"
        shutil.copytree(donor_payload, planted)
        state = json.loads(state_path.read_text())
        record = next(r for r in state["records"] if r["kind"] == "hook")
        for output in record["outputs"]:
            output["target"] = {"anchor": "workspace", "relative": ".grimoire/hooks/shell-guard"}
        state_path.write_text(json.dumps(state))
        assert (planted / "hook.toml").is_file()

    # A fresh machine: its own `$GRIM_HOME`, no install history, and a global
    # `trust_hooks = true` for the registry — the ordinary state of a user who
    # trusts one publisher.
    victim_home = tmp_path / "victim-home"
    victim_home.mkdir()
    from src.runner import GrimRunner

    victim = GrimRunner(donor.binary, victim_home, cwd=clone)
    victim.run("config", "registry", "add", "acme", "--oci", f"{registry}/{unique_repo}", "--global")
    victim.run("config", "set", "registry.acme.trust_hooks", "true", "--global")

    victim.env["GRIM_OFFLINE"] = "1"
    victim.run("install", check=False)

    assert _rows(victim) == [], "a repo-resident record must not be an arming authority"
    assert _managed_elements(clone) == []
    assert not (Path(victim.grim_home) / "hooks" / "bin" / "grim-hook").exists(), (
        "no launcher may be generated for a hook that never armed"
    )


def test_a_project_hook_arms_with_nothing_armable_in_the_workspace(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """⛔ **SEC-1 / invariant I1.** An armed project-scope hook leaves **no**
    payload, manifest, or handler under the workspace — only the per-developer
    `.claude/settings.local.json` registration, which is the one repo-resident
    surface I1 admits, plus grim's own `.grimoire/` bookkeeping.

    This test also pins the payload layout formula in one place — every other test
    here reaches it through `_payload`. The key is the SHA-256 of the workspace
    path, so two workspaces under one `$GRIM_HOME` cannot collide, and it is
    deliberately **not** the dispatch root token: a recorded install target is
    printed by `grim status`, while a guessable root token lets a hostile repo's
    own registration fire the victim's hooks (B3).
    """
    import hashlib

    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    rows = runner.json("install", "--allow-hooks")["items"]
    assert len(_rows(runner)) == 1, "positive control: the hook really is armed"

    key = hashlib.sha256(str(project_dir).encode()).hexdigest()
    payload = Path(runner.grim_home) / "hooks" / "payload" / key / "shell-guard"
    assert (payload / "hook.toml").is_file(), f"expected the payload at {payload}"
    assert (payload / "guard.sh").is_file()
    # The install report names that same directory, so a user can find it.
    assert rows[0]["target"] == str(payload), rows

    # Nothing armable under the workspace: no payload tree, no handler script,
    # no manifest — the only grim-written repo-resident file is Claude's local
    # settings registration (and `.grimoire/` state, which arms nothing).
    assert not (project_dir / ".grimoire" / "hooks").exists()
    allowed = {
        project_dir / ".claude" / "settings.local.json",
        project_dir / "grimoire.toml",
        project_dir / "grimoire.lock",
        project_dir / ".grimoire" / "state.json",
        project_dir / ".grimoire" / ".gitignore",
    }
    stray = sorted(p for p in project_dir.rglob("*") if p.is_file() and p not in allowed)
    assert stray == [], f"an armed project hook left files in the workspace: {stray}"

    # The dispatch row points into `$GRIM_HOME`, not the repository — this is the
    # value the runtime chdirs into and executes from.
    assert _rows(runner)[0]["payload_dir"] == str(payload)
    assert not _rows(runner)[0]["payload_dir"].startswith(str(project_dir))


def test_status_reports_an_allow_hooks_arming_as_armed_not_gated(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim status` must not call a live guardrail `gated`.

    `--allow-hooks` grants trust for one invocation and is deliberately never
    persisted, so no config file records it. A verdict derived from config alone
    therefore reports `gated` while the hook is armed and firing — and that is
    the state every CI run is in, because `--allow-hooks` is the only
    non-interactive route.

    Reporting a running guardrail as off is the wrong direction to be wrong in:
    a user who believes a hook is inert may act as if nothing is watching.
    `status` now consults the dispatch table, which is the machine-local arming
    authority and what `grim hook run` actually reads.

    Fails if the table check regresses: without it the row reads `gated` with a
    `registry-not-trusted` cause while `_rows` shows a live dispatch row — the
    exact contradiction this pins.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--allow-hooks")

    # Ground truth: the hook really is armed.
    assert len(_rows(runner)) == 1, "the fixture must actually arm, or this test proves nothing"

    row = next(r for r in runner.json("status")["items"] if r["kind"] == "hook")
    assert row["state"] != "gated", (
        "a hook with a live dispatch row must not report `gated`; the config-derived "
        f"verdict cannot see a per-invocation grant\nrow: {row}"
    )
    assert row["arming"] == [], f"an armed hook reports no arming cause\nrow: {row}"
