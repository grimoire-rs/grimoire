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
import re
import shutil
from pathlib import Path

import pytest

from src.helpers import make_artifact, make_bundle, make_dependent, write_config
from src.registry import REGISTRY_HOST, retag

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


def _publish_hook(unique_repo: str, name: str = "shell-guard", tag: str = "1", payload: str = GUARD_SH):
    """Push a real one-hook artifact; the payload tree is rooted at `<name>/`."""
    return make_artifact(
        f"{unique_repo}/{name}",
        "hook",
        {f"{name}/hook.toml": HOOK_TOML.format(name=name), f"{name}/guard.sh": payload},
        tag=tag,
    )


def _rows(runner) -> list[dict]:
    """Every dispatch row across every root — the flat arming truth."""
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    table = json.loads(path.read_text(encoding="utf-8"))
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
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


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
    """**I3** — no benign state errors, and grim forges no verdict of its own.

    A truncated or future-versioned record is treated as absent: the workspace
    is simply unconsented, and the command exits 0 with nothing armed. Erroring
    on it would turn a corrupt file in `$GRIM_HOME` into a grim-made failure in
    every workspace — and treating it as *valid* would be the security failure
    on the other side.
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
        _records(runner)[0].write_text(corrupt, encoding="utf-8", newline="\n")
        result = runner.run("install", check=False)
        assert result.returncode == 0, f"an unreadable record must not fail a command: {corrupt}"
        assert "workspace-not-consented" in _causes(runner), corrupt


# ---------------------------------------------------------------------------
# Dependencies — consent follows what the typed reference newly brings
# ---------------------------------------------------------------------------


def _skill_requiring(unique_repo: str, name: str, hooks: list[str], tag: str = "1"):
    """A skill whose requires layer names each of `hooks` (published beside it)."""
    return make_dependent(
        f"{unique_repo}/{name}",
        tag,
        "skill",
        {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"},
        [("hook", h, f"./{h}:1") for h in hooks],
    )


def _hook_names(rows: list[dict]) -> set[str]:
    return {row["artifact"] for row in rows}


def _hook_state(runner, name: str) -> dict:
    out = json.loads(runner.run("status", "--format", "json").stdout)
    return next(i for i in out["items"] if i["kind"] == "hook" and i["name"] == name)


def _drop_lock_block(lock: Path, table: str) -> None:
    """Delete every `[[<table>]]` block from a lock (a hand-edit, or a stale lock)."""
    text = lock.read_text(encoding="utf-8")
    lock.write_text(re.sub(rf"(?ms)^\[\[{table}\]\]\n.*?(?=^\[\[|\Z)", "", text), encoding="utf-8", newline="\n")


def test_add_does_not_consent_preexisting_hook(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A hook the workspace already declared stays unconsented even when the
    added artifact requires it too: `add` records only what it newly reaches."""
    planted = _publish_hook(unique_repo, name="planted")
    fresh = _publish_hook(unique_repo, name="fresh")
    wanted = _skill_requiring(unique_repo, "wanted", ["planted", "fresh"])
    write_config(project_dir, hooks={"planted": planted.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run("add", wanted.fq)

    record = _record_for(runner, project_dir)
    assert record is not None, "positive control: the newly reached hook is consented"
    assert record["hooks"] == [f"fresh@{REGISTRY_HOST}/{fresh.repo}"]
    assert _hook_names(_rows(runner)) == {"fresh"}
    assert "consent-drifted" in {a["cause"] for a in _hook_state(runner, "planted")["arming"]}


def test_add_does_not_consent_when_closure_incomputable(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """When the pre-add closure cannot be computed offline, every hook already
    in the config or the previous lock is excluded rather than assumed new."""
    _publish_hook(unique_repo, name="planted")
    fresh = _publish_hook(unique_repo, name="fresh")
    holder = _skill_requiring(unique_repo, "holder", ["planted"])
    wanted = _skill_requiring(unique_repo, "wanted", ["planted", "fresh"])
    write_config(project_dir, skills={"holder": holder.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    # The followed edge holder -> planted now has no registry entry: the
    # closure is unknowable without the network.
    _drop_lock_block(project_dir / "grimoire.lock", "hook")
    assert "[[hook]]" not in (project_dir / "grimoire.lock").read_text(encoding="utf-8")

    runner.run("add", wanted.fq)

    record = _record_for(runner, project_dir)
    assert record is not None, "positive control: the genuinely new hook is consented"
    assert record["hooks"] == [f"fresh@{REGISTRY_HOST}/{fresh.repo}"], record
    assert "planted" not in _hook_names(_rows(runner))


def test_add_bundle_does_not_consent_preexisting_member(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A bundle that lists a hook another declared bundle already provides
    consents to its new hook members only."""
    planted = _publish_hook(unique_repo, name="planted")
    fresh = _publish_hook(unique_repo, name="fresh")
    bundle = make_bundle(
        f"{unique_repo}/pack",
        [("hook", "planted", planted.fq), ("hook", "fresh", fresh.fq)],
        tag="1",
    )
    old = make_bundle(f"{unique_repo}/old", [("hook", "planted", planted.fq)], tag="1")
    write_config(project_dir, bundles={"old": old.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run("add", bundle.fq)

    record = _record_for(runner, project_dir)
    assert record is not None, "positive control: the bundle's new hook is consented"
    assert record["hooks"] == [f"fresh@{REGISTRY_HOST}/{fresh.repo}"], record
    assert _hook_names(_rows(runner)) == {"fresh"}


def test_install_writes_no_consent_for_dependency(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A hook reached only through `requires` is gated like any declared hook:
    converging writes no record and arms nothing until the workspace consents."""
    _publish_hook(unique_repo, name="deep")
    dep = _skill_requiring(unique_repo, "dep", ["deep"])
    write_config(project_dir, skills={"dep": dep.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run("install")

    assert _records(runner) == [], "install must never consent"
    assert _rows(runner) == []
    assert "workspace-not-consented" in {a["cause"] for a in _hook_state(runner, "deep")["arming"]}

    # Positive control: the explicit gesture arms the very same hook.
    runner.run("hook", "allow")
    runner.run("install")
    assert _hook_names(_rows(runner)) == {"deep"}


def test_gained_hook_drifts(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A dependency that gains a hook between versions does not arm it: the
    next `update` leaves it gated as drift naming the hook."""
    known = _publish_hook(unique_repo, name="known")
    gained = _publish_hook(unique_repo, name="gained")
    files = {"b/SKILL.md": "---\nname: b\ndescription: d\n---\n# b\n"}
    make_artifact(f"{unique_repo}/b", "skill", files, tag="1")
    top = make_dependent(
        f"{unique_repo}/top", "1", "skill",
        {"top/SKILL.md": "---\nname: top\ndescription: d\n---\n# top\n"},
        [("skill", "b", "./b:1")],
    )
    runner = grim_at(project_dir)
    runner.run("init")
    _enabled(runner)
    runner.run("add", "--kind", "hook", known.fq)
    runner.run("add", top.fq)
    assert _hook_names(_rows(runner)) == {"known"}

    b2 = make_dependent(
        f"{unique_repo}/b", "2", "skill", files, [("hook", "gained", "./gained:1")]
    )
    retag(f"{unique_repo}/b", "1", b2.digest)
    runner.run("update")

    state = _hook_state(runner, "gained")
    drift = [a for a in state["arming"] if a["cause"] == "consent-drifted"]
    assert drift and "gained" in drift[0]["message"], state
    assert "gained" not in _hook_names(_rows(runner)), "the gained hook must not arm"
    assert gained.digest in (project_dir / "grimoire.lock").read_text(encoding="utf-8")


def test_transitive_hook_transport_parity(
    grim_at, tmp_path: Path, unique_repo: str
) -> None:
    """A hook reached through `requires` follows the same arming gates as the
    same hook delivered by a bundle: same dispatch row, same per-invocation refusal."""
    hook = _publish_hook(unique_repo, name="shared")
    via_requires = _skill_requiring(unique_repo, "carrier", ["shared"])
    bundle = make_bundle(f"{unique_repo}/pack", [("hook", "shared", hook.fq)], tag="1")

    def arm(name: str, ref: str, *flags: str) -> tuple[list[dict], str]:
        proj = tmp_path / name
        proj.mkdir()
        (proj / ".claude").mkdir()
        runner = grim_at(proj)
        runner.run("init")
        _enabled(runner)
        before = _rows(runner)  # one $GRIM_HOME serves every project in a test
        out = runner.run("add", *flags, ref)
        return [r for r in _rows(runner) if r not in before], out.stderr

    rows_req, err_req = arm("via-requires", via_requires.fq)
    rows_bun, err_bun = arm("via-bundle", bundle.fq)
    refused_req, _ = arm("refused-requires", via_requires.fq, "--no-trust-hooks")
    refused_bun, _ = arm("refused-bundle", bundle.fq, "--no-trust-hooks")
    assert refused_req == refused_bun == [], "--no-trust-hooks refuses both alike"

    assert len(rows_req) == len(rows_bun) == 1
    keys = lambda r: {k: v for k, v in r[0].items() if k not in ("payload_dir", "resolved_digest")}
    assert keys(rows_req) == keys(rows_bun)
    assert "hook 'shared' is required by skill 'carrier'" in err_req, err_req
    assert "hook 'shared' is delivered by bundle" in err_bun, err_bun


# ---------------------------------------------------------------------------
# Consent needs a manifest this run read, and an install that succeeded
# ---------------------------------------------------------------------------

_LOCK_HEADER = f"""\
[metadata]
lock_version = 1
declaration_hash_version = 1
declaration_hash = "sha256:{'d' * 64}"
generated_by = "grim 0.15.1"
generated_at = "2026-11-01T00:00:00Z"
"""


def _skill(unique_repo: str, name: str):
    files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}
    return make_artifact(f"{unique_repo}/{name}", "skill", files, tag="1")


def _hooked_workspace(grim_at, project_dir: Path):
    """A fresh workspace with the feature on and no record."""
    runner = grim_at(project_dir)
    runner.run("init")
    _enabled(runner)
    return runner


def _warmed(grim_at, tmp_path: Path, **declared):
    """Lock `declared` in a sibling workspace: a test's one `$GRIM_HOME` now
    stores what that read, and the workspace under test stays untouched."""
    warm = tmp_path / "warm"
    (warm / ".claude").mkdir(parents=True)
    write_config(warm, **declared)
    runner = grim_at(warm)
    _enabled(runner)
    runner.run("lock")
    return runner


def _evict(runner, digest: str) -> None:
    algorithm, hex_digest = digest.split(":")
    (Path(runner.grim_home) / "blobs" / algorithm / hex_digest[:2] / hex_digest).unlink()


def _consented(runner, workspace: Path) -> set[str]:
    record = _record_for(runner, workspace)
    return set(record["hooks"]) if record else set()


def _armed_in(runner, workspace: Path) -> set[str]:
    """The hooks armed for `workspace`; one `$GRIM_HOME` serves every project in a test."""
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return set()
    roots = json.loads(path.read_text(encoding="utf-8"))["roots"].values()
    return {row["artifact"] for root in roots if root["root"] == str(workspace) for row in root["hooks"]}


def _key(hook) -> str:
    return f"{hook.repo.rsplit('/', 1)[-1]}@{REGISTRY_HOST}/{hook.repo}"


def _planted_bundle_row(unique_repo: str):
    """A published bundle, and a `[[bundle]]` row claiming `planted` for it."""
    planted = _publish_hook(unique_repo, name="planted")
    filler = _skill(unique_repo, "filler")
    inner = make_bundle(f"{unique_repo}/inner", [("skill", "filler", filler.fq)], tag="1")
    return inner, f"""
[[bundle]]
name = "inner"
repo = "{REGISTRY_HOST}/{inner.repo}"
tag = "{inner.tag}"
pinned = "{inner.pinned}"
transitive = true

[[bundle.member]]
kind = "hook"
name = "planted"
id = "{planted.pinned}"
"""


def _planted_requires_row(unique_repo: str):
    """A published skill, and a `[[skill.requires]]` row claiming `planted` for it."""
    planted = _publish_hook(unique_repo, name="planted")
    dep = _skill(unique_repo, "dep")
    return dep, f"""
[[skill]]
name = "dep"
pinned = "{dep.pinned}"

[[skill.requires]]
kind = "hook"
name = "planted"
id = "{planted.pinned}"
"""


def _stored(grim_at, tmp_path: Path, **declared) -> object:
    """[`_warmed`], installed with its hooks armed: the blobs of each are stored."""
    warm = _warmed(grim_at, tmp_path, **declared)
    warm.run("hook", "allow")
    warm.run("install")
    return warm


def _planted_reference(unique_repo: str) -> str:
    return f"{REGISTRY_HOST}/{unique_repo}/planted:1"


def _on_disk(runner, artifact) -> None:
    """Install `artifact`, drop its declaration and forget its manifest: its files
    are all an offline install needs, and only a lock row can name its edges."""
    runner.run("add", "--kind", artifact.kind, artifact.pinned)
    runner.run("remove", artifact.kind, artifact.repo.rsplit("/", 1)[-1])
    _evict(runner, artifact.digest)


def _requires_row_as_target(grim_at, tmp_path: Path, runner, unique_repo: str):
    dep, row = _planted_requires_row(unique_repo)
    _stored(grim_at, tmp_path, hooks={"planted": _planted_reference(unique_repo)})
    _on_disk(runner, dep)
    return dep.kind, dep.pinned, row


def _bundle_row_below_target(grim_at, tmp_path: Path, runner, unique_repo: str):
    inner, row = _planted_bundle_row(unique_repo)
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    declared = {"hooks": {"planted": _planted_reference(unique_repo)}, "bundles": {"outer": outer.fq}}
    _evict(_stored(grim_at, tmp_path, **declared), inner.digest)
    return outer.kind, outer.fq, row


def _requires_row_below_target(grim_at, tmp_path: Path, runner, unique_repo: str):
    dep, row = _planted_requires_row(unique_repo)
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "dep", dep.pinned)], tag="1")
    declared = {"hooks": {"planted": _planted_reference(unique_repo)}, "bundles": {"outer": outer.fq}}
    _stored(grim_at, tmp_path, **declared)
    _on_disk(runner, dep)
    return outer.kind, outer.fq, row


def _requires_row_elsewhere(grim_at, tmp_path: Path, runner, unique_repo: str):
    """The row claims the digest for another repository, so the node it answers
    for is new to the lock and the walk reaches below it."""
    dep, row = _planted_requires_row(unique_repo)
    outer = make_bundle(f"{unique_repo}/outer", [("skill", "dep", dep.pinned)], tag="1")
    declared = {"hooks": {"planted": _planted_reference(unique_repo)}, "bundles": {"outer": outer.fq}}
    _stored(grim_at, tmp_path, **declared)
    _on_disk(runner, dep)
    return outer.kind, outer.fq, row.replace(dep.pinned, f"{REGISTRY_HOST}/{unique_repo}/fork@{dep.digest}")


@pytest.mark.parametrize(
    "stand_in",
    [_requires_row_as_target, _bundle_row_below_target, _requires_row_below_target, _requires_row_elsewhere],
    ids=["requires-row-target", "bundle-row-nested", "requires-row-nested", "requires-row-elsewhere"],
)
def test_offline_add_records_no_consent_for_a_stand_in(
    grim_at, tmp_path: Path, project_dir: Path, unique_repo: str, stand_in
) -> None:
    """A lock row answering for a manifest may inform a report, never a consent
    record. Each run succeeds with the hook's blobs stored, so the consent gates
    stop the record and no failed install does.

    A `[[bundle]]` row has no target case: a bundle new to the declaration holds
    no pin, so offline it exits 69 before any row is read.
    """
    runner = _hooked_workspace(grim_at, project_dir)
    kind, reference, row = stand_in(grim_at, tmp_path, runner, unique_repo)
    lock = project_dir / "grimoire.lock"
    lock.write_text(_LOCK_HEADER + row, encoding="utf-8", newline="\n")

    runner.run("add", "--offline", "--kind", kind, reference)

    assert "d" * 64 not in lock.read_text(encoding="utf-8"), "the relock saved, so the row answered"
    assert 'name = "planted"' in lock.read_text(encoding="utf-8")
    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()


def test_a_stand_in_hook_stays_gated_on_the_next_online_install(
    grim_at, tmp_path: Path, project_dir: Path, unique_repo: str
) -> None:
    """An offline `add` leaves nothing the next online `install` arms on.

    The positive control shows the record is the one thing in the way: the
    gesture that writes it is what arms the same lock.
    """
    runner = _hooked_workspace(grim_at, project_dir)
    kind, reference, row = _bundle_row_below_target(grim_at, tmp_path, runner, unique_repo)
    (project_dir / "grimoire.lock").write_text(_LOCK_HEADER + row, encoding="utf-8", newline="\n")
    runner.run("add", "--offline", "--kind", kind, reference)

    runner.run("install")

    assert _armed_in(runner, project_dir) == set()
    assert "workspace-not-consented" in {a["cause"] for a in _hook_state(runner, "planted")["arming"]}

    runner.run("hook", "allow")
    runner.run("install")
    assert _armed_in(runner, project_dir) == {"planted"}


def test_offline_add_consents_to_what_its_stored_manifests_list(
    grim_at, tmp_path: Path, project_dir: Path, unique_repo: str
) -> None:
    """Offline the typed target consents and arms in the same run, and so does a
    hook it reaches over edges the user's own store lists. The stand-in cases are
    `test_offline_add_records_no_consent_for_a_stand_in`."""
    target = _publish_hook(unique_repo, name="typed")
    member = _publish_hook(unique_repo, name="member")
    pack = make_bundle(f"{unique_repo}/pack", [("hook", "member", member.fq)], tag="1")
    warm = _warmed(grim_at, tmp_path, hooks={"typed": target.fq}, bundles={"pack": pack.fq})
    warm.run("hook", "allow")
    warm.run("install")
    runner = _hooked_workspace(grim_at, project_dir)

    runner.run("add", "--offline", "--kind", "bundle", pack.fq)

    assert _consented(runner, project_dir) == {_key(member)}
    assert _armed_in(runner, project_dir) == {"member"}

    runner.run("add", "--offline", "--kind", "hook", target.fq)

    assert _consented(runner, project_dir) == {_key(member), _key(target)}
    assert _armed_in(runner, project_dir) == {"member", "typed"}


def test_a_failed_add_records_the_consent_its_gesture_carried(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Online form: the failed add keeps its declaration and lock, so it keeps
    its consent too. The hook it did install arms, and once the user clears the
    refusal the next `grim install` needs no `grim hook allow`.
    """
    fresh = _publish_hook(unique_repo, name="fresh")
    clash = _skill(unique_repo, "clash")
    pack = make_bundle(f"{unique_repo}/pack", [("hook", "fresh", fresh.fq), ("skill", "clash", clash.fq)], tag="1")
    runner = _hooked_workspace(grim_at, project_dir)
    theirs = project_dir / ".claude" / "skills" / "clash" / "SKILL.md"
    theirs.parent.mkdir(parents=True)
    theirs.write_text("not grim's\n", encoding="utf-8", newline="\n")

    result = runner.run("add", pack.fq, check=False)

    assert result.returncode == 65, result.stderr
    assert _consented(runner, project_dir) == {_key(fresh)}
    assert _armed_in(runner, project_dir) == {"fresh"}

    shutil.rmtree(theirs.parent)
    runner.run("install")
    assert _armed_in(runner, project_dir) == {"fresh"}


def test_a_failed_offline_add_records_its_typed_target(
    grim_at, tmp_path: Path, project_dir: Path, unique_repo: str
) -> None:
    """Offline form: the target's manifest is stored, its layer is not. The
    typed target is the gesture whether or not its layer arrives, so the record
    covers it, and the online retry arms it."""
    hook = _publish_hook(unique_repo)
    _warmed(grim_at, tmp_path, hooks={"shell-guard": hook.fq})
    runner = _hooked_workspace(grim_at, project_dir)

    result = runner.run("add", "--offline", "--kind", "hook", hook.fq, check=False)

    assert result.returncode == 81, result.stderr
    assert _consented(runner, project_dir) == {_key(hook)}
    assert _armed_in(runner, project_dir) == set()

    runner.run("add", "--kind", "hook", hook.fq)
    assert _consented(runner, project_dir) == {_key(hook)}
    assert _armed_in(runner, project_dir) == {"shell-guard"}


def test_a_failed_add_keeps_what_the_record_already_armed(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A failed `add` withdraws nothing the record covers and keeps the
    consent its own gesture carried."""
    kept = _publish_hook(unique_repo, name="kept")
    fresh = _publish_hook(unique_repo, name="fresh")
    clash = _skill(unique_repo, "clash")
    pack = make_bundle(f"{unique_repo}/pack", [("hook", "fresh", fresh.fq), ("skill", "clash", clash.fq)], tag="1")
    runner = _hooked_workspace(grim_at, project_dir)
    runner.run("add", "--kind", "hook", kept.fq)
    theirs = project_dir / ".claude" / "skills" / "clash" / "SKILL.md"
    theirs.parent.mkdir(parents=True)
    theirs.write_text("not grim's\n", encoding="utf-8", newline="\n")

    assert runner.run("add", pack.fq, check=False).returncode == 65

    assert _consented(runner, project_dir) == {_key(kept), _key(fresh)}
    assert _armed_in(runner, project_dir) == {"kept", "fresh"}


def test_a_failed_add_records_no_more_than_its_gesture_covered(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """The write precedes the install, and it is the same write: a hook already
    in the workspace stays out of the record, even when the failed bundle lists
    it."""
    planted = _publish_hook(unique_repo, name="planted")
    fresh = _publish_hook(unique_repo, name="fresh")
    clash = _skill(unique_repo, "clash")
    pack = make_bundle(
        f"{unique_repo}/pack",
        [("hook", "planted", planted.fq), ("hook", "fresh", fresh.fq), ("skill", "clash", clash.fq)],
        tag="1",
    )
    old = make_bundle(f"{unique_repo}/old", [("hook", "planted", planted.fq)], tag="1")
    write_config(project_dir, bundles={"old": old.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    theirs = project_dir / ".claude" / "skills" / "clash" / "SKILL.md"
    theirs.parent.mkdir(parents=True)
    theirs.write_text("not grim's\n", encoding="utf-8", newline="\n")

    result = runner.run("add", pack.fq, check=False)

    assert result.returncode == 65, result.stderr
    assert _consented(runner, project_dir) == {_key(fresh)}


def test_an_add_whose_relock_fails_records_nothing(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A relock that fails rolls the declaration back, so no consent survives it."""
    hook = _publish_hook(unique_repo)
    pack = make_bundle(
        f"{unique_repo}/pack",
        [("hook", "shell-guard", hook.fq), ("skill", "gone", f"{REGISTRY_HOST}/{unique_repo}/gone:1")],
        tag="1",
    )
    runner = _hooked_workspace(grim_at, project_dir)
    declared = (project_dir / "grimoire.toml").read_text(encoding="utf-8")

    result = runner.run("add", pack.fq, check=False)

    assert result.returncode != 0, result.stderr
    assert (project_dir / "grimoire.toml").read_text(encoding="utf-8") == declared
    assert _record_for(runner, project_dir) is None


def test_a_record_that_cannot_be_written_arms_nothing(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """I3: a persist failure after a successful install warns, exits 0 and
    leaves the hook unarmed; the next `install` gates it."""
    hook = _publish_hook(unique_repo)
    runner = _hooked_workspace(grim_at, project_dir)
    blocked = _consent_dir(runner)
    blocked.parent.mkdir(parents=True, exist_ok=True)
    blocked.write_text("", encoding="utf-8", newline="\n")

    result = runner.run("add", "--kind", "hook", hook.fq)

    assert "could not be recorded" in result.stderr, result.stderr
    assert _armed_in(runner, project_dir) == set()

    blocked.unlink()
    runner.run("install")
    assert _armed_in(runner, project_dir) == set()
    assert _records(runner) == []
    assert "workspace-not-consented" in _causes(runner)


def test_add_consents_no_hook_reached_only_through_a_carried_bundle(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A carried node's edges answer for the previous lock, not for this
    gesture. The hook installs and gates; the run exits 0."""
    planted = _publish_hook(unique_repo, name="planted")
    inner = make_bundle(f"{unique_repo}/inner", [("hook", "planted", planted.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    runner = _hooked_workspace(grim_at, project_dir)
    (project_dir / "grimoire.lock").write_text(
        _LOCK_HEADER
        + f"""
[[bundle]]
name = "inner"
repo = "{REGISTRY_HOST}/{inner.repo}"
tag = "{inner.tag}"
pinned = "{inner.pinned}"
transitive = true
""", encoding="utf-8", newline="\n"
    )

    runner.run("add", "--kind", "bundle", outer.fq)

    assert 'name = "planted"' in (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()


def test_add_consents_a_hook_reached_through_what_it_resolved(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """Target, new bundle, new hook: an honest lock loses nothing, beside
    a previous lock that holds something else."""
    fresh = _publish_hook(unique_repo, name="fresh")
    inner = make_bundle(f"{unique_repo}/inner", [("hook", "fresh", fresh.fq)], tag="1")
    outer = make_bundle(f"{unique_repo}/outer", [("bundle", "inner", inner.fq)], tag="1")
    other = _skill(unique_repo, "other")
    write_config(project_dir, skills={"other": other.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")

    runner.run("add", "--kind", "bundle", outer.fq)

    assert _consented(runner, project_dir) == {_key(fresh)}
    assert _armed_in(runner, project_dir) == {"fresh"}


def test_add_leaves_a_hook_behind_repaired_edges_unconsented(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """A lock missing an edge repairs it from the carried node's manifest.
    That hook came from no gesture, so it gates and the run exits 0."""
    _publish_hook(unique_repo, name="deep")
    dep = _skill_requiring(unique_repo, "dep", ["deep"])
    wanted = make_dependent(
        f"{unique_repo}/wanted",
        "1",
        "skill",
        {"wanted/SKILL.md": "---\nname: wanted\ndescription: d\n---\n# wanted\n"},
        [("skill", "dep", "./dep:1")],
    )
    write_config(project_dir, skills={"dep": dep.fq})
    runner = grim_at(project_dir)
    _enabled(runner)
    runner.run("lock")
    lock = project_dir / "grimoire.lock"
    _drop_lock_block(lock, "skill.requires")
    _drop_lock_block(lock, "hook")
    assert "deep" not in lock.read_text(encoding="utf-8")

    runner.run("add", wanted.fq)

    assert 'name = "deep"' in lock.read_text(encoding="utf-8"), "the relock repaired the edge"
    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()


# ---------------------------------------------------------------------------
# Consent needs an edge this run resolved
# ---------------------------------------------------------------------------


def _cloned(project_dir: Path, **declared) -> None:
    """A checkout whose own `grimoire.toml` turns the feature on."""
    write_config(project_dir, **declared)
    config = project_dir / "grimoire.toml"
    config.write_text(config.read_text(encoding="utf-8") + "\n[options.experimental]\nhooks = true\n", encoding="utf-8", newline="\n")


def _dependent(repo: str, name: str, *requires: tuple[str, str, str], tag: str = "1"):
    files = {f"{name}/SKILL.md": f"---\nname: {name}\ndescription: d\n---\n# {name}\n"}
    return make_dependent(f"{repo}/{name}", tag, "skill", files, list(requires))


def _no_lock_shadow(project_dir: Path, unique_repo: str) -> tuple[str, list[str]]:
    """An edge onto a key the checkout declares."""
    good, evil = f"{unique_repo}/good", f"{unique_repo}/evil"
    _skill(good, "dep")
    wanted = _dependent(good, "wanted", ("skill", "dep", "./dep:1"))
    _publish_hook(evil, name="planted")
    shadow = _dependent(evil, "dep", ("hook", "planted", "./planted:1"))
    _cloned(project_dir, skills={"dep": shadow.fq})
    return "planted", [wanted.fq]


def _older_tag_shadow(project_dir: Path, unique_repo: str) -> tuple[str, list[str]]:
    """The same, with the declaration in the repository the edge names."""
    good = f"{unique_repo}/good"
    _publish_hook(good, name="retired")
    older = _dependent(good, "dep", ("hook", "retired", "./retired:1"), tag="0")
    _skill(good, "dep")
    wanted = _dependent(good, "wanted", ("skill", "dep", "./dep:1"))
    _cloned(project_dir, skills={"dep": older.fq})
    return "retired", [wanted.fq]


def _path_shadow(project_dir: Path, unique_repo: str) -> tuple[str, list[str]]:
    """The same, with a path source declared."""
    good = f"{unique_repo}/good"
    _skill(good, "dep")
    wanted = _dependent(good, "wanted", ("skill", "dep", "./dep:1"))
    planted = _publish_hook(f"{unique_repo}/evil", name="planted")
    local = project_dir / "dep"
    local.mkdir()
    (local / "SKILL.md").write_text(
        f"---\nname: dep\ndescription: d\nmetadata:\n  grim.requires.hooks.planted: {planted.fq}\n---\n# dep\n", encoding="utf-8", newline="\n"
    )
    _cloned(project_dir, skills={"dep": "./dep"})
    return "planted", [wanted.fq]


def _nested_shadow(project_dir: Path, unique_repo: str) -> tuple[str, list[str]]:
    """A nested binding the checkout declares."""
    good, evil = f"{unique_repo}/good", f"{unique_repo}/evil"
    leaf = _skill(good, "leaf")
    inner = make_bundle(f"{good}/inner", [("skill", "leaf", leaf.fq)], tag="1")
    outer = make_bundle(f"{good}/outer", [("bundle", "inner", inner.fq)], tag="1")
    planted = _publish_hook(evil, name="planted")
    shadow = make_bundle(f"{evil}/inner", [("hook", "planted", planted.fq)], tag="1")
    _cloned(project_dir, bundles={"inner": shadow.fq})
    return "planted", ["--kind", "bundle", outer.fq]


def _diamond(project_dir: Path, unique_repo: str) -> tuple[str, list[str]]:
    """A key settled at a repository the edge does not name."""
    good, evil = f"{unique_repo}/good", f"{unique_repo}/evil"
    _publish_hook(good, name="guard")
    legit = _dependent(good, "dep", ("hook", "guard", "./guard:1"))
    wanted = _dependent(good, "wanted", ("skill", "dep", "./dep:1"))
    copy = _dependent(evil, "dep", ("hook", "guard", "./guard:1"))
    assert copy.digest == legit.digest
    _publish_hook(evil, name="guard", payload=GUARD_SH + "# planted\n")
    pack = make_bundle(f"{evil}/pack", [("skill", "dep", copy.fq)], tag="1")
    _cloned(project_dir, bundles={"pack": pack.fq})
    return "guard", [wanted.fq]


@pytest.mark.parametrize(
    "shape",
    [_no_lock_shadow, _older_tag_shadow, _path_shadow, _nested_shadow, _diamond],
    ids=["no-lock", "older-tag", "path", "nested", "diamond"],
)
def test_add_consents_no_hook_behind_an_edge_it_did_not_resolve(
    grim_at, project_dir: Path, unique_repo: str, shape
) -> None:
    """T3, CWE-863: a typed reference vouches only for what its own edges resolved
    to. The honest-lock control is `test_add_does_not_consent_preexisting_hook`."""
    hook, add = shape(project_dir, unique_repo)
    runner = grim_at(project_dir)

    runner.run("add", *add)

    assert f'name = "{hook}"' in (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()

    runner.run("hook", "allow")
    runner.run("install")
    assert _armed_in(runner, project_dir) == {hook}, "the record is the one thing in the way"


def test_add_consents_no_hook_row_a_lock_carried(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """T3, CWE-345: an edge vouches for no bytes a lock row chose. `grim hook allow`
    still answers it."""
    good = f"{unique_repo}/good"
    held = _publish_hook(good, name="held")
    wanted = _dependent(good, "wanted", ("hook", "held", "./held:1"))
    _cloned(project_dir)
    (project_dir / "grimoire.lock").write_text(
        _LOCK_HEADER + f'''
[[hook]]
name = "held"
pinned = "{held.pinned}"
''', encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)

    runner.run("add", wanted.fq)

    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()

    runner.run("hook", "allow")
    runner.run("install")
    assert _armed_in(runner, project_dir) == {"held"}


def test_add_of_a_carried_bundle_consents_to_no_member(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """T3, CWE-345: a kept pin answers for the previous lock, so re-adding the
    reference consents to nothing; `grim hook allow` is the route, as after a
    failed add."""
    repo = f"{unique_repo}/r"
    retired = _publish_hook(repo, name="retired")
    kept = _skill(repo, "kept")
    old = make_bundle(f"{repo}/pack", [("hook", "retired", retired.fq), ("skill", "kept", kept.fq)], tag="0")
    new = make_bundle(f"{repo}/pack", [("skill", "kept", kept.fq)], tag="1")
    assert old.digest != new.digest
    reference = f"{REGISTRY_HOST}/{repo}/pack:1"
    _cloned(project_dir, bundles={"pack": reference})
    lock = project_dir / "grimoire.lock"
    lock.write_text(
        _LOCK_HEADER + f'''
[[skill]]
name = "kept"
pinned = "{kept.pinned}"

[[skill.bundles]]
repo = "{REGISTRY_HOST}/{repo}/pack"
tag = "1"

[[bundle]]
name = "pack"
repo = "{REGISTRY_HOST}/{repo}/pack"
tag = "1"
pinned = "{old.pinned}"

[[bundle.member]]
kind = "skill"
name = "kept"
id = "{kept.fq}"
''', encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)

    runner.run("add", "--kind", "bundle", reference)

    assert 'name = "retired"' in lock.read_text(encoding="utf-8")
    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()

    runner.run("hook", "allow")
    runner.run("install")
    assert _armed_in(runner, project_dir) == {"retired"}


@pytest.mark.parametrize("flags", [[], ["--offline"]], ids=["online", "offline"])
def test_a_carried_hook_target_consents_only_to_a_pin_its_repository_serves(
    grim_at, tmp_path: Path, project_dir: Path, unique_repo: str, flags: list[str]
) -> None:
    """T3, CWE-345: a kept pin resolves no tag on relock, so a carried target
    consents only to a pin its repository confirms. Offline no answer exists, so
    nothing consents. `grim update` makes it a served pin, and the same gesture
    then consents."""
    good, evil = f"{unique_repo}/good", f"{unique_repo}/evil"
    _publish_hook(good, name="h", payload=GUARD_SH + "# good\n")
    theirs = _publish_hook(evil, name="h", payload=GUARD_SH + "# evil\n")
    other = tmp_path / "other"
    (other / ".claude").mkdir(parents=True)
    write_config(other, hooks={"h": theirs.fq})
    elsewhere = grim_at(other)
    _enabled(elsewhere)
    elsewhere.run("lock")
    elsewhere.run("hook", "allow")
    elsewhere.run("install")
    reference = f"{REGISTRY_HOST}/{good}/h:1"
    _cloned(project_dir, hooks={"h": reference})
    (project_dir / "grimoire.lock").write_text(
        _LOCK_HEADER + f'''
[[hook]]
name = "h"
pinned = "{reference}@{theirs.digest}"
''', encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)

    result = runner.run("add", *flags, "--kind", "hook", reference)

    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()
    if flags:
        assert "did not confirm" in result.stderr and "rerun online" in result.stderr, result.stderr
    else:
        assert "its tag now names another digest" in result.stderr, result.stderr

    runner.run("update", "h")
    runner.run("add", "--kind", "hook", reference)
    assert _consented(runner, project_dir) == {f"h@{REGISTRY_HOST}/{good}/h"}
    assert _armed_in(runner, project_dir) == {"h"}


def test_a_carried_hook_target_names_a_digest_its_repository_dropped(
    grim_at, tmp_path: Path, project_dir: Path, unique_repo: str
) -> None:
    """CWE-345: online, a kept digest the repository no longer serves is named as
    such, never as a run to repeat online."""
    good, evil = f"{unique_repo}/good", f"{unique_repo}/evil"
    _publish_hook(good, name="h", payload=GUARD_SH + "# good\n")
    theirs = _publish_hook(evil, name="h", payload=GUARD_SH + "# evil\n")
    other = tmp_path / "other"
    (other / ".claude").mkdir(parents=True)
    write_config(other, hooks={"h": theirs.fq})
    elsewhere = grim_at(other)
    _enabled(elsewhere)
    elsewhere.run("lock")
    elsewhere.run("hook", "allow")
    elsewhere.run("install")
    reference = f"{REGISTRY_HOST}/{good}/h@{theirs.digest}"
    _cloned(project_dir, hooks={"h": reference})
    (project_dir / "grimoire.lock").write_text(_LOCK_HEADER + f'\n[[hook]]\nname = "h"\npinned = "{reference}"\n', encoding="utf-8", newline="\n")
    runner = grim_at(project_dir)

    result = runner.run("add", "--kind", "hook", reference)

    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()
    warnings = [line for line in result.stderr.splitlines() if "stays unconsented" in line]
    assert len(warnings) == 1, result.stderr
    assert "no longer serves the digest" in warnings[0] and "`grim update -- h`" in warnings[0], warnings[0]
    assert "rerun online" not in warnings[0], warnings[0]


def test_a_carried_hook_target_consents_only_while_its_tag_names_the_pin(
    grim_at, project_dir: Path, unique_repo: str
) -> None:
    """T3, CWE-345: a kept pin its repository still serves is not what the typed tag
    names once the tag moved, so the add consents to nothing and names the way out.
    After `grim update` the same gesture consents to what the tag names."""
    good = f"{unique_repo}/good"
    old = _publish_hook(good, name="h", payload=GUARD_SH + "# old\n")
    retag(f"{good}/h", "keep", old.digest)
    reference = f"{REGISTRY_HOST}/{good}/h:1"
    _cloned(project_dir, hooks={"h": reference})
    (project_dir / "grimoire.lock").write_text(
        _LOCK_HEADER + f'''
[[hook]]
name = "h"
pinned = "{reference}@{old.digest}"
''', encoding="utf-8", newline="\n"
    )
    new = _publish_hook(good, name="h", payload=GUARD_SH + "# new\n")
    assert new.digest != old.digest
    runner = grim_at(project_dir)

    result = runner.run("add", "--kind", "hook", reference)

    assert _consented(runner, project_dir) == set()
    assert _armed_in(runner, project_dir) == set()
    warnings = [line for line in result.stderr.splitlines() if "stays unconsented" in line]
    assert len(warnings) == 1, result.stderr
    assert "`grim update -- h`" in warnings[0] and "`grim hook allow`" in warnings[0], warnings[0]

    runner.run("update", "h")
    runner.run("add", "--kind", "hook", reference)
    assert _consented(runner, project_dir) == {f"h@{REGISTRY_HOST}/{good}/h"}
    assert _armed_in(runner, project_dir) == {"h"}
