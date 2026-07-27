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


@pytest.fixture()
def project_dir(bare_project_dir: Path) -> Path:
    """Override: this suite is *about* client selection, so every workspace
    starts with no vendor marker at all and each test creates exactly the
    markers it means to have detected."""
    return bare_project_dir


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


def test_no_detected_clients_falls_back_to_the_generic_agents_client(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """No ``--client``, no ``[options].clients``, and no vendor dirs present
    targets the single generic ``agents`` client — one copy into the
    cross-vendor pool, not a copy into every vendor directory grim knows.

    The old behaviour installed into all ten clients, and the ten vendor
    directories it created were exactly what made the *next* run "detect"
    all ten: a fallback that manufactured its own detection signal.
    """
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install")["items"]
    assert rows, "install must return a non-empty result set"

    # The skill lands once, in the cross-vendor pool.
    assert_path_exists(project_dir / ".agents/skills/code-review/SKILL.md")

    # No vendor directory was created for any product client.
    for vendor_dir in (
        ".claude",
        ".opencode",
        ".github",
        ".codex",
        ".cursor",
        ".kiro",
        ".junie",
        ".gemini",
        ".zed",
        ".amp",
    ):
        assert_not_exists(project_dir / vendor_dir)

    # The rule is declined by the generic client (no vendor-neutral rule
    # surface exists): warn, skip, zero outputs — never a hard error, because
    # the skill in the same set did install.
    assert_not_exists(project_dir / ".agents/rules/rust-style.md")

    # Only `agents` is recorded, and the pool directory it just wrote must
    # not change the *next* run's resolution.
    outputs = {o["client"] for row in runner.json("status")["items"] for o in row["outputs"]}
    assert outputs == {"agents"}, f"only the generic client is recorded; got {outputs}"
    assert runner.json("context")["clients"] == ["agents"]
    second = runner.json("install")["items"]
    assert {r["name"]: r["status"] for r in second} == {
        "code-review": "unchanged",
        "rust-style": "skipped",
    }, second
    for vendor_dir in (".claude", ".opencode", ".github", ".codex"):
        assert_not_exists(project_dir / vendor_dir)


def test_undetected_workspace_with_no_installable_kind_exits_78(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The residual 78: the generic fallback renders skills only, so an
    artifact set of nothing but a rule has nowhere at all to go. That is the
    one case where grim genuinely cannot act, and it must say so instead of
    exiting 0 having written nothing."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    result = runner.run("install", check=False)
    assert result.returncode == 78, (
        f"expected EX_CONFIG (78), got {result.returncode}: {result.stderr}"
    )
    assert "--client" in result.stderr, (
        f"the message must name --client so the user can act: {result.stderr}"
    )
    assert_not_exists(project_dir / ".claude")
    assert_not_exists(project_dir / ".agents")


def test_generic_client_output_survives_a_real_client_appearing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Regression: the generic client is never *detected*, so reconciling its
    recorded output against detection would erase it. Install on a bare
    workspace, then let a real client appear — `status` must still report the
    skill installed with its `agents` output, not `missing` with `outputs: []`.
    The file is on disk and grim put it there."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.json("install")

    pooled = project_dir / ".agents/skills/code-review/SKILL.md"
    assert_path_exists(pooled)

    # A real client shows up afterwards — detection now answers `[claude]`.
    (project_dir / ".claude").mkdir()

    row = next(r for r in runner.json("status")["items"] if r["name"] == "code-review")
    assert row["state"] == "installed", f"the pooled skill is still on disk: {row}"
    assert [o["client"] for o in row["outputs"]] == ["agents"], row
    assert pooled.is_file()


def test_undetected_add_of_a_rule_exits_78_but_keeps_the_declaration(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """`grim add` installs what it declares through the same seam, so it hits
    the same 78 — and it has no `--client` flag, which is why the message
    names `[options].clients` too. The declaration and the lock entry must
    survive so selecting a client finishes the job without re-adding."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    runner = grim_at(project_dir)
    (project_dir / "grimoire.toml").write_text("[rules]\n")

    result = runner.run("add", ru.fq, check=False)
    assert result.returncode == 78, f"got {result.returncode}: {result.stderr}"
    assert "--client" in result.stderr and "[options].clients" in result.stderr, (
        f"`add` has no --client flag, so the message must name the config key too: {result.stderr}"
    )

    # Declared and locked despite the refusal — the recovery path is to pick
    # a client, not to re-add.
    assert "rust-style" in (project_dir / "grimoire.toml").read_text()
    assert "rust-style" in (project_dir / "grimoire.lock").read_text()
    rows = runner.json("install", "--client", "claude")["items"]
    assert rows[0]["status"] == "installed", rows
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")


def test_undetected_dev_install_of_a_rule_exits_78(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    """The dev-install path builds a synthetic single-entry lock and runs the
    same seam, so it refuses identically — no network, no registry."""
    src = tmp_path / "rust-style.md"
    src.write_text("---\npaths: ['**/*.rs']\n---\n# Rust Style\n")
    (project_dir / "grimoire.toml").write_text("[rules]\n")
    runner = grim_at(project_dir)

    result = runner.run("install", str(src), "--kind", "rule", check=False)
    assert result.returncode == 78, f"got {result.returncode}: {result.stderr}"
    assert_not_exists(project_dir / ".agents")


def test_explicit_agents_client_with_only_a_rule_still_exits_zero(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``--client agents`` is a choice, not a fallback: the rule is declined
    with a warning exactly as Codex declines rules, and the exit code stays 0.
    The residual 78 fires only when grim picked the generic client itself."""
    ru = make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"},
        tag="v1",
    )
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n')

    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    result = runner.run("install", "--client", "agents", check=False)
    assert result.returncode == 0, f"explicit selection is never refused: {result.stderr}"


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
# Rule declines: Gemini/Zed/Amp have no grim-ownable per-file rule surface
# (adr_vendor_wave_expansion.md §2 — Kiro is Native, so excluded). Junie was
# here until its `.junie/rules/` surface was re-verified as current rather
# than legacy: it is now Degraded at project scope (installs, `paths`
# dropped) and has no global surface at all, so it is covered by its own
# pair of tests in test_shared_skills.py instead.
# Agent declines: Kiro (#8040 CLI/IDE schema collision), Junie (EAP-only),
# Zed (ACP, no file format), Amp (runtime-spawned subagents).
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "client",
    ["gemini", "zed", "amp", "cline", "droid", "goose", "warp", "kilo"],
)
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


@pytest.mark.parametrize(
    "client",
    ["kiro", "junie", "zed", "amp", "cline", "droid", "goose", "warp", "kilo"],
)
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


# ---------------------------------------------------------------------------
# Wave-2 skills-only batch: one skill, six clients, three different layouts.
#
# OpenClaw is absent from the project-scope table on purpose — it has no
# per-repository scope at all and is covered by its own pair of tests below.
# ---------------------------------------------------------------------------


def _skill_only(unique_repo: str):
    return make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# CR\n"},
        tag="v1",
    )


@pytest.mark.parametrize(
    ("client", "expected", "forbidden"),
    [
        ("cline", ".cline/skills/code-review", ".agents/skills/code-review"),
        # The client is `droid`; the directory is `.factory`. Both frozen.
        ("droid", ".factory/skills/code-review", ".droid/skills/code-review"),
        # Goose is the one that renders INTO the pool.
        ("goose", ".agents/skills/code-review", ".goose/skills/code-review"),
        # Warp is pool-capable but renders natively by default.
        ("warp", ".warp/skills/code-review", ".agents/skills/code-review"),
        # Never the deprecated `.kilocode`.
        ("kilo", ".kilo/skills/code-review", ".kilocode/skills/code-review"),
    ],
)
def test_skills_only_client_installs_to_its_own_layout(
    grim_at, project_dir: Path, registry: str, unique_repo: str,
    client: str, expected: str, forbidden: str,
) -> None:
    """Each wave-2 client writes exactly one layout and nothing else. The
    ``forbidden`` column is the mistake each one is most likely to make:
    rendering into the shared pool when it is not a pool client, using the
    client name as the directory, or resurrecting a deprecated dir."""
    sk = _skill_only(unique_repo)
    (project_dir / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    rows = runner.json("install", "--client", client)["items"]
    assert rows[0]["status"] == "installed", rows

    assert_path_exists(project_dir / expected / "SKILL.md")
    assert_not_exists(project_dir / forbidden)


def test_openclaw_project_scope_writes_nothing_and_warns(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """OpenClaw has no per-repository scope: the path its docs call "project"
    is a fixed daemon home shared by every repo on the machine. A project
    install must record zero outputs and touch nothing, rather than anchor a
    record at ``Workspace`` while the file lands outside the workspace."""
    sk = _skill_only(unique_repo)
    (project_dir / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n')
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "openclaw", format="json")
    rows = json.loads(result.stdout)["items"]
    assert rows[0]["status"] == "skipped", rows
    assert rows[0]["target"] is None, rows
    assert_not_exists(project_dir / ".openclaw")
    assert_not_exists(project_dir / ".agents/skills/code-review")
    assert "openclaw" in result.stderr.lower(), result.stderr

    status = runner.json("status")["items"]
    assert status[0]["outputs"] == [], f"zero outputs required: {status}"


def test_openclaw_global_scope_installs_to_its_own_root(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """The scope that does work: ``--global`` lands in ``~/.openclaw/skills``,
    OpenClaw's own root rather than the shared pool it also reads."""
    from src.runner import GrimRunner

    sk = _skill_only(unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "openclaw")["items"]
    assert rows[0]["status"] == "installed", rows

    assert_path_exists(runner.home / ".openclaw/skills/code-review/SKILL.md")
    assert_not_exists(runner.home / ".agents/skills/code-review")


def test_goose_global_skill_lands_in_the_shared_pool(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Goose is a full pool member at BOTH scopes, unlike Antigravity and Kilo.
    Its global skill must land in ``$HOME/.agents/skills`` — the same physical
    tree Codex/Gemini/Zed/Amp read — and never under a ``~/.goose`` root, which
    grim deliberately does not define an anchor for."""
    from src.runner import GrimRunner

    sk = _skill_only(unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n')
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "goose")["items"]
    assert rows[0]["status"] == "installed", rows

    assert_path_exists(runner.home / ".agents/skills/code-review/SKILL.md")
    assert_not_exists(runner.home / ".goose/skills/code-review")
