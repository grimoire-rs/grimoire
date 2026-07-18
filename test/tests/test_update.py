# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim update` acceptance tests (rolling release)."""
from __future__ import annotations

import json
from pathlib import Path

from src.assertions import assert_not_exists, assert_path_exists
from src.helpers import make_artifact, write_config
from src.registry import retag


def _write_rule_config(project_dir: Path, rule_ref: str, clients: list[str]) -> None:
    """Write a grimoire.toml declaring one rule with an explicit
    ``[options].clients`` array."""
    clients_toml = ", ".join(f'"{c}"' for c in clients)
    (project_dir / "grimoire.toml").write_text(
        f"[options]\nclients = [{clients_toml}]\n\n"
        "[rules]\n"
        f'rust-style = "{rule_ref}"\n'
    )


def _rule_row(rows: list[dict], name: str = "rust-style") -> dict:
    return next(r for r in rows if r["name"] == name)


def _recorded_clients(project_dir: Path, name: str = "rust-style") -> set[str]:
    """The client names recorded in ``.grimoire/state.json`` for ``name``."""
    state = json.loads((project_dir / ".grimoire" / "state.json").read_text())
    for rec in state.get("records", []):
        if rec.get("name") == name:
            return {o["client"] for o in rec.get("outputs", [])}
    return set()


def test_update_rewrites_lock_and_rematerializes(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    repo = f"{unique_repo}/code-review"
    v1 = make_artifact(
        repo, "skill", {"code-review/SKILL.md": "v1\n"}, tag="1.0.0"
    )
    make_artifact(  # floating tag initially points at v1
        repo, "skill", {"code-review/SKILL.md": "v1\n"}, tag="stable"
    )
    write_config(
        project_dir, skills={"code-review": f"{registry}/{repo}:stable"}
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    installed = project_dir / ".claude/skills/code-review/SKILL.md"
    assert installed.read_text() == "v1\n"

    # Publish v2 and move the floating tag onto it (rolling release).
    v2 = make_artifact(
        repo, "skill", {"code-review/SKILL.md": "v2\n"}, tag="2.0.0"
    )
    retag(repo, "stable", v2.digest)
    assert v1.digest != v2.digest

    rows = runner.json("update")["items"]
    assert rows[0]["action"] == "updated"
    assert installed.read_text() == "v2\n"
    assert v2.digest in (project_dir / "grimoire.lock").read_text()


def test_update_named_only_touches_that_artifact(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a_repo = f"{unique_repo}/a"
    b_repo = f"{unique_repo}/b"
    make_artifact(a_repo, "rule", {"a.md": "a1\n"}, tag="latest")
    make_artifact(b_repo, "rule", {"b.md": "b1\n"}, tag="latest")
    write_config(
        project_dir,
        rules={
            "a": f"{registry}/{a_repo}:latest",
            "b": f"{registry}/{b_repo}:latest",
        },
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    a2 = make_artifact(a_repo, "rule", {"a.md": "a2\n"}, tag="latest")
    assert a2  # a's floating tag advanced; b unchanged

    rows = runner.json("update", "a")["items"]
    by_name = {r["name"]: r for r in rows}
    assert by_name["a"]["action"] == "updated"
    assert by_name["b"]["action"] == "unchanged"
    # A partial update carries non-named entries forward in the lock, so the
    # prune pass must not treat "b" as an orphan and delete it.
    assert all(r["action"] != "removed" for r in rows), "partial update must not prune the unnamed entry"
    assert (project_dir / ".claude/rules/a.md").read_text() == "a2\n"
    assert (project_dir / ".claude/rules/b.md").read_text() == "b1\n"


def test_prune_of_a_zero_output_declined_record_is_clean(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C3.9 leftover: a zero-output record (a rule installed with
    `--client codex` only — Codex declines rules) must prune cleanly when
    dropped from the declaration; no crash, no orphaned state."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "codex")["items"]
    assert rows[0]["status"] == "skipped", rows

    # Drop the declaration and re-lock — the prune pass must reap the
    # zero-output record cleanly.
    (project_dir / "grimoire.toml").write_text("")
    runner.run("lock", check=False)
    update_rows = runner.json("update")["items"]
    assert any(r["action"] == "removed" for r in update_rows), update_rows

    status = runner.json("status")["items"]
    assert not any(r["name"] == "rust-style" for r in status), status


def test_update_installs_newly_declared_rule(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    # Add-on-upgrade for a non-bundle, config-level addition: the
    # declaration gains a new rule and the lock is refreshed (so the
    # stale-lock guard stays out of the picture); a full `grim update`
    # must then materialize the addition through the full-resolve path —
    # even though it was never separately installed, the update's
    # force-materialization writes it to disk.
    a_repo = f"{unique_repo}/a"
    make_artifact(a_repo, "rule", {"a.md": "a1\n"}, tag="latest")
    write_config(project_dir, rules={"a": f"{registry}/{a_repo}:latest"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    assert (project_dir / ".claude/rules/a.md").read_text() == "a1\n"

    # Declare a second rule AND re-lock so the lock is not stale, then run a
    # full update (no names) to exercise the add path via resolve_lock.
    b_repo = f"{unique_repo}/b"
    make_artifact(b_repo, "rule", {"b.md": "b1\n"}, tag="latest")
    write_config(
        project_dir,
        rules={
            "a": f"{registry}/{a_repo}:latest",
            "b": f"{registry}/{b_repo}:latest",
        },
    )
    runner.run("lock", check=False)
    # The new rule is locked but not yet on disk — only `update` materializes it.
    assert not (project_dir / ".claude/rules/b.md").exists()

    rows = runner.json("update")["items"]
    by_name = {r["name"]: r for r in rows}
    # The newly declared rule appears in the update report with a real pin.
    # Its action is `unchanged` (the re-lock above already established the
    # pin, so the digest does not move during update) — the add-on-upgrade
    # contract is the disk materialization below, not the diff label.
    assert "b" in by_name
    assert by_name["b"]["new"] is not None, "the locked rule carries a pin"
    # The addition is materialized by the update; the pre-existing rule is
    # untouched.
    assert (project_dir / ".claude/rules/b.md").read_text() == "b1\n"
    assert (project_dir / ".claude/rules/a.md").read_text() == "a1\n"


def test_partial_update_with_stale_lock_exits_65(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    a_repo = f"{unique_repo}/a"
    make_artifact(a_repo, "rule", {"a.md": "a1\n"}, tag="latest")
    write_config(project_dir, rules={"a": f"{registry}/{a_repo}:latest"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    # Mutate the declaration (add a rule) without re-locking, then ask for
    # a *partial* update — the stale-lock guard must refuse with 65.
    b_repo = f"{unique_repo}/b"
    make_artifact(b_repo, "rule", {"b.md": "b1\n"}, tag="latest")
    write_config(
        project_dir,
        rules={
            "a": f"{registry}/{a_repo}:latest",
            "b": f"{registry}/{b_repo}:latest",
        },
    )
    result = runner.run("update", "a", check=False)
    assert result.returncode == 65, (
        f"partial update on a stale lock must exit 65, got "
        f"{result.returncode}; {result.stderr}"
    )


def test_update_reaps_unmodified_dropped_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Narrowing ``[options].clients`` from claude+copilot to claude and
    running `grim update` reaps the dropped client's unmodified output:
    the file is deleted, the install record no longer lists it, and the
    update report row surfaces it under ``reaped_clients``."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\n"},
        tag="v1",
    )
    _write_rule_config(project_dir, ru.fq, ["claude", "copilot"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    claude = project_dir / ".claude/rules/rust-style.md"
    copilot = project_dir / ".github/instructions/rust-style.instructions.md"
    assert_path_exists(claude)
    assert_path_exists(copilot)

    # Drop copilot from the configured client set, then update (no tag move).
    _write_rule_config(project_dir, ru.fq, ["claude"])
    rows = runner.json("update")["items"]

    assert_path_exists(claude)
    assert_not_exists(copilot)
    row = _rule_row(rows)
    assert row["reaped_clients"] == ["copilot"], row
    assert row["kept_modified_clients"] == [], row
    assert _recorded_clients(project_dir) == {"claude"}, "copilot must leave the record"

    # State stays a valid V2 file.
    state = json.loads((project_dir / ".grimoire" / "state.json").read_text())
    assert state["version"] == 2, state


def test_update_preserves_modified_dropped_client_then_force_reaps(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A user-edited output for a dropped client is preserved (file kept,
    still in state, reported under ``kept_modified_clients``) on a plain
    `grim update`; `grim update --force` then deletes it and drops it from
    the record."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\n"},
        tag="v1",
    )
    _write_rule_config(project_dir, ru.fq, ["claude", "copilot"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    copilot = project_dir / ".github/instructions/rust-style.instructions.md"
    assert_path_exists(copilot)
    # Hand-edit the copilot output so its content drifts from the record.
    copilot.write_text("locally edited by the user\n")

    # Drop copilot from config, update WITHOUT --force: the edit is preserved.
    _write_rule_config(project_dir, ru.fq, ["claude"])
    rows = runner.json("update")["items"]
    row = _rule_row(rows)
    assert row["kept_modified_clients"] == ["copilot"], row
    assert row["reaped_clients"] == [], row
    assert copilot.read_text() == "locally edited by the user\n", "edit preserved"
    assert "copilot" in _recorded_clients(project_dir), "kept-modified stays in state"

    # --force reaps even the modified output.
    rows = runner.json("update", "--force")["items"]
    row = _rule_row(rows)
    assert row["reaped_clients"] == ["copilot"], row
    assert_not_exists(copilot)
    assert _recorded_clients(project_dir) == {"claude"}, "force drops copilot from state"


def test_update_widens_client_set_materializes_added_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Adding a client to ``[options].clients`` materializes it on the next
    `grim update` (covers_targets widening) — the reaper is drop-only and
    never removes a still-configured client."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\n"},
        tag="v1",
    )
    _write_rule_config(project_dir, ru.fq, ["claude"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)

    copilot = project_dir / ".github/instructions/rust-style.instructions.md"
    assert_not_exists(copilot)

    # Widen to claude+copilot and update: copilot must materialize.
    _write_rule_config(project_dir, ru.fq, ["claude", "copilot"])
    rows = runner.json("update")["items"]
    assert_path_exists(copilot)
    row = _rule_row(rows)
    assert row["reaped_clients"] == [], row
    assert _recorded_clients(project_dir) == {"claude", "copilot"}, row
