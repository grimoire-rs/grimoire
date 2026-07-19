# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim install` multi-client acceptance tests — config `clients` array.

The ``[options].clients`` TOML array drives which client layouts receive
the materialized artifacts when ``--client`` is absent.  The ``--client``
flag overrides the config array for a single invocation.
"""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from src.assertions import assert_not_exists, assert_path_exists
from src.helpers import make_artifact


def _build_toml(
    project_dir: Path,
    skill_ref: str,
    rule_ref: str,
    clients: list[str] | None,
) -> None:
    """Write a grimoire.toml with one skill+rule.

    ``clients`` writes ``[options].clients`` when a list is given; ``None``
    omits the ``[options]`` table entirely so default-client detection runs.
    """
    options = ""
    if clients is not None:
        clients_toml = ", ".join(f'"{c}"' for c in clients)
        options = f"[options]\nclients = [{clients_toml}]\n\n"
    toml = (
        f"{options}"
        "[skills]\n"
        f'code-review = "{skill_ref}"\n'
        "\n"
        "[rules]\n"
        f'rust-style = "{rule_ref}"\n'
    )
    (project_dir / "grimoire.toml").write_text(toml)


def _publish_skill_and_rule(unique_repo: str):
    """Publish a single skill + rule pair and return ``(skill, rule)``."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {
            "code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n",
            "code-review/scripts/run.sh": "echo hi\n",
        },
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    return sk, ru


def test_no_clients_config_installs_to_detected_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """No ``--client`` and no ``[options].clients`` installs to the detected
    clients only.

    With ``.opencode`` and ``.github/instructions`` present (but no
    ``.claude``), the materialized artifacts land in those two layouts and
    NOT in ``.claude``.
    """
    sk, ru = _publish_skill_and_rule(unique_repo)
    # Pre-create the OpenCode + Copilot markers (NOT .claude). A bare
    # `.github/instructions` dir is the Copilot detection signal.
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    (project_dir / ".github" / "instructions").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert rows, "install must return a non-empty result set"
    assert all(r["status"] in ("installed", "unchanged") for r in rows), (
        f"all entries must be installed/unchanged, got: {rows}"
    )

    # Detected clients (OpenCode + Copilot) received the artifacts.
    assert_path_exists(project_dir / ".opencode/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".opencode/rules/rust-style.md")
    assert_path_exists(project_dir / ".github/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".github/instructions/rust-style.instructions.md")

    # Claude was NOT detected ⇒ no `.claude` artifacts.
    assert_not_exists(project_dir / ".claude/skills/code-review")
    assert_not_exists(project_dir / ".claude/rules/rust-style.md")


def test_no_detected_clients_falls_back_to_all_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """No ``--client``, no ``[options].clients``, and no vendor dirs present
    falls back to **all** clients — no client is silently preferred."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert rows, "install must return a non-empty result set"

    # Every client layout received the artifacts.
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")
    assert_path_exists(project_dir / ".opencode/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".opencode/rules/rust-style.md")
    assert_path_exists(project_dir / ".github/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".github/instructions/rust-style.instructions.md")
    # C3.9 leftover: Codex is the 4th ALL-fallback client — its skill lands
    # at the cross-vendor `.agents/skills` standard (the rule is declined,
    # same as the other Codex tests in this file).
    assert_path_exists(project_dir / ".agents/skills/code-review/SKILL.md")
    assert_not_exists(project_dir / ".codex/rules/rust-style.md")


def test_config_clients_array_installs_to_all_declared_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``clients = ["claude", "copilot"]`` in config installs to both without --client."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {
            "code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n",
            "code-review/scripts/run.sh": "echo hi\n",
        },
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    _build_toml(project_dir, sk.fq, ru.fq, ["claude", "copilot"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    rows = runner.json("install")["items"]
    assert rows, "install must return a non-empty result set"
    assert all(r["status"] in ("installed", "unchanged") for r in rows), (
        f"all entries must be installed/unchanged, got: {rows}"
    )

    # Claude layout.
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")

    # Copilot layout — skill verbatim, rule transformed.
    assert_path_exists(project_dir / ".github/skills/code-review/SKILL.md")
    assert_path_exists(
        project_dir / ".github/instructions/rust-style.instructions.md"
    )


def test_config_clients_array_includes_codex_skill_and_skips_rule(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``clients = ["codex"]`` installs the skill to the cross-vendor
    ``.agents/skills`` tree and **skips** the rule (Codex declines rules):
    no ``.codex`` rule file is written and stderr carries the skip warning.
    """
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, ["codex"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", format="json")
    rows = json.loads(result.stdout)
    assert rows, "install must return a non-empty result set"

    # Skill lands in the cross-vendor `.agents/skills` standard, NOT `.codex`.
    assert_path_exists(project_dir / ".agents/skills/code-review/SKILL.md")
    assert_not_exists(project_dir / ".codex/skills/code-review")

    # Rule is declined: no Codex rule file anywhere.
    assert_not_exists(project_dir / ".codex/rules/rust-style.md")
    assert_not_exists(project_dir / ".agents/rules/rust-style.md")

    # The skip is surfaced on stderr.
    assert "no native target for rule" in result.stderr.lower(), (
        f"a rule installed for Codex must warn on stderr; got: {result.stderr!r}"
    )
    assert "codex" in result.stderr.lower()


def test_client_codex_rule_only_warns_and_writes_nothing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A project declaring only a rule, installed with ``--client codex``,
    writes no Codex file but still records the artifact (install succeeds)."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "codex", format="json")
    rows = json.loads(result.stdout)
    assert rows, "install must return a non-empty result set"
    # No Codex rule file is written anywhere.
    assert_not_exists(project_dir / ".codex/rules/rust-style.md")
    assert "no native target for rule" in result.stderr.lower(), result.stderr


def test_client_flag_overrides_config_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``--client opencode`` overrides the config ``clients`` list."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    # Config declares claude+copilot; the test overrides to opencode only.
    _build_toml(project_dir, sk.fq, ru.fq, ["claude", "copilot"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    rows = runner.json("install", "--client", "opencode")["items"]
    assert rows, "install must return a non-empty result set"

    # OpenCode layout must exist.
    assert_path_exists(project_dir / ".opencode/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".opencode/rules/rust-style.md")


def test_mixed_client_selection_stderr_stays_quiet_for_supporting_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C3.9 leftover: `--client claude,codex` on a rule installs cleanly for
    claude; codex's per-client decline is expected (not every client can
    host every kind) and must stay debug-only — the artifact-level "no
    client can host this" warning is reserved for the case where the WHOLE
    selected set declines (see `test_client_codex_rule_only_warns_and_writes_nothing`
    below), so a mixed selection that merely *includes* Codex stays quiet
    on stderr."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "claude,codex", check=False)
    assert result.returncode == 0, result.stderr
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")
    assert_not_exists(project_dir / ".codex/rules/rust-style.md")
    assert result.stderr.strip() == "", (
        f"a mixed selection where another client covers the kind must stay quiet on stderr: {result.stderr!r}"
    )


def test_prior_output_skipped_ordering_in_mixed_status_set(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A mixed batch containing both real outputs and a zero-output
    Codex-declined kind must report each item's own status — a `skipped`
    entry never masks (or gets masked by) a sibling `installed`/`updated`
    status. `--client codex` on a skill+rule batch makes the skill
    `installed` (Codex supports skills) while the rule stays `skipped`
    (Codex declines rules); a version bump on the skill flips it to
    `updated` while the rule's status is unchanged."""
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# v1\n"},
        tag="stable",
    )
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# rust\n"},
        tag="v1",
    )
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    first = runner.json("install", "--client", "codex")["items"]
    assert len(first) == 2, first
    by_name = {r["name"]: r for r in first}
    assert by_name["code-review"]["status"] == "installed", by_name
    assert by_name["code-review"]["target"] is not None
    assert by_name["rust-style"]["status"] == "skipped", by_name
    assert by_name["rust-style"]["target"] is None

    # Move the skill's floating tag onto new content (rolling release) so a
    # second install reports `updated` for it, while the Codex-declined
    # rule stays `skipped` alongside it in the same batch.
    make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# v2\n"},
        tag="stable",
    )
    runner.run("lock", check=False)
    second = runner.json("install", "--client", "codex")["items"]
    assert len(second) == 2, second
    by_name2 = {r["name"]: r for r in second}
    assert by_name2["code-review"]["status"] == "updated", by_name2
    assert by_name2["code-review"]["target"] is not None
    assert by_name2["rust-style"]["status"] == "skipped", by_name2
    assert by_name2["rust-style"]["target"] is None


def test_uninstall_of_a_zero_output_declined_record_is_clean(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """C3.9 leftover: a rule installed with `--client codex` only produces a
    zero-output record (Codex declines rules). Uninstalling it must not
    crash or leave orphaned state — it converges cleanly like any other
    record."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    rows = runner.json("install", "--client", "codex")["items"]
    assert rows[0]["status"] == "skipped", rows
    assert rows[0]["target"] is None, rows

    out = runner.json("uninstall", "rule", "rust-style")
    assert out["status"] in ("uninstalled", "removed"), out

    status = runner.json("status")["items"]
    assert not any(r["name"] == "rust-style" for r in status), (
        f"a zero-output record must uninstall cleanly, got: {status}"
    )


# ---------------------------------------------------------------------------
# Wave-1 vendor declined-kind semantics (mirrors the Codex-declines-Rule
# blocks above, one row per new vendor).
#
# Rule declines: Junie/Gemini/Zed/Amp have no grim-ownable per-file rule
# surface (adr_vendor_wave_expansion.md §2 — Kiro is Native, so excluded).
# Agent declines: Kiro (#8040 CLI/IDE schema collision), Junie (EAP-only),
# Zed (ACP, no file format), Amp (runtime-spawned subagents).
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("client", ["junie", "gemini", "zed", "amp"])
def test_declined_rule_vendor_warns_skips_and_uninstalls_clean(
    grim_at, project_dir: Path, registry: str, unique_repo: str, client: str
) -> None:
    """A rule installed with ``--client <declining-vendor>`` warns on stderr,
    reports ``skipped`` with a null target and zero outputs, writes no file,
    and uninstalls cleanly."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", "--client", client, format="json")
    rows = json.loads(result.stdout)["items"]
    assert rows[0]["status"] == "skipped", rows
    assert rows[0]["target"] is None, rows
    assert "no native target" in result.stderr.lower(), (
        f"a declined rule for {client} must warn on stderr; got: {result.stderr!r}"
    )
    assert client in result.stderr.lower()

    # Zero outputs: the status record carries no materialized output.
    status = runner.json("status")["items"]
    assert status[0]["outputs"] == [], f"a declined rule must record zero outputs: {status}"

    # Clean uninstall — no crash, no orphaned record.
    out = runner.json("uninstall", "rule", "rust-style")
    assert out["status"] in ("uninstalled", "removed"), out
    after = runner.json("status")["items"]
    assert not any(r["name"] == "rust-style" for r in after), after


@pytest.mark.parametrize("client", ["kiro", "junie", "zed", "amp"])
def test_declined_agent_vendor_warns_skips_and_uninstalls_clean(
    grim_at, project_dir: Path, registry: str, unique_repo: str, client: str
) -> None:
    """An agent installed with ``--client <declining-vendor>`` warns on stderr,
    reports ``skipped`` with a null target and zero outputs, and uninstalls
    cleanly. Cursor and Gemini support agents and are excluded here."""
    ag = make_artifact(
        f"{unique_repo}/my-agent",
        "agent",
        {"my-agent.md": "---\nname: my-agent\ndescription: d\nmodel: sonnet\n---\n# my-agent\nbody\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[agents]\nmy-agent = "{ag.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", "--client", client, format="json")
    rows = json.loads(result.stdout)["items"]
    assert rows[0]["status"] == "skipped", rows
    assert rows[0]["target"] is None, rows
    assert "no native target" in result.stderr.lower(), (
        f"a declined agent for {client} must warn on stderr; got: {result.stderr!r}"
    )
    assert client in result.stderr.lower()

    status = runner.json("status")["items"]
    assert status[0]["outputs"] == [], f"a declined agent must record zero outputs: {status}"

    out = runner.json("uninstall", "agent", "my-agent")
    assert out["status"] in ("uninstalled", "removed"), out
    after = runner.json("status")["items"]
    assert not any(r["name"] == "my-agent" for r in after), after
