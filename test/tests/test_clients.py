# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""`grim install` multi-client acceptance tests — config `clients` array.

The ``[options].clients`` TOML array drives which client layouts receive
the materialized artifacts when ``--client`` is absent.  The ``--client``
flag overrides the config array for a single invocation.
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from src.assertions import assert_not_exists, assert_path_exists
from src.helpers import make_artifact, make_settings, write_config


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
    (project_dir / "grimoire.toml").write_text(toml, encoding="utf-8", newline="\n")


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
        ".qoder",
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


def test_generic_fallback_install_prints_a_hint_naming_the_pool_and_the_knobs(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Regression for #113: the fallback is the right default, but it used to
    fire silently — a Claude Code user following the quick start got an
    ``added``/``installed`` row and a skill in a pool Claude never reads. The
    install seam must print one hint on stderr naming the pool and both
    selection knobs (``add`` has no ``--client``). An explicit
    ``--client agents`` is a choice, not a fallback, and stays quiet."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", format="json")
    assert ".agents/skills" in result.stderr, f"hint must name the pool: {result.stderr}"
    assert "--client" in result.stderr and "[options].clients" in result.stderr, (
        f"hint must name both knobs: {result.stderr}"
    )

    chosen = runner.run("install", "--client", "agents", format="json")
    assert ".agents/skills" not in chosen.stderr, f"an explicit selection is not a fallback: {chosen.stderr}"


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
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n', encoding="utf-8", newline="\n")

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
    (project_dir / "grimoire.toml").write_text("[rules]\n", encoding="utf-8", newline="\n")

    result = runner.run("add", ru.fq, check=False)
    assert result.returncode == 78, f"got {result.returncode}: {result.stderr}"
    assert "--client" in result.stderr and "[options].clients" in result.stderr, (
        f"`add` has no --client flag, so the message must name the config key too: {result.stderr}"
    )

    # Declared and locked despite the refusal — the recovery path is to pick
    # a client, not to re-add.
    assert "rust-style" in (project_dir / "grimoire.toml").read_text(encoding="utf-8")
    assert "rust-style" in (project_dir / "grimoire.lock").read_text(encoding="utf-8")
    rows = runner.json("install", "--client", "claude")["items"]
    assert rows[0]["status"] == "installed", rows
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")


def test_undetected_dev_install_of_a_rule_exits_78(
    grim_at, project_dir: Path, tmp_path: Path
) -> None:
    """The dev-install path builds a synthetic single-entry lock and runs the
    same seam, so it refuses identically — no network, no registry."""
    src = tmp_path / "rust-style.md"
    src.write_text("---\npaths: ['**/*.rs']\n---\n# Rust Style\n", encoding="utf-8", newline="\n")
    (project_dir / "grimoire.toml").write_text("[rules]\n", encoding="utf-8", newline="\n")
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
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n', encoding="utf-8", newline="\n")

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


def test_reserved_namespace_key_dropped_for_own_client_warns_by_name(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A ``codex.*`` metadata key installed **for Codex** is dropped, and says so.

    Codex reserves the ``codex.`` prefix but declares no skill fields, so an
    own-namespace key can only ever be dropped.  Doing that silently is the
    defect: the key was plain pass-through data before the namespace was
    reserved, so a user reads its disappearance as a bug.  The warning has to
    name the key and the client, and this is the only test that proves the
    warning actually reaches the user rather than dying in the renderer.
    """
    sk = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {
            "code-review/SKILL.md": (
                "---\nname: code-review\ndescription: d\n"
                "metadata:\n  codex.made-up-key: x\n---\n# CR\n"
            )
        },
        tag="stable",
    )
    (project_dir / "grimoire.toml").write_text(
        f'[options]\nclients = ["codex"]\n\n[skills]\ncode-review = "{sk.fq}"\n', encoding="utf-8", newline="\n"
    )
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", format="json")

    installed = project_dir / ".agents/skills/code-review/SKILL.md"
    assert_path_exists(installed)
    assert "made-up-key" not in installed.read_text(encoding="utf-8"), (
        "an unknown own-namespace key must not survive into the installed file"
    )
    assert "codex.made-up-key" in result.stderr, (
        f"the drop must name the key on stderr; got: {result.stderr!r}"
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
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n', encoding="utf-8", newline="\n")
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
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n', encoding="utf-8", newline="\n")
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "claude,codex", check=False)
    assert result.returncode == 0, result.stderr
    assert_path_exists(project_dir / ".claude/rules/rust-style.md")
    assert_not_exists(project_dir / ".codex/rules/rust-style.md")
    assert result.stderr.strip() == "clients: claude (--client), codex (--client)", (
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

    # Bump the declared tag, which `grim lock` resolves (a moved tag alone it
    # keeps), so a second install reports `updated` for the skill, while the
    # Codex-declined rule stays `skipped` alongside it in the same batch.
    bumped = make_artifact(
        f"{unique_repo}/code-review",
        "skill",
        {"code-review/SKILL.md": "---\nname: code-review\ndescription: d\n---\n# v2\n"},
        tag="next",
    )
    _build_toml(project_dir, bumped.fq, ru.fq, clients=None)
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
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n', encoding="utf-8", newline="\n")
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
# Agent declines: Kiro (#8040 CLI/IDE schema collision), Zed (ACP, no file
# format), Amp (runtime-spawned subagents).
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
    (project_dir / "grimoire.toml").write_text(f'[rules]\nrust-style = "{ru.fq}"\n', encoding="utf-8", newline="\n")
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
    ["kiro", "zed", "amp", "cline", "warp"],
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
    (project_dir / "grimoire.toml").write_text(f'[agents]\nmy-agent = "{ag.fq}"\n', encoding="utf-8", newline="\n")
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
        # Pool-capable since 2026-09-27, but native by default (the Warp
        # shape) — the pool path is still the mistake to catch here.
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
    (project_dir / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n', encoding="utf-8", newline="\n")
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
    (project_dir / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n', encoding="utf-8", newline="\n")
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
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n', encoding="utf-8", newline="\n")
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "openclaw")["items"]
    assert rows[0]["status"] == "installed", rows

    assert_path_exists(runner.home / ".openclaw/skills/code-review/SKILL.md")
    assert_not_exists(runner.home / ".agents/skills/code-review")


def test_goose_global_skill_lands_in_the_shared_pool(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """Goose is a full pool member at BOTH scopes, unlike Antigravity.
    Its global skill must land in ``$HOME/.agents/skills`` — the same physical
    tree Codex/Gemini/Zed/Amp read — and never under a ``~/.goose`` root, which
    grim deliberately does not define an anchor for."""
    from src.runner import GrimRunner

    sk = _skill_only(unique_repo)
    (grim_home / "grimoire.toml").write_text(f'[skills]\ncode-review = "{sk.fq}"\n', encoding="utf-8", newline="\n")
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "goose")["items"]
    assert rows[0]["status"] == "installed", rows

    assert_path_exists(runner.home / ".agents/skills/code-review/SKILL.md")
    assert_not_exists(runner.home / ".goose/skills/code-review")


# ---------------------------------------------------------------------------
# Qoder: Claude Code's config shapes under `.qoder/`, all four kinds native.
# ---------------------------------------------------------------------------

_QODER_SKILL = "---\nname: code-review\ndescription: d\n---\n# CR\n"
_QODER_RULE = "---\npaths: ['**/*.rs']\n---\n# Rust Style\nUse 4 spaces.\n"
_QODER_AGENT = "---\nname: my-agent\ndescription: d\nmodel: inherit\ntools: Read,Grep\n---\n# my-agent\nbody\n"


def _qoder_artifacts(unique_repo: str):
    sk = make_artifact(
        f"{unique_repo}/code-review", "skill", {"code-review/SKILL.md": _QODER_SKILL}, tag="v1"
    )
    ru = make_artifact(f"{unique_repo}/rust-style", "rule", {"rust-style.md": _QODER_RULE}, tag="v1")
    ag = make_artifact(f"{unique_repo}/my-agent", "agent", {"my-agent.md": _QODER_AGENT}, tag="v1")
    return sk, ru, ag


def _release_qoder_mcp(runner, registry: str, unique_repo: str, src: Path) -> str:
    descriptor = src / "mcp" / "q-mcp.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\nargs = ["mcp"]\n'
        "timeout = 7000\n", encoding="utf-8", newline="\n"
    )
    ref = f"{registry}/{unique_repo}/mcp/q-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    return ref


def test_qoder_project_install_round_trips_every_kind(
    grim_at, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Qoder reads Claude Code's shapes, so skill, rule and agent install
    byte-identical under ``.qoder/``, and the MCP entry lands in Qoder's own
    ``.qoder/settings.json`` — never the ``.mcp.json`` grim manages for Claude."""
    sk, ru, ag = _qoder_artifacts(unique_repo)
    (project_dir / ".qoder").mkdir()
    runner = grim_at(project_dir)
    mcp_ref = _release_qoder_mcp(runner, registry, unique_repo, tmp_path / "q_src")
    (project_dir / "grimoire.toml").write_text(
        f'[skills]\ncode-review = "{sk.fq}"\n[rules]\nrust-style = "{ru.fq}"\n'
        f'[agents]\nmy-agent = "{ag.fq}"\n[mcp]\nq-mcp = "{mcp_ref}"\n', encoding="utf-8", newline="\n"
    )
    runner.run("lock", check=False)

    # Detected from the `.qoder` marker alone — no `--client`.
    assert runner.json("context")["clients"] == ["qoder"]
    rows = runner.json("install")["items"]
    assert all(r["status"] == "installed" for r in rows), rows

    q = project_dir / ".qoder"
    assert (q / "skills/code-review/SKILL.md").read_text(encoding="utf-8") == _QODER_SKILL
    assert (q / "rules/rust-style.md").read_text(encoding="utf-8") == _QODER_RULE, "paths: is native"
    assert (q / "agents/my-agent.md").read_text(encoding="utf-8") == _QODER_AGENT
    entry = json.loads((q / "settings.json").read_text(encoding="utf-8"))["mcpServers"]["q-mcp"]
    assert entry == {"command": "grim", "args": ["mcp"], "timeout": 7000}, entry
    assert_not_exists(project_dir / ".mcp.json")

    # Uninstalling the MCP server removes only grim's member, never the file.
    runner.json("uninstall", "mcp", "q-mcp")
    assert json.loads((q / "settings.json").read_text(encoding="utf-8")).get("mcpServers", {}) == {}


def test_qoder_mcp_with_env_ref_is_skipped_with_a_warning(
    grim_at, project_dir: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """Qoder documents no ``${VAR}`` expansion, so a ref-bearing descriptor is
    skipped for Qoder rather than written as a broken literal, while Claude —
    which expands ``${VAR}`` natively — still registers it."""
    descriptor = tmp_path / "q_env" / "mcp" / "q-env.toml"
    descriptor.parent.mkdir(parents=True)
    descriptor.write_text(
        'description = "d"\n\n[server]\ntransport = "stdio"\ncommand = "grim"\n'
        'env = { TOKEN = "${GITHUB_TOKEN}" }\n', encoding="utf-8", newline="\n"
    )
    ref = f"{registry}/{unique_repo}/mcp/q-env:1.0.0"
    runner = grim_at(project_dir)
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    (project_dir / "grimoire.toml").write_text(f'[mcp]\nq-env = "{ref}"\n', encoding="utf-8", newline="\n")
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "claude,qoder", format="json")
    assert "skipped for qoder" in result.stderr.lower(), result.stderr
    assert_not_exists(project_dir / ".qoder/settings.json")
    assert "q-env" in json.loads((project_dir / ".mcp.json").read_text(encoding="utf-8"))["mcpServers"]


def test_qoder_global_install_honours_qoder_config_dir(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str, tmp_path: Path
) -> None:
    """``QODER_CONFIG_DIR`` replaces ``~/.qoder`` outright — no ``.qoder``
    segment appended — for every kind, MCP included. The only test that pins
    the variable's *name*; the unit tests inject values into the resolver."""
    from src.runner import GrimRunner

    sk, ru, ag = _qoder_artifacts(unique_repo)
    qoder_dir = grim_home.parent / "qoder_config"
    runner = GrimRunner(grim_binary, grim_home)
    runner.env["QODER_CONFIG_DIR"] = str(qoder_dir)
    mcp_ref = _release_qoder_mcp(runner, registry, unique_repo, tmp_path / "qg_src")
    (grim_home / "grimoire.toml").write_text(
        f'[skills]\ncode-review = "{sk.fq}"\n[rules]\nrust-style = "{ru.fq}"\n'
        f'[agents]\nmy-agent = "{ag.fq}"\n[mcp]\nq-mcp = "{mcp_ref}"\n', encoding="utf-8", newline="\n"
    )
    runner.json("lock", "--global")

    rows = runner.json("install", "--global", "--client", "qoder")["items"]
    assert all(r["status"] == "installed" for r in rows), rows

    assert_path_exists(qoder_dir / "skills/code-review/SKILL.md")
    assert_path_exists(qoder_dir / "rules/rust-style.md")
    assert_path_exists(qoder_dir / "agents/my-agent.md")
    assert "q-mcp" in json.loads((qoder_dir / "settings.json").read_text(encoding="utf-8"))["mcpServers"]
    assert_not_exists(qoder_dir / ".qoder")
    assert_not_exists(runner.home / ".qoder")


# --- The global [options].clients joins detection at project scope -----------


def _clients_lines(stderr: str) -> list[str]:
    """The ``clients: …`` lines grim wrote, whole."""
    return [ln for ln in stderr.splitlines() if ln.startswith("clients: ")]


def _set_global_clients(runner, value: str) -> None:
    runner.json("config", "set", "--global", "options.clients", value)


def test_global_clients_join_the_detected_clients_on_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The global list adds to what is detected when neither ``--client`` nor
    the project list selects anything, and ``install`` names each source."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    _set_global_clients(runner, "claude")
    runner.run("lock", check=False)

    result = runner.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")
    assert_path_exists(project_dir / ".opencode/skills/code-review/SKILL.md")
    assert _clients_lines(result.stderr) == ["clients: claude (global), opencode (detected)"], result.stderr


def test_global_clients_alone_select_when_nothing_is_detected(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Nothing detected plus a global list installs the list alone: no
    ``agents`` pool, and the line says ``global``, not ``fallback``."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    _set_global_clients(runner, "claude")
    runner.run("lock", check=False)

    result = runner.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")
    assert_not_exists(project_dir / ".agents/skills")
    assert _clients_lines(result.stderr) == ["clients: claude (global)"], result.stderr


def test_a_project_client_list_ignores_the_global_list(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=["opencode"])
    runner = grim_at(project_dir)
    _set_global_clients(runner, "claude")
    runner.run("lock", check=False)

    result = runner.run("install", check=False)

    assert result.returncode == 0, result.stderr
    assert_not_exists(project_dir / ".claude")
    assert_path_exists(project_dir / ".opencode/skills/code-review/SKILL.md")
    assert _clients_lines(result.stderr) == ["clients: opencode (project)"], result.stderr


def test_the_client_flag_ignores_the_global_list(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    _set_global_clients(runner, "claude")
    runner.run("lock", check=False)

    result = runner.run("install", "--client", "opencode", check=False)

    assert result.returncode == 0, result.stderr
    assert_not_exists(project_dir / ".claude")
    assert _clients_lines(result.stderr) == ["clients: opencode (--client)"], result.stderr


def test_install_without_a_global_list_names_detection_and_the_fallback(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    bare = runner.run("install", check=False)
    assert _clients_lines(bare.stderr) == ["clients: agents (fallback)"], bare.stderr

    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    detected = runner.run("install", check=False)
    assert _clients_lines(detected.stderr) == ["clients: opencode (detected)"], detected.stderr


def test_the_clients_line_adds_nothing_to_install_stdout(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The source line is a stderr note; JSON stdout stays one parseable
    document with no ``clients`` key, and ``--progress json`` stderr stays
    NDJSON."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)

    result = runner.run("--progress", "json", "install", format="json")

    assert set(json.loads(result.stdout)) == {"items", "settings_changes"}, result.stdout
    assert _clients_lines(result.stderr) == [], result.stderr
    for line in (ln for ln in result.stderr.splitlines() if ln.startswith("{")):
        json.loads(line)


def test_status_lists_a_global_clients_outputs_as_pending_before_install(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A client added to the global list after an install reads as pending in
    ``status`` (exit unchanged), says where it came from, and the next
    ``install`` writes it."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    assert_not_exists(project_dir / ".claude")
    _set_global_clients(runner, "claude")

    report = runner.json("status")
    row = next(r for r in report["items"] if r["name"] == "code-review")
    assert {o["client"] for o in row["outputs_pending"]} == {"claude"}, row
    assert "clients" not in report, "no new JSON key"
    assert row["clients_missing"] == [] and row["clients_extra"] == [], row

    plain = runner.run("status", check=False)
    assert plain.returncode == 0, plain.stderr
    assert _clients_lines(plain.stderr) == ["clients: claude (global), opencode (detected)"], plain.stderr
    assert _clients_lines(runner.run("status", format="json").stderr) == []

    runner.run("install", check=False)
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")


def test_status_reads_a_deleted_global_client_output_as_missing(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """A global-list client counts as active even without a marker, so its
    deleted output is ``missing``, not a client the user dropped."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    _set_global_clients(runner, "claude")
    runner.run("lock", check=False)
    runner.run("install", check=False)
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")
    # Without the global list `.claude` is no longer a detected client.
    shutil.rmtree(project_dir / ".claude")

    row = next(r for r in runner.json("status")["items"] if r["name"] == "code-review")
    assert row["state"] == "missing", row


_UNKNOWN_GLOBAL_CLIENT = '[options]\nclients = ["claude", "not-a-client"]\n'
_MALFORMED_GLOBAL = "[options\nclients = [\n"


def test_a_malformed_global_config_is_warned_about_and_changes_no_exit(
    grim_at, project_dir: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A hook-free project never needed the global config, so a broken one
    still installs: the global list reads as empty and the run says why."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL, encoding="utf-8", newline="\n")

    for command in ("install", "status"):
        result = runner.run(command, check=False)
        assert result.returncode == 0, (command, result.stderr)
        assert str(grim_home / "grimoire.toml") in result.stderr, (command, result.stderr)
        assert _clients_lines(result.stderr) == ["clients: opencode (detected)"], (command, result.stderr)


def test_an_unknown_global_client_is_warned_about_and_the_list_ignored(
    grim_at, project_dir: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    sk, ru = _publish_skill_and_rule(unique_repo)
    (project_dir / ".opencode").mkdir(parents=True, exist_ok=True)
    _build_toml(project_dir, sk.fq, ru.fq, clients=None)
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    (grim_home / "grimoire.toml").write_text(_UNKNOWN_GLOBAL_CLIENT, encoding="utf-8", newline="\n")

    for command in ("install", "status"):
        result = runner.run(command, check=False)
        assert result.returncode == 0, (command, result.stderr)
        assert "not-a-client" in result.stderr, (command, result.stderr)
        assert _clients_lines(result.stderr) == ["clients: opencode (detected)"], (command, result.stderr)
    assert_not_exists(project_dir / ".claude")


def test_update_warns_once_about_a_malformed_global_config(
    grim_at, project_dir: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """A lock that holds settings makes ``update`` read the global client list
    for the pre-save conflict check and again for the install target. The
    warning names the broken file once, the list reads as empty, and the exit
    is the one a hook-free project always had."""
    pkg = make_settings(f"{unique_repo}/team", "1", {"description": "team", "claude": {"model": "sonnet"}})
    write_config(project_dir, settings={"team": pkg.fq})
    runner = grim_at(project_dir)
    runner.run("lock")
    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL, encoding="utf-8", newline="\n")

    result = runner.run("update", check=False)

    assert result.returncode == 0, result.stderr
    broken = str(grim_home / "grimoire.toml")
    assert result.stderr.count(broken) == 1, result.stderr
    assert result.stderr.count("ignoring the global [options].clients") == 1, result.stderr


def test_search_warns_once_about_a_malformed_global_config(
    grim_at, project_dir: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    """``search --registry`` reads the global client list for its badges; the
    config is loaded once, so the broken file is named once."""
    write_config(project_dir)
    runner = grim_at(project_dir)
    (grim_home / "grimoire.toml").write_text(_MALFORMED_GLOBAL, encoding="utf-8", newline="\n")

    result = runner.run("search", "--registry", f"{registry}/{unique_repo}", check=False)

    assert result.returncode == 0, result.stderr
    broken = str(grim_home / "grimoire.toml")
    assert result.stderr.count(broken) == 1, result.stderr
    assert result.stderr.count("ignoring the global [options].clients") == 1, result.stderr


def test_the_global_list_does_not_widen_status_when_the_project_names_its_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """The global list only selects clients where the project's own list is
    empty. With ``clients = ["opencode"]`` a recorded Claude output whose
    directory is gone is a client the user dropped, not a missing file."""
    sk, ru = _publish_skill_and_rule(unique_repo)
    _build_toml(project_dir, sk.fq, ru.fq, clients=["claude", "opencode"])
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.run("install", check=False)
    assert_path_exists(project_dir / ".claude/skills/code-review/SKILL.md")
    _build_toml(project_dir, sk.fq, ru.fq, clients=["opencode"])
    _set_global_clients(runner, "claude")
    shutil.rmtree(project_dir / ".claude")

    row = next(r for r in runner.json("status")["items"] if r["name"] == "code-review")
    assert row["state"] == "installed", row


def test_a_global_run_labels_its_own_client_list_global(
    grim_binary: Path, grim_home: Path, registry: str, unique_repo: str
) -> None:
    from src.runner import GrimRunner

    sk = _skill_only(unique_repo)
    (grim_home / "grimoire.toml").write_text(
        f'[options]\nclients = ["claude"]\n\n[skills]\ncode-review = "{sk.fq}"\n', encoding="utf-8", newline="\n"
    )
    runner = GrimRunner(grim_binary, grim_home)
    runner.json("lock", "--global")

    result = runner.run("install", "--global", check=False)

    assert result.returncode == 0, result.stderr
    assert _clients_lines(result.stderr) == ["clients: claude (global)"], result.stderr
