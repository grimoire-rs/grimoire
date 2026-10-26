# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The native hook launcher, end to end: ``grim install`` writes it, a client's own
registration fires it, and what comes back is the exit table a client reads.

The launcher is the real ``$GRIM_HOME/hooks/grim-hook`` (``src/hook_launcher.py``);
a test that needs its own table overwrites the ``dispatch.json`` beside it.  Shell
form registrations (Claude, Codex) run through ``sh -c`` exactly as the client does,
the exec form (Copilot) as ``exec`` plus ``args``.

Exit table: 0 success or nothing selected, 1 internal failure, 64 usage, 65 data,
74 I/O, 79 missing table; never 2, which is Claude's deny code.  Exit 1 is not
reachable from outside the process (the dispatcher refuses the input that would
cause it first), so it is covered by the core and launcher Rust tests; this file
only proves nothing here ever exits 2 or forges a 0.

Every "nothing ran" assertion carries a positive control in the same test: a
launcher that spawns nothing at all satisfies every bare negative.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from src.hook_launcher import publish_hook, run_launcher
from src.stdio_drivers import HOSTILE_PAYLOAD
from src.stdio_drivers import entry as _entry
from src.stdio_drivers import payload as _payload
from src.stdio_drivers import write_table as _write_table

pytestmark = pytest.mark.skipif(
    os.name == "nt",
    reason="the payload fixtures are POSIX shell scripts (v1 launcher scope)",
)

MARKER_KEY = "com.grimoire.managed"
MARKER_VALUE = "hook-dispatcher"
OTHER_ROOT = "fedcba9876543210fedcba9876543210"
USAGE = "grim-hook: usage: grim-hook <client> <event> <root>"
MAX_STDIN = 8 * 1024 * 1024
MAX_TABLE_BYTES = 1 << 20
SANE_ENV = {"PATH": os.environ.get("PATH", "")}


@pytest.fixture()
def token(launcher: Path) -> str:
    """The one root token the arming install minted, read before a test replaces the table."""
    (root,) = json.loads((launcher.parent / "dispatch.json").read_text())["roots"]
    return root


def _fire(launcher: Path, token: str, **kwargs) -> subprocess.CompletedProcess[str]:
    kwargs.setdefault("stdin", HOSTILE_PAYLOAD)
    return run_launcher(launcher, "claude", "PreToolUse", token, **kwargs)


def _assert_failure(result: subprocess.CompletedProcess[str], code: int) -> None:
    """The failure contract: this exit, never 2, nothing on stdout, one explaining line."""
    assert result.returncode == code, f"rc={result.returncode}\n{result.stderr}"
    assert result.returncode != 2, "2 is Claude's deny code; grim never emits it"
    assert result.stdout == "", f"a failing launcher wrote to stdout: {result.stdout!r}"
    lines = [line for line in result.stderr.splitlines() if line.strip()]
    assert lines, "a failure must say why on stderr"
    assert "panicked" not in result.stderr, result.stderr
    if code == 64:
        assert lines[-1] == USAGE, lines
    else:
        assert lines[-1].startswith("grim-hook: error: "), lines
        assert sum(line.startswith("grim-hook: error: ") for line in lines) == 1, lines


def _claude_command(project_dir: Path) -> str:
    settings = json.loads((project_dir / ".claude" / "settings.local.json").read_text())
    elements = [
        e
        for groups in settings["hooks"].values()
        for g in groups
        for e in g["hooks"]
        if e.get(MARKER_KEY) == MARKER_VALUE
    ]
    (element,) = elements
    return element["command"]


def _run_shell_form(
    command: str, *, stdin: str = HOSTILE_PAYLOAD, cwd: Path | None = None
):
    return subprocess.run(
        ["/bin/sh", "-c", command],
        input=stdin,
        capture_output=True,
        text=True,
        env=SANE_ENV,
        cwd=str(cwd) if cwd else None,
        check=False,
    )


# ---------------------------------------------------------------------------
# S-005 — a client fires an armed event through its real registration
# ---------------------------------------------------------------------------


def test_s005_the_claude_registration_fires_the_launcher(
    launcher: Path, token: str, project_dir: Path, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "hit")
    _write_table(
        grim_home,
        [_entry(id="hit", handler=handler, payload_dir=payload_dir)],
        root=token,
    )

    result = _run_shell_form(_claude_command(project_dir), cwd=project_dir)

    assert result.returncode == 0, result.stderr
    assert marker.exists(), (
        f"the registered command never reached the payload\n{result.stderr}"
    )
    envelope = json.loads(marker.read_text())
    assert envelope["client"] == "claude" and envelope["event"] == "PreToolUse", (
        envelope
    )
    assert HOSTILE_PAYLOAD in marker.read_text(), (
        "the client's bytes must reach `raw` verbatim"
    )


def test_s005_a_gatekeeper_deny_travels_as_json_with_exit_zero(
    launcher: Path, token: str, project_dir: Path, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(
        tmp_path, "deny", response='{"decision":"deny","reason":"piping curl into sh"}'
    )
    _write_table(
        grim_home,
        [
            _entry(
                id="deny", handler=handler, payload_dir=payload_dir, tier="gatekeeper"
            )
        ],
        root=token,
    )

    result = _run_shell_form(_claude_command(project_dir), cwd=project_dir)

    assert marker.exists(), f"the gatekeeper never ran\n{result.stderr}"
    assert result.returncode == 0, "a verdict must never travel as an exit code"
    out = json.loads(result.stdout)["hookSpecificOutput"]
    assert (
        out["permissionDecision"]
        and out["permissionDecisionReason"] == "piping curl into sh"
    ), out


def test_s005_codex_shell_form_and_copilot_exec_form_fire_the_same_launcher(
    grim, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    home = Path(grim.home)
    for marker_dir in (".claude", ".codex", ".copilot/skills"):
        (home / marker_dir).mkdir(parents=True, exist_ok=True)
    hook = publish_hook(unique_repo)
    grim_home.mkdir(parents=True, exist_ok=True)
    (grim_home / "grimoire.toml").write_text(
        f'[hooks]\nshell-guard = "{hook.fq}"\n\n[options.experimental]\nhooks = true\n'
    )
    grim.run("lock", "--global")
    grim.run("install", "--global", "--trust-hooks")
    launcher = grim_home / "hooks" / "grim-hook"
    (token,) = json.loads((grim_home / "hooks" / "dispatch.json").read_text())["roots"]

    codex = json.loads((home / ".codex" / "hooks.json").read_text())["hooks"][
        "PreToolUse"
    ]
    codex_command = codex[0]["hooks"][0]["command"]
    copilot = json.loads((home / ".copilot" / "hooks" / "grim.json").read_text())[
        "hooks"
    ]["PreToolUse"][0]
    assert copilot["exec"] == str(launcher) and copilot["args"] == [
        "copilot",
        "PreToolUse",
        token,
    ], copilot

    markers = {}
    entries = []
    for client in ("codex", "copilot"):
        handler, markers[client], payload_dir = _payload(tmp_path, client)
        entries.append(
            _entry(id=client, handler=handler, payload_dir=payload_dir, client=client)
        )
    _write_table(grim_home, entries, root=token)

    shell = _run_shell_form(codex_command)
    exec_form = subprocess.run(
        [copilot["exec"], *copilot["args"]],
        input=HOSTILE_PAYLOAD,
        capture_output=True,
        text=True,
        env=SANE_ENV,
        check=False,
    )

    assert shell.returncode == 0, shell.stderr
    assert exec_form.returncode == 0, exec_form.stderr
    for client, marker in markers.items():
        assert marker.exists(), f"the {client} registration never reached its payload"
        assert json.loads(marker.read_text())["client"] == client


# ---------------------------------------------------------------------------
# S-006 — the launcher is deleted while still registered
# ---------------------------------------------------------------------------


def test_s006_a_deleted_launcher_fails_visibly_and_install_repairs_it(
    launcher: Path,
    token: str,
    project_dir: Path,
    grim_at,
    grim_home: Path,
    tmp_path: Path,
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "repair")
    _write_table(
        grim_home,
        [_entry(id="repair", handler=handler, payload_dir=payload_dir)],
        root=token,
    )
    command = _claude_command(project_dir)
    control = _run_shell_form(command, cwd=project_dir)
    assert control.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{control.stderr}"
    )
    marker.unlink()

    launcher.unlink()
    gone = _run_shell_form(command, cwd=project_dir)
    assert gone.returncode == 127, (
        f"a missing launcher is the shell's not-found, not a forged 0: {gone.returncode}"
    )
    assert gone.stdout == "" and not marker.exists()

    runner = grim_at(project_dir)
    assert runner.json("hook", "list")["launcher"]["state"] == "missing"

    runner.run("install", "--trust-hooks")
    assert launcher.is_file() and os.access(launcher, os.X_OK), (
        "install must rewrite the launcher"
    )
    assert runner.json("hook", "list")["launcher"]["state"] != "missing"
    (token_after,) = json.loads((launcher.parent / "dispatch.json").read_text())[
        "roots"
    ]
    _write_table(
        grim_home,
        [_entry(id="repair", handler=handler, payload_dir=payload_dir)],
        root=token_after,
    )
    repaired = _run_shell_form(_claude_command(project_dir), cwd=project_dir)
    assert repaired.returncode == 0 and marker.exists(), (
        f"the repaired launcher does not fire\n{repaired.stderr}"
    )


# ---------------------------------------------------------------------------
# C-007 exit table, from the outside
# ---------------------------------------------------------------------------


def test_exit_0_nothing_selected_is_silent_and_spawns_nothing(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "quiet")
    _write_table(
        grim_home,
        [_entry(id="quiet", handler=handler, payload_dir=payload_dir)],
        root=token,
    )

    # A well-formed root no table holds (the forged-registration case), an event
    # with no rows, an event this grim does not know, a client with no rows.
    for operands in (
        ("claude", "PreToolUse", OTHER_ROOT),
        ("claude", "Stop", token),
        ("claude", "PermissionRequest", token),
        ("warp", "PreToolUse", token),
    ):
        result = run_launcher(launcher, *operands, stdin=HOSTILE_PAYLOAD)
        assert result.returncode == 0, f"{operands}: {result.stderr}"
        assert result.stdout == "", operands
        assert not marker.exists(), f"{operands} fired a hook it does not own"

    control = _fire(launcher, token)
    assert control.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{control.stderr}"
    )


@pytest.mark.parametrize(
    "operands",
    [
        (),
        ("claude",),
        ("claude", "PreToolUse"),
        ("claude", "PreToolUse", "{token}", "extra"),
        ("Claude", "PreToolUse", "{token}"),
        ("", "PreToolUse", "{token}"),
        ("claude", "", "{token}"),
        ("claude", "Pre Tool", "{token}"),
        ("claude", "PreToolUse", ""),
        ("claude", "PreToolUse", "global"),
        ("claude", "PreToolUse", "0123456789ABCDEF0123456789ABCDEF"),
        ("claude", "PreToolUse", "0123456789abcdef"),
        ("--client", "claude", "PreToolUse", "{token}"),
        ("claude", "PreToolUse", "--", "{token}"),
    ],
    ids=lambda operands: "-".join(operands) or "no-operands",
)
def test_exit_64_for_an_argv_that_breaks_the_grammar(
    launcher: Path,
    token: str,
    grim_home: Path,
    tmp_path: Path,
    operands: tuple[str, ...],
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "usage")
    _write_table(
        grim_home,
        [_entry(id="usage", handler=handler, payload_dir=payload_dir)],
        root=token,
    )

    result = run_launcher(
        launcher, *(o.format(token=token) for o in operands), stdin=HOSTILE_PAYLOAD
    )

    _assert_failure(result, 64)
    assert not marker.exists(), "a malformed argv still spawned a payload"
    control = _fire(launcher, token)
    assert control.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{control.stderr}"
    )


def test_exit_79_when_the_table_is_absent(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "absent")
    table = _write_table(
        grim_home,
        [_entry(id="absent", handler=handler, payload_dir=payload_dir)],
        root=token,
    )
    control = _fire(launcher, token)
    assert control.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{control.stderr}"
    )
    marker.unlink()

    table.unlink()
    result = _fire(launcher, token)

    _assert_failure(result, 79)
    assert not marker.exists()


def _write_raw(grim_home: Path, body: str) -> Path:
    table = grim_home / "hooks" / "dispatch.json"
    table.write_text(body)
    return table


@pytest.mark.parametrize(
    "body,why",
    [
        ("{not json at all", "truncated garbage"),
        ("", "an empty file"),
        ("[]", "valid JSON that is not an object"),
        ('{"roots": {}}', "no schema field"),
        ('{"schema": "1", "roots": {}}', "a schema of the wrong type"),
        ('{"schema": 999, "roots": {}}', "a newer schema after a grim downgrade"),
        ('{"schema": 2, "roots": {}}', "the next schema"),
        ('{"schema": 1, "roots": []}', "roots is not an object"),
    ],
)
def test_exit_65_for_a_table_the_launcher_cannot_vouch_for(
    launcher: Path, token: str, grim_home: Path, body: str, why: str
) -> None:
    _write_raw(grim_home, body)

    _assert_failure(_fire(launcher, token), 65)


def test_exit_65_for_an_oversize_table_and_a_selected_row_that_cannot_be_read(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "data")
    good = _entry(id="data", handler=handler, payload_dir=payload_dir)

    oversize = dict(good, padding="x" * (MAX_TABLE_BYTES + 1))
    table = _write_raw(
        grim_home,
        json.dumps(
            {"schema": 1, "roots": {token: {"root": "global", "hooks": [oversize]}}}
        ),
    )
    assert table.stat().st_size > MAX_TABLE_BYTES, "fixture must exceed the cap"
    _assert_failure(_fire(launcher, token), 65)

    relative_dir = _entry(id="data", handler=handler, payload_dir=Path("payloads"))
    long_matcher = _entry(
        id="data", handler=handler, payload_dir=payload_dir, matcher="B" * 257
    )
    for bad in (relative_dir, long_matcher):
        _write_table(grim_home, [bad, good], root=token)
        _assert_failure(_fire(launcher, token), 65)
    assert not marker.exists(), "a row from a table the launcher refused still ran"

    # An unreadable row addressed to another client is not selected, so the good row still runs.
    foreign = dict(relative_dir, client="codex")
    _write_table(grim_home, [foreign, good], root=token)
    control = _fire(launcher, token)
    assert control.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{control.stderr}"
    )


def test_exit_65_for_a_client_payload_over_the_cap_only_when_rows_match(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "cap")
    _write_table(
        grim_home,
        [_entry(id="cap", handler=handler, payload_dir=payload_dir)],
        root=token,
    )
    oversize = "x" * (MAX_STDIN + 1)

    unselected = run_launcher(
        launcher, "claude", "PreToolUse", OTHER_ROOT, stdin=oversize
    )
    assert unselected.returncode == 0 and unselected.stdout == "", unselected.stderr

    selected = _fire(launcher, token, stdin=oversize)
    _assert_failure(selected, 65)
    assert not marker.exists()


def test_exit_74_when_the_table_cannot_be_read(
    launcher: Path, token: str, grim_home: Path
) -> None:
    table = grim_home / "hooks" / "dispatch.json"
    table.unlink()
    table.mkdir()

    _assert_failure(_fire(launcher, token), 74)


@pytest.mark.parametrize("tier,fails", [("observer", False), ("gatekeeper", True)])
def test_an_unwritable_audit_withholds_a_gatekeeper_with_exit_74_and_an_observer_with_0(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path, tier: str, fails: bool
) -> None:
    handler, marker, payload_dir = _payload(
        tmp_path, f"audit-{tier}", response='{"decision":"deny","reason":"blocked"}'
    )
    _write_table(
        grim_home,
        [_entry(id="audit", handler=handler, payload_dir=payload_dir, tier=tier)],
        root=token,
    )
    control = _fire(launcher, token)
    assert control.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{control.stderr}"
    )
    marker.unlink()

    trail = grim_home / "hooks" / "hook_audit.jsonl"
    trail.unlink(missing_ok=True)
    trail.mkdir()
    result = _fire(launcher, token)

    assert not marker.exists(), f"the {tier} ran unlogged"
    if fails:
        _assert_failure(result, 74)
    else:
        assert result.returncode == 0, result.stderr
        assert "deny" not in result.stdout, (
            "an audit failure must not produce a verdict"
        )
        assert result.stderr.strip(), "an unlogged refusal must warn on stderr"


def test_no_invocation_shape_exits_2_or_forges_a_zero(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    """Every degrade shape in one sweep: the exit is in the table, never 2."""
    handler, _, payload_dir = _payload(tmp_path, "sweep")
    _write_table(
        grim_home,
        [_entry(id="sweep", handler=handler, payload_dir=payload_dir)],
        root=token,
    )
    shapes = [
        (("claude", "PreToolUse", token), HOSTILE_PAYLOAD, 0),
        (("claude", "PreToolUse", token), "not json", 0),
        (("claude", "PreToolUse", token), "", 0),
        (("claude", "PreToolUse", "not-a-token"), HOSTILE_PAYLOAD, 64),
        (("claude", "Bogus", token), HOSTILE_PAYLOAD, 0),
        (("warp", "PreToolUse", token), HOSTILE_PAYLOAD, 0),
    ]
    for operands, stdin, expected in shapes:
        result = run_launcher(launcher, *operands, stdin=stdin)
        assert result.returncode == expected, (
            f"{operands} {stdin!r}: rc={result.returncode}\n{result.stderr}"
        )
        assert "panicked" not in result.stderr, result.stderr
        if result.returncode:
            assert result.stdout == "", result.stdout


# ---------------------------------------------------------------------------
# C-014 — the launcher reads its sibling table and writes only beside it
# ---------------------------------------------------------------------------


def test_the_table_is_the_one_beside_the_launcher_whatever_the_cwd_and_environment_say(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    real_handler, real_marker, real_dir = _payload(tmp_path, "real")
    decoy_handler, decoy_marker, decoy_dir = _payload(tmp_path, "decoy")
    _write_table(
        grim_home,
        [_entry(id="real", handler=real_handler, payload_dir=real_dir)],
        root=token,
    )

    # A hostile workspace ships its own dispatch.json, and its environment names a data root.
    workspace = tmp_path / "workspace"
    fake_home = tmp_path / "fake-grim-home"
    for planted in (workspace, fake_home / "hooks"):
        planted.mkdir(parents=True)
        (planted / "dispatch.json").write_text(
            json.dumps(
                {
                    "schema": 1,
                    "roots": {
                        token: {
                            "root": "global",
                            "hooks": [
                                _entry(
                                    id="decoy",
                                    handler=decoy_handler,
                                    payload_dir=decoy_dir,
                                )
                            ],
                        }
                    },
                }
            )
        )

    result = _fire(
        launcher,
        token,
        cwd=workspace,
        env={**SANE_ENV, "GRIM_HOME": str(fake_home), "HOME": str(fake_home)},
    )

    assert result.returncode == 0, result.stderr
    assert real_marker.exists(), (
        f"POSITIVE CONTROL FAILED: the sibling table did not run\n{result.stderr}"
    )
    assert not decoy_marker.exists(), (
        "the launcher read a table that is not the one beside it"
    )


def test_a_copy_of_the_launcher_reads_the_table_beside_the_copy(
    launcher: Path, token: str, grim_home: Path, tmp_path: Path
) -> None:
    here_handler, here_marker, here_dir = _payload(tmp_path, "here")
    there_handler, there_marker, there_dir = _payload(tmp_path, "there")
    _write_table(
        grim_home,
        [_entry(id="here", handler=here_handler, payload_dir=here_dir)],
        root=token,
    )

    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    copy = elsewhere / "grim-hook"
    shutil.copy2(launcher, copy)
    (elsewhere / "dispatch.json").write_text(
        json.dumps(
            {
                "schema": 1,
                "roots": {
                    token: {
                        "root": "global",
                        "hooks": [
                            _entry(
                                id="there", handler=there_handler, payload_dir=there_dir
                            )
                        ],
                    }
                },
            }
        )
    )

    result = _fire(copy, token)

    assert result.returncode == 0, result.stderr
    assert there_marker.exists() and not here_marker.exists(), (
        "the copy must resolve its own directory's table"
    )


def test_the_launcher_writes_only_the_audit_trail(
    launcher: Path, token: str, grim_home: Path, project_dir: Path, tmp_path: Path
) -> None:
    handler, marker, payload_dir = _payload(tmp_path, "writes")
    _write_table(
        grim_home,
        [_entry(id="writes", handler=handler, payload_dir=payload_dir)],
        root=token,
    )
    home = grim_home.parent / "home"
    roots = [grim_home, project_dir, home]

    def snapshot() -> set[tuple[str, int]]:
        return {
            (str(p), p.stat().st_size if p.is_file() else -1)
            for root in roots
            if root.exists()
            for p in root.rglob("*")
        }

    before = snapshot()
    result = _fire(launcher, token, cwd=project_dir)
    after = snapshot()

    assert result.returncode == 0 and marker.exists(), (
        f"POSITIVE CONTROL FAILED\n{result.stderr}"
    )
    touched = {Path(path) for path, _ in after - before}
    assert touched == {grim_home / "hooks" / "hook_audit.jsonl"}, touched
