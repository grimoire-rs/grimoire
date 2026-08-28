# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Workspace consent — the gate that answers **which checkout may arm hooks**.

The registry gate this replaces answered a different question (*whose code*),
and the difference is the whole point. From `.claude/rules/arch-threat-model.md`:

> **T3 — An untrusted repository the user clones or opens.** […] **The user is
> *not* vouching for a repo by cloning it.**

So the headline here is a negative that the branch never had a test for: a
freshly-cloned repository that declares a hook, from a registry the user
already uses for everything else, with the feature flag **on**, arms nothing.
It is inert, it exits 0, and `grim status` says which gesture would change
that.

Every negative below carries a positive control in the same function. A build
where nothing arms at all satisfies "nothing was armed" perfectly, and that is
the state four waves of this feature shipped in.

**The suite is non-interactive by construction** — pytest gives the child no
TTY, which is C-023's exact condition. So a workspace here is consented by
`grim hook allow` or by `grim add`, never by a prompt, and `--trust-hooks` is
the per-invocation escape. That is the shape CI has too.

**The transport gate is not covered here, deliberately.** The acceptance
registry is `localhost`, which is loopback and therefore exempt from it by
design. Finding B3 recorded that deleting the loopback clause broke no
acceptance test, which is why the plain-HTTP rung is pinned by unit tests in
`src/hook/trust.rs` instead. Do not add a "transport" test here believing it
covers that rung; it cannot.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.registry import REGISTRY_HOST

pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

HOOK_TOML = """\
schema = 1
name = "{name}"
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


def _publish_hook(unique_repo: str, name: str = "shell-guard", tag: str = "1"):
    """Push a real one-hook artifact; the payload tree is rooted at `<name>/`."""
    return make_artifact(
        f"{unique_repo}/{name}",
        "hook",
        {f"{name}/hook.toml": HOOK_TOML.format(name=name), f"{name}/guard.sh": GUARD_SH},
        tag=tag,
    )


def _rows(runner) -> list[dict]:
    """Every dispatch row across every root — the flat arming truth."""
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    table = json.loads(path.read_text())
    return [row for root in table["roots"].values() for row in root["hooks"]]


def _consent_dir(runner) -> Path:
    return Path(runner.grim_home) / "hooks" / "consent"


def _records(runner) -> list[Path]:
    """Every consent record on this machine, in a stable order."""
    d = _consent_dir(runner)
    return sorted(d.glob("*.json")) if d.is_dir() else []


def _record_for(runner, workspace: Path) -> dict | None:
    """The record keyed on `workspace`, read through the key formula grim uses.

    The key is pinned here rather than discovered by globbing, because the
    formula is load-bearing: the payload directory derives from the **same**
    `hook_dispatch::workspace_key`, so a divergence would let a workspace
    consent to one directory and arm from another. Deliberately **not**
    canonicalized — grim hashes the path as resolved, which fails safe.
    """
    key = hashlib.sha256(str(workspace).encode()).hexdigest()
    path = _consent_dir(runner) / f"{key}.json"
    return json.loads(path.read_text()) if path.is_file() else None


def _hook_row(runner) -> dict:
    """The single hook row from `grim status --format json`."""
    out = json.loads(runner.run("status", "--format", "json").stdout)
    rows = [i for i in out["items"] if i["kind"] == "hook"]
    assert len(rows) == 1, f"expected exactly one hook row: {rows}"
    return rows[0]


def _causes(runner) -> set[str]:
    return {a["cause"] for a in _hook_row(runner)["arming"]}


def _enabled(runner) -> None:
    runner.run("config", "set", "options.experimental.hooks", "true")


# ---------------------------------------------------------------------------
# T3 — the headline
# ---------------------------------------------------------------------------


def test_a_cloned_workspace_declaring_a_hook_arms_nothing_until_consented(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """The gate T3 asks for, executed end to end.

    Everything an attacker controls is present and correct: the hook resolves,
    the digest pins, the feature flag is on, and the registry is one this
    machine uses for every other artifact. The one thing missing is the only
    thing that was ever the user's to give — a gesture naming *this checkout* —
    and without it the hook is inert.

    The remedy is asserted too. A gate whose refusal does not name its own
    remedy trains users to reach for `--trust-hooks` permanently, which is how
    a consent gate becomes a formality.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    result = runner.run("install")
    assert result.returncode == 0, "a withheld hook is not a failure (I3)"
    assert _rows(runner) == [], "a clone must not arm a hook it merely declares"
    assert _records(runner) == [], "install must not consent on the user's behalf"

    row = _hook_row(runner)
    assert row["state"] == "gated"
    assert _causes(runner) == {"workspace-not-consented"}
    assert "grim hook allow" in row["arming"][0]["message"]

    # Positive control: the same everything, one gesture later.
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1, "consent arms what was already declared"


def test_consenting_in_one_workspace_arms_nothing_in_another(
    grim_at, tmp_path: Path, unique_repo: str
) -> None:
    """The direnv property, executed (`direnv/direnv#83`).

    `direnv` shipped content-only trust and had to add the path, because an
    approved `.envrc` copied into a hostile directory executed. This is the
    same artifact, the same registry, the same `$GRIM_HOME`, the same
    everything — and the second checkout is still unconsented, because consent
    keys on the directory rather than on what it contains.
    """
    hook = _publish_hook(unique_repo)
    consented = tmp_path / "consented"
    other = tmp_path / "other"
    for ws in (consented, other):
        # The `.claude` marker is what makes each a *detected* workspace, so
        # both resolve the same stable client set — see `project_dir`.
        (ws / ".claude").mkdir(parents=True)
        write_config(ws, hooks={"shell-guard": hook.fq})

    a = grim_at(consented)
    _enabled(a)
    a.run("lock")
    a.run("hook", "allow")
    a.run("install")
    assert len(_rows(a)) == 1

    b = grim_at(other)
    _enabled(b)
    b.run("lock")
    b.run("install")
    # Same machine, same store: the first workspace's rows are still there, so
    # count rows keyed on the second workspace rather than the global total.
    assert _record_for(b, other) is None, "the second checkout carries no record"
    assert "workspace-not-consented" in _causes(b)


# ---------------------------------------------------------------------------
# The write seam — a closed allowlist, asserted as a negative
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "command",
    [("install",), ("update",), ("lock",), ("status",), ("context",), ("hook", "list")],
    ids=["install", "update", "lock", "status", "context", "hook-list"],
)
def test_no_converging_or_reporting_command_writes_a_consent_record(
    grim_at, project_dir: Path, unique_repo: str, command: tuple[str, ...]
) -> None:
    """**This is the T3 control**, and it is a negative by necessity.

    `grim install` materializes what is already declared. A cloned
    repository's `grimoire.toml` is not the user's gesture, so no amount of
    converging over it may become consent — and a report command reaching a
    write seam would be the invisible version of the same defect.

    Asserted by test rather than by visibility: `consent::record` is `pub`
    within the crate, so nothing but this stops a future caller.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run(*command, check=False)
    assert _records(runner) == [], f"`grim {' '.join(command)}` must never write consent"


def test_add_records_consent_because_typing_a_ref_is_the_gesture(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`grim add` is a write seam; `grim install` is not, and the asymmetry is
    the whole design.

    Typing a reference *is* the declaration gesture
    (`adr_artifact_trust_model.md` decision 1). Converging over a file that
    arrived with a clone is not.
    """
    hook = _publish_hook(unique_repo)
    runner = grim_at(project_dir)
    runner.run("init")
    _enabled(runner)

    runner.run("add", "--kind", "hook", hook.fq)
    record = _record_for(runner, project_dir)
    assert record is not None, "typing the ref consents to it"
    assert record["hooks"] == [f"shell-guard@{REGISTRY_HOST}/{hook.repo}"]
    assert len(_rows(runner)) == 1, "and the hook it named arms"


def test_add_unions_and_never_consents_to_a_hook_already_sitting_there(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """The T3 hole that recording the *whole declared set* on `add` would open.

    A hostile clone declares `planted`. The user types `grim add wanted`. If
    `add` recorded everything the workspace declares, that keystroke would
    consent to a hook the user never saw — through the front door, with no
    prompt and no diff. `add` therefore unions in only what the added
    reference brought.
    """
    planted = _publish_hook(unique_repo, name="planted")
    wanted = _publish_hook(unique_repo, name="wanted")
    write_config(project_dir, hooks={"planted": planted.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run("add", "--kind", "hook", wanted.fq)

    record = _record_for(runner, project_dir)
    assert record is not None
    assert record["hooks"] == [f"wanted@{REGISTRY_HOST}/{wanted.repo}"], (
        "adding one hook must not consent to another that was merely lying there"
    )
    armed = {row["artifact"] for row in _rows(runner)}
    assert "planted" not in armed


# ---------------------------------------------------------------------------
# Drift — consented, then the declaration moved
# ---------------------------------------------------------------------------


def test_a_newly_declared_hook_drifts_and_names_itself(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A record covers the set it was given, not the workspace forever.

    Reported apart from "never consented" because the remedies differ: one
    user has never answered, the other answered a different question. Telling
    them apart is the axis issue #92 turned out to be about.
    """
    first = _publish_hook(unique_repo, name="shell-guard")
    second = _publish_hook(unique_repo, name="second-guard")
    write_config(project_dir, hooks={"shell-guard": first.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1

    write_config(project_dir, hooks={"shell-guard": first.fq, "second-guard": second.fq})
    _enabled(runner)  # `write_config` rewrites the whole file, `[options]` included
    runner.run("lock")
    runner.run("install")

    out = json.loads(runner.run("status", "--format", "json").stdout)
    causes = {a["cause"] for i in out["items"] if i["kind"] == "hook" for a in i["arming"]}
    assert "consent-drifted" in causes
    message = next(
        a["message"] for i in out["items"] if i["kind"] == "hook" for a in i["arming"]
        if a["cause"] == "consent-drifted"
    )
    assert "second-guard" in message, f"drift must name what changed: {message}"

    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 2


def test_a_version_bump_of_a_consented_hook_does_not_re_gate(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """The recorded residual, asserted so nobody "fixes" it into a re-prompt.

    Consent entries carry no tag and no digest, so a publisher who owns a
    consented repository can ship a new version without asking again. That is
    **T1**, grim's answer to T1 is the lock's digest pin, and its visibility is
    `git diff` on the lock — **I5**, evidence rather than prevention. Making
    consent re-gate here would not close T1; it would only add a prompt on
    every ordinary upgrade.
    """
    hook = _publish_hook(unique_repo, tag="1")
    write_config(project_dir, hooks={"shell-guard": f"{REGISTRY_HOST}/{hook.repo}:1"})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1

    _publish_hook(unique_repo, tag="2")
    write_config(project_dir, hooks={"shell-guard": f"{REGISTRY_HOST}/{hook.repo}:2"})
    _enabled(runner)  # `write_config` rewrites the whole file, `[options]` included
    runner.run("lock")
    runner.run("install")

    assert len(_rows(runner)) == 1, "an upgrade of a consented hook stays armed"
    assert _causes(runner) == set() or "consent-drifted" not in _causes(runner)


# ---------------------------------------------------------------------------
# `grim hook allow` / `grim hook revoke`
# ---------------------------------------------------------------------------


def test_revoke_is_idempotent_and_disarms_on_the_next_converge(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Revoking twice is exit 0 twice.

    The second run asks for a state that already holds, and putting a failure
    on a command's most ordinary outcome is the defect, not the safety. The
    command also does not disarm by itself — convergence writes the dispatch
    table, so `revoke` removes the authority and the next `install` acts on it.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1

    first = json.loads(runner.run("hook", "revoke", "--format", "json").stdout)
    assert first["action"] == "revoked"
    assert _records(runner) == []
    assert len(_rows(runner)) == 1, "revoke removes the record, not the table"

    second = runner.run("hook", "revoke", "--format", "json")
    assert second.returncode == 0
    assert json.loads(second.stdout)["action"] == "not-consented"

    runner.run("install")
    assert _rows(runner) == [], "the next converge acts on the withdrawn consent"


def test_deny_is_the_same_command_as_revoke_not_a_fourth_state(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`grim hook deny` is an alias, and the report proves it is nothing more.

    `direnv` calls this operation `deny` and carries `revoke` as its own alias
    for it, so a user arriving from the tool this design cites should not have
    to learn which of the two words grim picked. What the alias must **not**
    become is a durable "no": there are three consent states, and *denied* is
    deliberately the same one as never-answered. So `action` is asserted to
    read `revoked` — the operation's name, which consumers branch on — rather
    than a token that follows whichever word was typed.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")
    assert _records(runner) != []

    report = json.loads(runner.run("hook", "deny", "--format", "json").stdout)
    assert report["action"] == "revoked", f"the token names the operation, not the spelling: {report}"
    assert _records(runner) == []

    # Idempotent under the alias too, and still not a recorded refusal: the
    # workspace is back to unconsented, which is what a re-run must report.
    assert json.loads(runner.run("hook", "deny", "--format", "json").stdout)["action"] == "not-consented"
    runner.run("install")
    assert "workspace-not-consented" in _causes(runner), (
        "denying must leave the workspace unconsented, never a fourth state"
    )


def test_allow_replaces_the_recorded_set_rather_than_growing_it(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`allow` is the reviewing gesture, so the set the user was shown is the
    set that lands.

    A hook that dropped out of the declaration must not linger in the record
    as a standing pre-approval for its own return — which is exactly what a
    union here would leave behind.
    """
    first = _publish_hook(unique_repo, name="shell-guard")
    second = _publish_hook(unique_repo, name="second-guard")
    write_config(project_dir, hooks={"shell-guard": first.fq, "second-guard": second.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")
    assert len(_record_for(runner, project_dir)["hooks"]) == 2

    write_config(project_dir, hooks={"shell-guard": first.fq})
    _enabled(runner)  # `write_config` rewrites the whole file, `[options]` included
    runner.run("lock")
    runner.run("hook", "allow")
    assert _record_for(runner, project_dir)["hooks"] == [f"shell-guard@{REGISTRY_HOST}/{first.repo}"]


def test_allow_refuses_the_global_scope_as_a_usage_error(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """`$GRIM_HOME/grimoire.toml` is the user's own file on their own machine.

    There is no third party's checkout to gate, so the global toolchain is
    permanently consented and carries no record. Reporting that as success
    would claim a record that does not exist, and reporting it as an I/O
    failure would blame the filesystem for a decision — so it is **64**,
    naming the reason.
    """
    runner = grim_at(project_dir)
    result = runner.run("hook", "allow", "--global", check=False)
    assert result.returncode == 64
    assert "always consented" in (result.stderr + result.stdout)
    assert _records(runner) == []


def test_a_global_scope_hook_arms_with_no_record_written(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Global scope is always consented, and "never writes a record" is a
    testable invariant rather than a convention.

    `consent::record` refuses `$GRIM_HOME` outright, so an empty consent
    directory here proves the refusal rather than merely observing that
    nothing happened to call it.
    """
    hook = _publish_hook(unique_repo)
    runner = grim_at(project_dir)
    # Global scope detects clients from the isolated `$HOME`, so give it one.
    (Path(runner.home) / ".claude").mkdir(parents=True, exist_ok=True)
    runner.run("config", "set", "options.experimental.hooks", "true", "--global")
    runner.run("add", "--kind", "hook", hook.fq, "--global")

    assert len(_rows(runner)) == 1, "the user's own toolchain needs no gesture"
    assert _records(runner) == [], "and leaves no record behind to revoke"


# ---------------------------------------------------------------------------
# N4 — the flag pair beats the record, in both directions
# ---------------------------------------------------------------------------


def test_the_flag_pair_outranks_the_record_in_both_directions(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A flag typed on this run is the most explicit answer there is, and no
    file can type one.

    That asymmetry is why the pair may outrank a stored answer where a config
    key may not (threat-model **N4**). `--trust-hooks` also writes nothing: it
    is per-invocation by contract, so a CI run never leaves consent behind on
    a shared machine.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run("install", "--trust-hooks")
    assert len(_rows(runner)) == 1, "the flag arms an unconsented workspace"
    assert _records(runner) == [], "and records nothing while doing it"

    runner.run("hook", "allow")
    runner.run("install", "--no-trust-hooks")
    assert _rows(runner) == [], "and refuses a consented one"


def test_the_feature_flag_is_unreachable_past_by_any_consent(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """**I4** — default-deny is answered first, and nothing reaches past it.

    Consent is not a way to turn the feature on. A workspace may be consented
    and still arm nothing, because the two answer different questions and the
    flag is asked first.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")

    runner.run("config", "unset", "options.experimental.hooks")
    runner.run("install", "--trust-hooks")
    assert _rows(runner) == [], "not even the flag pair opens the feature gate"
    assert "feature-flag-off" in _causes(runner)

    _enabled(runner)
    runner.run("install")
    assert len(_rows(runner)) == 1, "and the consent recorded earlier still stands"


# ---------------------------------------------------------------------------
# The record itself
# ---------------------------------------------------------------------------


def test_the_record_is_machine_local_and_never_repo_resident(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """**I1** — nothing armable lives inside a repository, and an approval
    record is named in that list explicitly.

    A repo-resident consent record would travel with the clone, which is the
    same defect one level up: the artifact would arm because the attacker
    shipped the permission for it.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")

    record = _record_for(runner, project_dir)
    assert record is not None, "the record lives under $GRIM_HOME, keyed on the workspace"
    assert record["workspace"] == str(project_dir), "the path is the identity, not the filename"
    assert record["v"] == 1

    inside = [p for p in project_dir.rglob("*consent*") if p.is_file()]
    assert inside == [], f"nothing consent-shaped may sit in the repository: {inside}"


def test_an_unreadable_record_degrades_to_unconsented_and_never_to_an_error(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """**I3** — grim degrades to "the feature is off", never to "the agent is
    blocked".

    A truncated or future-versioned record is treated as absent. Failing hard
    would let a corrupt file in `$GRIM_HOME` deny every command in every
    workspace, which is the availability failure I3 exists to forbid — and
    treating it as *valid* would be the security failure on the other side.
    """
    hook = _publish_hook(unique_repo)
    write_config(project_dir, hooks={"shell-guard": hook.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1

    for corrupt in ('{"v":99,"workspace":"/x","hooks":[],"consented_at":"x"}', '{"v":1,"work'):
        _records(runner)[0].write_text(corrupt)
        result = runner.run("install", check=False)
        assert result.returncode == 0, f"an unreadable record must not fail a command: {corrupt}"
        assert "workspace-not-consented" in _causes(runner), corrupt
