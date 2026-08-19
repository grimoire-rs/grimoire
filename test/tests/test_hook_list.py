# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim hook list` (S-015) through the real binary — the user-facing inventory.

`test_hook_arming.py` proves the dispatch table and the client registration get
written. This file proves the **verb a human types** reports what got written.

The command shipped as a stub that returned `{"items": []}` unconditionally, on
a REMOVAL TRIGGER comment whose premise ("nothing can install a hook yet")
stopped being true two work packages earlier. That failure mode is the most
dangerous wrong answer this feature can give: an empty inventory reads as "no
hooks are armed", and a user who believes a guardrail is inert acts as if
nothing is watching.

⛔ **Every test here must fail against that stub**, which is why none of them
asserts on an empty scope: `items == []` is what the stub already returns. Each
test either asserts the presence of a real row, or asserts a stderr line the
stub never emits.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

from src.helpers import make_artifact, write_config

# POSIX-only for the same reason `test_hook_arming.py` is: arming a hook writes
# a `sh` launcher and a POSIX one-liner, and every test here arms one.
pytestmark = pytest.mark.skipif(
    os.name == "nt", reason="the hook launcher and its registered command are POSIX-only in v1"
)

# Two entries on purpose: the report contract is one item per `[[hooks]]` entry,
# not one per artifact, and a single-entry fixture cannot tell the two apart.
HOOK_TOML = """\
schema = 1
name = "shell-guard"
description = "Observes and gates Bash tool calls."

[[hooks]]
id = "post"
event = "PostToolUse"
tier = "observer"
command = "sh observe.sh"
timeout = 5

[[hooks]]
id = "pre"
event = "PreToolUse"
tier = "gatekeeper"
matcher = "Bash"
command = "sh guard.sh"
timeout = 5
"""

GUARD_SH = "#!/bin/sh\ncat > /dev/null\necho '{}'\n"


def _publish_hook(unique_repo: str):
    """Push a real two-entry hook artifact; the payload tree is rooted at `<name>/`."""
    return make_artifact(
        f"{unique_repo}/shell-guard",
        "hook",
        {
            "shell-guard/hook.toml": HOOK_TOML,
            "shell-guard/guard.sh": GUARD_SH,
            "shell-guard/observe.sh": GUARD_SH,
        },
        tag="1",
    )


def _declare(project_dir: Path, fq: str) -> None:
    write_config(project_dir, hooks={"shell-guard": fq})


def _rows(runner) -> list[dict]:
    """Every dispatch row across every root — the flat arming truth."""
    path = Path(runner.grim_home) / "hooks" / "dispatch.json"
    if not path.is_file():
        return []
    table = json.loads(path.read_text())
    return [row for root in table["roots"].values() for row in root["hooks"]]


def _arm(runner, project_dir: Path, unique_repo: str):
    """Publish, declare, enable the flag, and arm — the precondition of the inventory."""
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner.run("lock")
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("install", "--allow-hooks")
    # Positive control. Every assertion below is about what `hook list` reports,
    # and a build that armed nothing would make some of them vacuously agreeable.
    assert sorted({r["id"] for r in _rows(runner)}) == ["post", "pre"], _rows(runner)
    return hook


# ---------------------------------------------------------------------------
# The defect: an armed hook must appear in the inventory
# ---------------------------------------------------------------------------


def test_an_armed_hook_is_listed_with_its_tier_events_and_no_verdict(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The whole point of the command: a hook that is armed right now is reported.

    Fails against the stub, which returns `{"items": []}` — the `ids` assertion
    is the first thing it breaks.
    """
    runner = grim_at(project_dir)
    _arm(runner, project_dir, unique_repo)

    report = runner.json("hook", "list")
    items = report["items"]

    assert [(i["artifact"], i["id"]) for i in items] == [
        ("shell-guard", "post"),
        ("shell-guard", "pre"),
    ], items

    by_id = {i["id"]: i for i in items}
    assert by_id["post"]["tier"] == "observer"
    assert by_id["pre"]["tier"] == "gatekeeper"
    assert by_id["post"]["events"] == ["PostToolUse"]
    assert by_id["pre"]["events"] == ["PreToolUse"]

    # `[]` is "armed on every configured client" — never "unknown". The dispatch
    # rows asserted in `_arm` are what makes that reading true here.
    assert all(i["arming"] == [] for i in items), items
    assert all(i["state"] == "installed" for i in items), items


def test_the_plain_table_names_each_hook_entry(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The plain format is the one a human reads, and it must not be headers only.

    Fails against the stub, whose empty report renders the header row and
    nothing else.
    """
    runner = grim_at(project_dir)
    _arm(runner, project_dir, unique_repo)

    out = runner.plain("hook", "list").stdout
    assert "shell-guard/pre" in out, out
    assert "shell-guard/post" in out, out
    assert "gatekeeper" in out, out


# ---------------------------------------------------------------------------
# A broken gate must name its cause, not vanish
# ---------------------------------------------------------------------------


def test_turning_the_feature_flag_off_reports_the_cause_per_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Arm, then break the feature-flag gate and re-converge.

    The payload survives (the record is left alone so `grim uninstall` can still
    reach the files), so the entries are still enumerable — and every one of them
    now carries `feature-flag-off` for the client that lost its registration.

    Fails against the stub twice over: there is no item to look up, and the
    stub's `state` is unreachable.
    """
    runner = grim_at(project_dir)
    _arm(runner, project_dir, unique_repo)

    runner.run("config", "set", "options.experimental.hooks", "false")
    runner.run("install")
    assert _rows(runner) == [], "re-converging with the flag off must disarm"

    items = runner.json("hook", "list")["items"]
    assert [i["id"] for i in items] == ["post", "pre"], items
    for item in items:
        assert item["state"] == "gated", item
        causes = [a["cause"] for a in item["arming"]]
        assert "feature-flag-off" in causes, item
        # C-017: the cause is the machine-readable field, and the message is the
        # remedy-bearing half — a cause with an empty message is a refusal the
        # user cannot act on.
        assert all(a["message"] for a in item["arming"]), item


def test_the_verdicts_agree_with_grim_status_for_the_same_hook(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """One hook, two commands, one answer.

    `grim hook list` derives its verdicts from `status.rs`'s own arming seam
    rather than re-deriving the gates. A second derivation is how the two
    commands come to describe one hook differently, and the user cannot tell
    which is right — so the agreement is asserted, not assumed.

    Fails against the stub: it has no item to compare against `status`'s row.
    """
    runner = grim_at(project_dir)
    _arm(runner, project_dir, unique_repo)
    runner.run("config", "set", "options.experimental.hooks", "false")
    runner.run("install")

    row = next(i for i in runner.json("status")["items"] if i["kind"] == "hook")
    items = runner.json("hook", "list")["items"]
    assert items, "the inventory must not be empty while `grim status` reports a hook"

    for item in items:
        assert item["state"] == row["state"], (item, row)
        assert [(a["client"], a["cause"]) for a in item["arming"]] == [
            (a["client"], a["cause"]) for a in row["arming"]
        ], (item, row)


# ---------------------------------------------------------------------------
# Degrading is not the same as staying silent
# ---------------------------------------------------------------------------


def test_a_declared_but_never_materialized_hook_warns_instead_of_vanishing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A gated hook is skipped before its blob is fetched (S-001), so there is no
    `hook.toml` on disk and no `[[hooks]]` entry to enumerate.

    The report degrades — it must not fail (I3) — but degrading silently is the
    same defect in a smaller costume, so the artifact is named on stderr.

    Fails against the stub, which emits a `debug` line naming nothing and is
    invisible under the default `warn` filter.
    """
    hook = _publish_hook(unique_repo)
    _declare(project_dir, hook.fq)
    runner = grim_at(project_dir)
    runner.run("lock")
    runner.run("install")  # gated by default (I4) — nothing is materialized

    result = runner.run("hook", "list", format="json")
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["items"] == []
    assert "shell-guard" in result.stderr, result.stderr

    # Positive control: enabling the feature materializes the payload, and the
    # very same command then reports both entries.
    runner.run("config", "set", "options.experimental.hooks", "true")
    runner.run("install", "--allow-hooks")
    assert [i["id"] for i in runner.json("hook", "list")["items"]] == ["post", "pre"]
