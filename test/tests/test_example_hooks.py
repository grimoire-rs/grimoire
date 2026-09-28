# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""The first-party example hooks in ``catalog/hooks/``, actually fired.

These are published packages, not sample text, so the bar is that each one
**runs and has its documented effect** — through the real binary, from the
real catalog directory, with no fixture manifest anywhere in this file. What
is released here is byte-for-byte what `task catalog:verify` builds and
`grim publish` ships.

Every assertion is on a *side effect*, never on an exit code alone: the
dispatcher exits 0 on every refusal path by design (invariant I3), so a green
exit proves nothing about whether a payload ran. The observer is judged by the
line it appends; the gatekeeper by the verdict document that reaches the
client.

Covered:

* `tool-call-logger` — the `observer` example, at both of its moments
  (`PreToolUse` and `SessionStart`), writing to `$GRIM_EXAMPLE_LOG`.
* `command-guard` — the `gatekeeper` example, denying and *not* denying, with
  both legs in one test so neither is vacuous.
* The catalog wiring: an example directory that is not published is an example
  nobody can install.

The whole file is arm-then-disarm: `test_uninstalling_the_examples_disarms_them`
is the executed half of the walkthrough's teardown section, which is the part a
reader is most likely to skip.
"""
from __future__ import annotations

import json
import os
import tomllib
from pathlib import Path

import pytest

from src.helpers import write_config
from src.runner import GrimRunner

# POSIX-only, matching the payloads themselves: both are `sh` scripts, which
# is the v1 launcher scope.
pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the example payloads are POSIX shell scripts (v1 launcher scope)"
)

# The repository root, from `test/tests/<this file>`. The examples are read
# from the real catalog rather than reconstructed, which is the point: a test
# against a copied fixture would stay green while the shipped package rotted.
REPO_ROOT = Path(__file__).resolve().parents[2]
CATALOG = REPO_ROOT / "catalog"
HOOKS_DIR = CATALOG / "hooks"

LOGGER = "tool-call-logger"
GUARD = "command-guard"

# A Claude-shaped `PreToolUse` payload — the wire format the registered
# command receives.
def _pre_tool_use(command: str) -> str:
    return json.dumps(
        {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "cwd": "/repo",
            "session_id": "example-1",
        }
    )


SESSION_START = json.dumps(
    {"hook_event_name": "SessionStart", "cwd": "/repo", "session_id": "example-1"}
)

# The literal `command-guard` refuses. Duplicated from the payload script
# deliberately: if the example's policy changes, this test should fail rather
# than follow along silently.
DESTRUCTIVE = "rm -rf / --no-preserve-root"


# ---------------------------------------------------------------------------
# Fixture helpers
# ---------------------------------------------------------------------------


def _release(runner: GrimRunner, registry: str, unique_repo: str, name: str) -> str:
    """Push one example straight out of `catalog/hooks/`, and return its ref."""
    source = HOOKS_DIR / name
    assert (source / "hook.toml").is_file(), f"no example package at {source}"
    ref = f"{registry}/{unique_repo}/hooks/{name}:0"
    runner.run("release", str(source), ref)
    return ref


def _arm(grim_at, project_dir: Path, registry: str, unique_repo: str, log: Path) -> GrimRunner:
    """Release both examples, declare them, and arm them for claude.

    `--trust-hooks` rather than a persisted grant: pytest gives the child no
    TTY, which is exactly the non-interactive condition under which grim
    refuses to arm without it. That is also the shape CI has.
    """
    runner = grim_at(project_dir)
    # Where the observer writes. Set on grim's own environment because the
    # payload inherits it through `grim hook run`; without it the example
    # falls back to a shared path under $TMPDIR, which tests must not touch.
    runner.env["GRIM_EXAMPLE_LOG"] = str(log)
    (project_dir / ".claude").mkdir(exist_ok=True)

    refs = {name: _release(runner, registry, unique_repo, name) for name in (LOGGER, GUARD)}
    write_config(project_dir, hooks=refs)
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--trust-hooks")
    return runner


def _table(runner: GrimRunner) -> Path:
    return Path(runner.grim_home) / "hooks" / "dispatch.json"


def _rows(runner: GrimRunner) -> list[dict]:
    """Every dispatch row, across every root."""
    table = _table(runner)
    if not table.is_file():
        return []
    document = json.loads(table.read_text())
    return [row for root in document["roots"].values() for row in root["hooks"]]


def _root_token(runner: GrimRunner) -> str:
    (token,) = json.loads(_table(runner).read_text())["roots"].keys()
    return token


def _fire(runner: GrimRunner, event: str, payload: str, *, root: str | None = None):
    """Invoke the dispatcher exactly as the generated launcher does.

    `root` is explicit for the disarm test, which fires a token captured
    *before* the uninstall — a client whose registration grim has not rewritten
    yet still names the old one, and that invocation must run nothing.
    """
    return runner.run(
        "hook",
        "run",
        "--client",
        "claude",
        "--event",
        event,
        "--table",
        str(_table(runner)),
        "--root",
        root or _root_token(runner),
        stdin=payload,
        check=False,
    )


def _log_lines(log: Path) -> list[str]:
    return log.read_text().splitlines() if log.is_file() else []


# ---------------------------------------------------------------------------
# tool-call-logger — the observer example
# ---------------------------------------------------------------------------


def test_the_observer_example_logs_a_tool_call(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """`tool-call-logger` appends the line its own description promises.

    The line's *content* is asserted, not merely its existence: the fields it
    names (`GRIM_HOOK_EVENT`, `_CLIENT`, `_TOOL`, `_NAME`, `_TIER`) are grim's
    exported environment allowlist, and the example exists to demonstrate that
    a payload can get its bearings from flat scalars without parsing anything.
    """
    log = tmp_path / "hooks.log"
    runner = _arm(grim_at, project_dir, registry, unique_repo, log)

    result = _fire(runner, "PreToolUse", _pre_tool_use("ls -la"))

    assert result.returncode == 0, result.stderr
    lines = _log_lines(log)
    assert lines, f"the observer never ran — nothing at {log}\nstderr: {result.stderr}"
    assert lines[-1] == (
        "PreToolUse client=claude tool=Bash "
        f"hook={LOGGER}/log-tool-call tier=observer"
    ), lines


def test_the_observer_example_logs_a_session_start(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """The second entry, sharing the first one's payload tree.

    Two `[[hooks]]` entries bound to different moments through one script is
    the arrangement the array-of-tables exists for, and `SessionStart` carries
    no tool — so the example's `${GRIM_HOOK_TOOL:-none}` fallback is load
    bearing rather than defensive decoration.
    """
    log = tmp_path / "hooks.log"
    runner = _arm(grim_at, project_dir, registry, unique_repo, log)

    result = _fire(runner, "SessionStart", SESSION_START)

    assert result.returncode == 0, result.stderr
    lines = _log_lines(log)
    assert lines, f"the SessionStart entry never ran\nstderr: {result.stderr}"
    assert lines[-1] == (
        f"SessionStart client=claude tool=none hook={LOGGER}/log-session-start tier=observer"
    ), lines


def test_the_observer_never_denies_anything(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """The observer tier's promise, asserted rather than described.

    Its description tells a reader it "cannot block a tool call". The payload
    answers `{}`, so nothing it returns can become a verdict — and firing it
    with the very command the *gatekeeper* refuses is the sharpest way to show
    the difference is the tier and not the input.
    """
    log = tmp_path / "hooks.log"
    runner = grim_at(project_dir)
    runner.env["GRIM_EXAMPLE_LOG"] = str(log)
    (project_dir / ".claude").mkdir(exist_ok=True)
    # The logger ALONE this time: with `command-guard` armed the denial would
    # be its work, and this test would prove nothing about the observer.
    ref = _release(runner, registry, unique_repo, LOGGER)
    write_config(project_dir, hooks={LOGGER: ref})
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("lock")
    runner.run("install", "--trust-hooks")

    result = _fire(runner, "PreToolUse", _pre_tool_use(DESTRUCTIVE))

    assert result.returncode == 0, result.stderr
    assert _log_lines(log), (
        f"POSITIVE CONTROL FAILED: the observer did not run at all, so the negative below is "
        f"vacuous\nstderr: {result.stderr}"
    )
    assert "deny" not in result.stdout, (
        f"an observer produced a verdict; its response must be discarded: {result.stdout!r}"
    )


# ---------------------------------------------------------------------------
# command-guard — the gatekeeper example
# ---------------------------------------------------------------------------


def test_the_gatekeeper_example_denies_and_the_verdict_reaches_the_client(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """Both legs, one test: the refusal, and the command that must not be refused.

    The denial is asserted in **Claude's own response shape**, because that
    projection is the whole reason a payload writes grim's small canonical
    vocabulary instead of a vendor's: the example's `{"decision":"deny"}` has to
    arrive as `hookSpecificOutput.permissionDecision`.

    Exit 0 is asserted alongside it. A verdict travelling as an exit code would
    be the fail-closed bug the projection exists to prevent — and `2` is
    Claude's own deny code, so the two channels must never be shared.
    """
    log = tmp_path / "hooks.log"
    runner = _arm(grim_at, project_dir, registry, unique_repo, log)

    allowed = _fire(runner, "PreToolUse", _pre_tool_use("ls -la"))
    assert allowed.returncode == 0, allowed.stderr
    assert "permissionDecision" not in allowed.stdout, (
        f"a harmless command drew a verdict: {allowed.stdout!r}"
    )

    refused = _fire(runner, "PreToolUse", _pre_tool_use(DESTRUCTIVE))
    assert refused.returncode == 0, (
        f"the verdict travelled as an exit code ({refused.returncode}), not as a document"
    )
    document = json.loads(refused.stdout)
    output = document["hookSpecificOutput"]
    assert output["permissionDecision"] == "deny", document
    assert output["hookEventName"] == "PreToolUse", document
    assert "rm -rf /" in output["permissionDecisionReason"], (
        f"the reason must say what was refused: {output}"
    )


def test_the_gatekeeper_example_leaves_an_audit_record(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """A refusal is recorded, keyed to the digest that was armed.

    The example's own answer is only half of what a reader needs to trust it;
    the other half is that grim wrote down what ran. The digest asserted here
    is the *pinned* one from the dispatch row, so this also shows the trail
    joins back to the artifact a user approved.
    """
    log = tmp_path / "hooks.log"
    runner = _arm(grim_at, project_dir, registry, unique_repo, log)
    (guard_row,) = [r for r in _rows(runner) if r["artifact"] == GUARD]

    _fire(runner, "PreToolUse", _pre_tool_use(DESTRUCTIVE))

    trail = Path(runner.grim_home) / "hooks" / "hook_audit.jsonl"
    assert trail.is_file(), f"no audit trail at {trail}"
    records = [json.loads(line) for line in trail.read_text().splitlines() if line.strip()]
    denials = [r for r in records if r["hook_id"] == "refuse-recursive-root-delete"]
    assert denials, f"the gatekeeper's invocation was not recorded: {records}"
    assert denials[-1]["verdict"] == "deny", denials[-1]
    assert denials[-1]["tier"] == "gatekeeper", denials[-1]
    assert denials[-1]["digest"] == guard_row["resolved_digest"], (
        "the record must name the digest that was armed, or it joins to nothing"
    )


# ---------------------------------------------------------------------------
# The gate, and the way back out
# ---------------------------------------------------------------------------


def test_the_examples_do_not_arm_on_install(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """Installing a published example arms **nothing** until the user says so.

    Both descriptions promise this in as many words, and it is the claim most
    likely to become false by accident. The positive control in the same
    function is what stops this passing against a build that cannot arm.
    """
    runner = grim_at(project_dir)
    runner.env["GRIM_EXAMPLE_LOG"] = str(tmp_path / "hooks.log")
    (project_dir / ".claude").mkdir(exist_ok=True)
    refs = {name: _release(runner, registry, unique_repo, name) for name in (LOGGER, GUARD)}
    write_config(project_dir, hooks=refs)
    runner.run("lock")

    result = runner.run("install")
    assert result.returncode == 0, result.stderr
    assert "options.experimental.hooks" in result.stderr, (
        f"the skip must name the flag that would enable it: {result.stderr}"
    )
    assert _rows(runner) == [], "a published example armed itself on install"

    status = {item["name"]: item for item in runner.json("status")["items"]}
    for name in (LOGGER, GUARD):
        assert status[name]["state"] == "gated", status[name]
        assert [a["cause"] for a in status[name]["arming"]] == ["feature-flag-off"], status[name]

    # Positive control: the same artifacts, once the user opts in.
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("install", "--trust-hooks")
    assert {r["artifact"] for r in _rows(runner)} == {LOGGER, GUARD}


def test_uninstalling_the_examples_disarms_them(
    grim_at, project_dir: Path, tmp_path: Path, registry: str, unique_repo: str
) -> None:
    """The walkthrough's teardown, executed.

    A guide that arms something and never disarms it is a trap, so the reverse
    is a tested property rather than a paragraph: after `grim uninstall`, the
    dispatch row, the client registration and the payload are all gone — and
    firing the dispatcher again runs nothing.
    """
    log = tmp_path / "hooks.log"
    runner = _arm(grim_at, project_dir, registry, unique_repo, log)
    armed_token = _root_token(runner)
    _fire(runner, "PreToolUse", _pre_tool_use("ls -la"))
    assert _log_lines(log), "positive control: the examples really were armed"

    for name in (LOGGER, GUARD):
        runner.run("uninstall", "hook", name)

    assert _rows(runner) == [], "the dispatch rows survived uninstall"
    settings = project_dir / ".claude" / "settings.local.json"
    registrations = json.loads(settings.read_text()).get("hooks", {}) if settings.is_file() else {}
    assert registrations == {}, f"a client registration survived uninstall: {registrations}"
    payload_root = Path(runner.grim_home) / "hooks" / "payload"
    survivors = [p for p in payload_root.rglob("hook.toml")] if payload_root.is_dir() else []
    assert survivors == [], f"a payload survived uninstall: {survivors}"

    # And the dispatcher now fires nothing, even for the token that was armed a
    # moment ago: there are no rows left to select.
    before = len(_log_lines(log))
    result = _fire(runner, "PreToolUse", _pre_tool_use("ls -la"), root=armed_token)
    assert result.returncode == 0, result.stderr
    assert len(_log_lines(log)) == before, "a disarmed example still ran"


# ---------------------------------------------------------------------------
# Catalog wiring — an unpublished example is an example nobody can install
# ---------------------------------------------------------------------------


def test_every_example_hook_is_wired_into_the_catalog() -> None:
    """Each directory under `catalog/hooks/` is publishable and described.

    `task catalog:verify` proves each package *builds*; nothing else proves it
    is reachable. A new example added without its `publish.toml` entry would
    build cleanly in CI forever and never ship — and one without a description
    companion falls back to the conventional probe, which would publish the
    maintainer README to the package repository.
    """
    manifest = tomllib.loads((CATALOG / "publish.toml").read_text())
    declared = set(manifest.get("hooks", {}))
    on_disk = {d.name for d in HOOKS_DIR.iterdir() if (d / "hook.toml").is_file()}

    assert on_disk, f"no example packages under {HOOKS_DIR}"
    assert on_disk == declared, (
        f"catalog/hooks and publish.toml disagree: only on disk {on_disk - declared}, "
        f"only declared {declared - on_disk}"
    )

    for name in sorted(declared):
        entry = manifest["hooks"][name]
        assert entry["repository"].endswith(f"/hooks/{name}"), entry
        readme = CATALOG / entry["description"]["readme"]
        assert readme.is_file(), f"{name} declares a missing description companion: {readme}"


def test_every_example_declares_its_tier_in_its_description() -> None:
    """The published `description` must say what the hook does at runtime.

    It is the one line a consumer sees in `grim search` before installing, and
    for a hook it is a security disclosure: the tier decides whether the thing
    can read, refuse, or rewrite. The gatekeeper is held to more — its
    description has to disown the security-control reading outright, because
    "gatekeeper" invites exactly that reading.
    """
    tiers = {LOGGER: "observer", GUARD: "gatekeeper"}
    for name, tier in tiers.items():
        manifest = tomllib.loads((HOOKS_DIR / name / "hook.toml").read_text())
        description = manifest["description"]
        assert tier in description.lower(), f"{name} does not name its tier: {description}"
        assert {entry["tier"] for entry in manifest["hooks"]} == {tier}, manifest["hooks"]

    guard = tomllib.loads((HOOKS_DIR / GUARD / "hook.toml").read_text())["description"]
    assert "NOT a security control" in guard, (
        f"the gatekeeper example must disown the security-control reading: {guard}"
    )
