# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Local-modification integrity gate acceptance tests."""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact, write_config
from src.registry import retag


def _install_rule(grim_at, project_dir, registry, unique_repo):
    repo = f"{unique_repo}/rust-style"
    make_artifact(
        repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# canonical\n"},
        tag="v1",
    )
    write_config(
        project_dir, rules={"rust-style": f"{registry}/{repo}:v1"}
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    return runner, project_dir / ".claude/rules/rust-style.md"


def test_modified_install_is_refused_then_forced(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    refused = runner.run("install", check=False)
    assert refused.returncode == 65, (
        f"modified artifact must refuse with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused install must not overwrite the user's edit"
    )

    forced = runner.run("install", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert installed.read_text().endswith("# canonical\n")


def test_modified_add_is_refused_then_forced(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim add` installs-on-add through the same integrity gate as
    `grim install`: re-adding the same reference over a locally modified
    file refuses (65) until `--force` — the VS Code-extension retry shape."""
    repo = f"{unique_repo}/rust-style"
    make_artifact(
        repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# canonical\n"},
        tag="v1",
    )
    write_config(project_dir)
    runner = grim_at(project_dir)
    ref = f"{registry}/{repo}:v1"

    added = runner.run("add", ref, check=False)
    assert added.returncode == 0, added.stderr
    installed = project_dir / ".claude/rules/rust-style.md"
    installed.write_text("hand edited\n")

    # Re-adding the same reference is an idempotent re-declare that reaches
    # the shared install pipeline — the integrity gate refuses it.
    refused = runner.run("add", ref, check=False)
    assert refused.returncode == 65, (
        f"modified artifact must refuse re-add with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused add must not overwrite the user's edit"
    )

    forced = runner.run("add", "--force", ref, check=False)
    assert forced.returncode == 0, forced.stderr
    assert installed.read_text().endswith("# canonical\n")


def test_status_reports_modified(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("tampered\n")

    rows = runner.json("status")["items"]
    row = next(r for r in rows if r["name"] == "rust-style")
    assert row["state"] == "modified"
    # status is read-only data: it must always exit 0.


def test_update_also_refuses_modified_without_force(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`update` runs the same integrity gate as `install`.

    Regression: `update` used to pass a hard-coded `force = true` into the
    installer, so a locally modified artifact was overwritten silently —
    exit 0, no warning, no report field — while `install` refused the same
    bytes with 65. Destroying a hand edit is the one thing `--force` exists
    to gate, and update's own prune/reap passes already honour it.
    """
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    refused = runner.run("update", check=False)
    assert refused.returncode == 65, (
        f"modified artifact must refuse update with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused update must not overwrite the user's edit"
    )

    # C-006: the refusal must name the artifact and the reason, and say
    # that --force also authorizes update's own prune/reap deletions, not
    # just the install gate. Today's message is the shared
    # `IntegrityMismatch` text alone ("rerun with --force to overwrite"),
    # with no mention of prune/reap — red until `update.rs` wraps it with
    # update-specific context at the call site (D3: wrap, never by
    # parameterizing the shared message `install.rs` also emits).
    stderr_lower = refused.stderr.lower()
    assert "rust-style" in refused.stderr, (
        f"the refusal must name the artifact; got {refused.stderr}"
    )
    assert "modified locally" in stderr_lower, (
        f"the refusal must state the reason (local modification); got {refused.stderr}"
    )
    assert "prune" in stderr_lower and "reap" in stderr_lower, (
        "the refusal must say --force on update also authorizes prune "
        f"and reap deletions, not just the install gate; got {refused.stderr}"
    )

    forced = runner.run("update", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert installed.read_text().endswith("# canonical\n")


def test_install_refusal_message_stays_generic_no_update_wording(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Negative counterpart to C-006: `grim install`'s refusal must not
    gain update's prune/reap wording.

    D3 chose the call-site wrap at `update.rs:311` specifically so the
    shared `IntegrityMismatch` message (also emitted by `install.rs:382`)
    stays generic. The forbidden alternative — parameterizing the shared
    message instead — would leak update-only vocabulary into `install`'s
    refusal too. This test must stay green through the fix; if it goes
    red, the forbidden route was taken.
    """
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    refused = runner.run("install", check=False)
    assert refused.returncode == 65, refused.stderr
    # "prune", not the bare word "reap": `installer.rs` logs unrelated
    # "could not reap …" warnings on its own failure paths, and a trip-wire
    # that fires on those cries wolf and gets deleted. "prune" is logged by
    # nothing `grim install` can reach, so it stays broad enough to catch any
    # phrasing of the authorization wording, not just update's exact one.
    assert "prune" not in refused.stderr.lower(), (
        f"install's refusal must not gain update-specific wording; got {refused.stderr}"
    )


def test_update_refusal_still_emits_the_report_for_what_reconciled(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C-005 / S-003: a refused `update` must still report what reconciled.

    Regression: `update::run` builds the full `UpdateReport`
    (`update.rs:306`) and then throws it away with `return Err`
    (`update.rs:309`) the instant *any* outcome is a refusal — so one
    hand-edited artifact turns a fully-completed update of every other
    artifact into total failure with no report at all. This must exit 65
    *and* still emit the report covering everything that did reconcile.
    """
    modified_repo = f"{unique_repo}/rust-style"
    make_artifact(
        modified_repo,
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# canonical\n"},
        tag="v1",
    )
    other_repo = f"{unique_repo}/other-rule"
    make_artifact(other_repo, "rule", {"other-rule.md": "v1\n"}, tag="stable")
    write_config(
        project_dir,
        rules={
            "rust-style": f"{registry}/{modified_repo}:v1",
            "other-rule": f"{registry}/{other_repo}:stable",
        },
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    modified = project_dir / ".claude/rules/rust-style.md"
    modified.write_text("hand edited\n")

    # Roll `other-rule`'s floating tag forward so this update pass has real
    # reconciliation work to do besides the refusal.
    v2 = make_artifact(other_repo, "rule", {"other-rule.md": "v2\n"}, tag="2")
    retag(other_repo, "stable", v2.digest)
    other_installed = project_dir / ".claude/rules/other-rule.md"

    result = runner.run("update", format="json", check=False)
    assert result.returncode == 65, result.stderr
    assert modified.read_text() == "hand edited\n", (
        "a refused update must not overwrite the user's edit"
    )
    assert result.stdout.strip(), (
        "a report must still be emitted on stdout alongside the 65 exit; "
        f"got empty stdout, stderr={result.stderr!r}"
    )
    report = json.loads(result.stdout)
    names = {item["name"] for item in report["items"]}
    assert {"rust-style", "other-rule"} <= names, (
        f"the report must cover every artifact, not just the refused one; got {report}"
    )
    assert other_installed.read_text() == "v2\n", (
        "other-rule's reconciliation must have actually run despite the sibling refusal"
    )

    # Error case (S-003): re-running is idempotent — same refusal, same
    # exit code, and a report is still surfaced, never a bare failure.
    again = runner.run("update", format="json", check=False)
    assert again.returncode == 65
    assert again.stdout.strip(), "a repeat run must still surface a report"
    assert {i["name"] for i in json.loads(again.stdout)["items"]} >= {"rust-style"}


def test_update_refusal_marks_the_row_in_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C-005: the refused row is machine-identifiable, not just the exit code.

    Because the report now travels the success path, `emit_error_document`
    never runs, so `--format json` carries no `reason`. The additive
    always-present `refused` bool on the row is what lets a consumer tell an
    integrity refusal from any other 65 — and which artifact it is about.
    """
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    refused = runner.run("update", format="json", check=False)
    assert refused.returncode == 65, refused.stderr
    row = next(
        r for r in json.loads(refused.stdout)["items"] if r["name"] == "rust-style"
    )
    assert row["refused"] is True, f"the refused row must say so; got {row}"
    # `action` still reports the lock diff — the pin rolled forward, only the
    # materialization was refused.
    assert row["action"] in {"updated", "unchanged"}, row

    forced = runner.run("update", "--force", format="json", check=False)
    assert forced.returncode == 0, forced.stderr
    assert all(not r["refused"] for r in json.loads(forced.stdout)["items"]), (
        "a clean update must carry the key on every row, always false"
    )


def test_update_refusal_names_the_artifact_on_stderr(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The refused artifact is named, not just counted by the exit code.

    `update` inspects only `Err` outcomes; a `Refused` one is `Ok(..)`, so
    without explicit handling the refusal falls through as exit 0 with the
    artifact silently unmaterialized. The refusal travels the same route as
    `install`'s: an `IntegrityMismatch` carrying the artifact reference. The
    report survives the refusal (C-005) but carries no per-row refusal
    marker, so stderr is what names the refused artifact.
    """
    runner, installed = _install_rule(
        grim_at, project_dir, registry, unique_repo
    )
    installed.write_text("hand edited\n")

    result = runner.run("update", check=False)
    assert result.returncode == 65, result.stderr
    assert "rust-style" in result.stderr, (
        f"the refusal must name the artifact; got {result.stderr}"
    )
