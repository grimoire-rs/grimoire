# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 The Grimoire Authors
"""Per-client frontmatter rendering acceptance tests.

Skills whose ``metadata`` carries tool-namespaced keys (``claude.*``,
``opencode.*``, ``copilot.*``) are rendered differently per client at
install time:

- **claude**: namespaced keys lifted to native top-level typed fields
  (bool without quotes, enum/string verbatim); foreign keys dropped;
  plain metadata kept; `claude.*` keys gone from ``metadata``.
- **opencode** / **copilot**: clean universal ``SKILL.md`` — all tool keys
  stripped, plain metadata kept; clients with empty field registries
  produce a typo-guard warning for their own unknown keys but still
  succeed.

Rules carry a ``paths:`` frontmatter that maps per client:

- **claude**: verbatim copy (``paths:`` is native).
- **copilot**: ``.github/instructions/<name>.instructions.md`` whose
  frontmatter maps ``paths`` → ``applyTo`` and optional
  ``copilot.exclude-agent`` → ``excludeAgent``.
- **opencode**: ``.opencode/rules/<name>.md`` = provenance comment + body
  (no frontmatter); the workspace ``opencode.json`` gains a managed
  ``".opencode/rules/*.md"`` entry.

Integrity / drift: the rendered ``SKILL.md`` is hashed against its
*expected* bytes, so editing the installed file is still detected as drift
and refused on the next install; ``--force`` overwrites.

Publish-time gate: ``grim release`` (or ``grim build``) fails with exit
code 65 for a skill with a bad bool/enum value in a known namespaced
metadata key, or for a rule with a bad ``copilot.exclude-agent`` value.
An unknown key (e.g. typo) only warns (stderr) and succeeds.
"""
from __future__ import annotations

import json
from pathlib import Path

from src.helpers import make_artifact, write_config


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _write(p: Path, body: str) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(body)


def _canonical_skill_doc(name: str = "my-skill") -> str:
    """A canonical SKILL.md with claude.* namespaced metadata keys."""
    return (
        f"---\n"
        f"name: {name}\n"
        f"description: Test skill.\n"
        f"metadata:\n"
        f"  keywords: testing,automation\n"
        f'  claude.user-invocable: "false"\n'
        f'  claude.model: opus\n'
        f"---\n"
        f"# {name}\n"
        f"Body text.\n"
    )


def _push_namespaced_skill(
    unique_repo: str, name: str = "my-skill"
) -> "src.registry.PublishedArtifact":
    return make_artifact(
        f"{unique_repo}/{name}",
        "skill",
        {f"{name}/SKILL.md": _canonical_skill_doc(name)},
        tag="v1",
    )


def _push_rule_with_paths_and_exclude(unique_repo: str) -> "src.registry.PublishedArtifact":
    """Rule with ``paths:`` and ``copilot.exclude-agent`` authored under ``metadata:``."""
    doc = (
        "---\n"
        'paths: ["**/*.rs"]\n'
        "metadata:\n"
        "  copilot.exclude-agent: code-review\n"
        "---\n"
        "# Rust Style\n"
        "Use 4 spaces.\n"
    )
    return make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": doc},
        tag="v1",
    )


def _push_rule_paths_only(unique_repo: str) -> "src.registry.PublishedArtifact":
    """Rule with only ``paths:``, no vendor-namespaced metadata keys."""
    doc = (
        "---\n"
        'paths: ["**/*.rs"]\n'
        "---\n"
        "# Rust Style\n"
        "Use 4 spaces.\n"
    )
    return make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": doc},
        tag="v1",
    )


def _push_rule_with_paths_and_keywords_and_vendor(unique_repo: str) -> "src.registry.PublishedArtifact":
    """Rule with ``paths:``, a plain ``keywords`` key, and a vendor metadata key.

    Used to verify that Claude's cleaned render keeps ``paths:`` and plain
    metadata keys but drops ``copilot.*`` vendor keys.
    """
    doc = (
        "---\n"
        'paths: ["**/*.rs"]\n'
        "keywords: rust,style\n"
        "metadata:\n"
        "  copilot.exclude-agent: code-review\n"
        "---\n"
        "# Rust Style\n"
        "Use 4 spaces.\n"
    )
    return make_artifact(
        f"{unique_repo}/rust-style",
        "rule",
        {"rust-style.md": doc},
        tag="v1",
    )


# ---------------------------------------------------------------------------
# Skill rendering — one release, three client installs
# ---------------------------------------------------------------------------


def test_claude_skill_lifts_namespaced_keys_to_native_typed_fields(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Claude install: ``claude.*`` metadata keys become native typed fields.

    - ``claude.user-invocable: "false"`` lifts to ``user-invocable: false``
      (a real YAML bool — no quotes in the line).
    - ``claude.model: opus`` lifts to ``model: opus``.
    - ``claude.*`` keys are gone from ``metadata``.
    - Plain metadata key ``keywords`` is preserved.
    """
    _push_namespaced_skill(unique_repo)
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "claude")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    skill_md = project_dir / ".claude/skills/my-skill/SKILL.md"
    assert skill_md.is_file(), "SKILL.md must be installed for claude"
    text = skill_md.read_text()

    # The bool is a native YAML bool — no quotes around `false`.
    assert "user-invocable: false" in text, (
        f"expected native bool `user-invocable: false` in:\n{text}"
    )
    # The quoted form must NOT appear.
    assert "'false'" not in text and '"false"' not in text, (
        f"quoted bool must not appear in rendered skill:\n{text}"
    )
    # The model string lifts to top-level.
    assert "model: opus" in text, f"expected `model: opus` in:\n{text}"

    # No namespaced keys remain in the document.
    assert "claude." not in text, (
        f"claude.* namespaced keys must be gone from the rendered skill:\n{text}"
    )
    # Plain metadata passes through.
    assert "keywords: testing,automation" in text, (
        f"expected plain metadata key `keywords` in:\n{text}"
    )


def test_opencode_skill_is_clean_universal_no_tool_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """OpenCode install: all ``claude.*`` keys dropped; plain metadata kept."""
    _push_namespaced_skill(unique_repo)
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "opencode")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    skill_md = project_dir / ".opencode/skills/my-skill/SKILL.md"
    assert skill_md.is_file(), "SKILL.md must be installed for opencode"
    text = skill_md.read_text()

    assert "claude." not in text, (
        f"claude.* keys must be gone for opencode:\n{text}"
    )
    # Claude-native lifted fields must not appear either.
    assert "user-invocable" not in text, (
        f"user-invocable must not appear in opencode SKILL.md:\n{text}"
    )
    assert "model:" not in text, (
        f"model must not appear in opencode SKILL.md:\n{text}"
    )
    assert "keywords: testing,automation" in text, (
        f"plain metadata must be preserved for opencode:\n{text}"
    )


def test_copilot_skill_is_clean_universal_no_tool_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Copilot install: all ``claude.*`` keys dropped; plain metadata kept."""
    _push_namespaced_skill(unique_repo)
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "copilot")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    skill_md = project_dir / ".github/skills/my-skill/SKILL.md"
    assert skill_md.is_file(), "SKILL.md must be installed for copilot"
    text = skill_md.read_text()

    assert "claude." not in text, (
        f"claude.* keys must be gone for copilot:\n{text}"
    )
    assert "user-invocable" not in text, (
        f"user-invocable must not appear in copilot SKILL.md:\n{text}"
    )
    assert "keywords: testing,automation" in text, (
        f"plain metadata must be preserved for copilot:\n{text}"
    )


def test_opencode_and_copilot_skill_renders_are_byte_identical(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """OpenCode and Copilot skill renders are byte-identical (unified universal render).

    Both clients have empty skill field registries so they both emit only
    the universal agentskills fields; the ``claude.*`` keys are dropped and
    plain metadata is kept for both.  The installed ``SKILL.md`` bytes must
    be identical across the two clients.
    """
    _push_namespaced_skill(unique_repo)
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "opencode,copilot")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    oc_skill = project_dir / ".opencode/skills/my-skill/SKILL.md"
    cp_skill = project_dir / ".github/skills/my-skill/SKILL.md"
    assert oc_skill.is_file(), "SKILL.md must exist for opencode"
    assert cp_skill.is_file(), "SKILL.md must exist for copilot"

    oc_bytes = oc_skill.read_bytes()
    cp_bytes = cp_skill.read_bytes()
    assert oc_bytes == cp_bytes, (
        "OpenCode and Copilot skill SKILL.md must be byte-identical "
        "(unified universal render):\n"
        f"  opencode: {oc_skill.read_text()!r}\n"
        f"  copilot:  {cp_skill.read_text()!r}"
    )


def test_sibling_files_install_verbatim_for_all_clients(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Non-SKILL.md sibling files are always copied verbatim (no rendering).

    Uses ``--client claude,opencode,copilot`` to install to all three in
    one pass, then verifies each client's sibling file separately.
    """
    make_artifact(
        f"{unique_repo}/my-skill",
        "skill",
        {
            "my-skill/SKILL.md": _canonical_skill_doc("my-skill"),
            "my-skill/reference.md": "# Reference\nVerbatim content.\n",
        },
        tag="v1",
    )
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "claude,opencode,copilot")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    for client, root in (
        ("claude", project_dir / ".claude/skills/my-skill"),
        ("opencode", project_dir / ".opencode/skills/my-skill"),
        ("copilot", project_dir / ".github/skills/my-skill"),
    ):
        ref = root / "reference.md"
        assert ref.is_file(), f"reference.md must exist for {client}"
        assert ref.read_text() == "# Reference\nVerbatim content.\n", (
            f"sibling reference.md must be verbatim for {client}"
        )


# ---------------------------------------------------------------------------
# Skill drift: rendered SKILL.md is integrity-anchored
# ---------------------------------------------------------------------------


def test_rendered_skill_drift_refused_then_forced(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Editing an installed rendered SKILL.md is detected as drift.

    A plain ``grim install`` refuses (exit 65); ``--force`` overwrites.
    This mirrors the integrity tests for plain canonical files.
    """
    _push_namespaced_skill(unique_repo)
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.json("install", "--client", "claude")

    installed = project_dir / ".claude/skills/my-skill/SKILL.md"
    original_text = installed.read_text()
    installed.write_text("hand edited\n")

    # Without --force: must refuse with exit 65.
    refused = runner.run("install", "--client", "claude", check=False)
    assert refused.returncode == 65, (
        f"modified rendered skill must refuse with 65, got "
        f"{refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", (
        "a refused install must not overwrite the user's edit"
    )

    # With --force: must overwrite and restore the rendered content.
    forced = runner.run("install", "--client", "claude", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    restored = installed.read_text()
    # The restored content must carry the native lifted field (deterministic).
    assert "user-invocable: false" in restored, (
        f"forced install must restore the rendered content:\n{restored}"
    )
    assert restored != "hand edited\n"


# ---------------------------------------------------------------------------
# Rule rendering — per client
# ---------------------------------------------------------------------------


def test_claude_rule_is_verbatim_paths_frontmatter_intact(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Claude: rule installed verbatim; ``paths:`` frontmatter is native."""
    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "claude")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    rule_md = project_dir / ".claude/rules/rust-style.md"
    assert rule_md.is_file(), "rule must exist for claude"
    text = rule_md.read_text()
    assert "paths:" in text, "paths: frontmatter must be present for claude"
    assert "Use 4 spaces." in text


def test_claude_rule_with_vendor_metadata_is_cleaned(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Claude: rule WITH tool-namespaced metadata is rendered (not verbatim).

    A rule carrying vendor-namespaced keys in its ``metadata:`` map triggers a
    cleaned render for Claude:

    - The ``copilot.*`` (foreign) key is dropped silently.
    - The ``paths:`` scoping and plain top-level keys (``keywords``) are kept.
    - The Markdown body is preserved verbatim.
    - No provenance comment is inserted (Claude is a canonical client).
    - The installed file differs from the source bytes, so it is
      integrity-anchored (``generated: true`` in the install record).
    """
    _push_rule_with_paths_and_keywords_and_vendor(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "claude")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    rule_md = project_dir / ".claude/rules/rust-style.md"
    assert rule_md.is_file(), "rule must exist for claude"
    text = rule_md.read_text()

    # The foreign copilot vendor key must be gone from the installed file.
    assert "copilot.exclude-agent" not in text, (
        f"copilot.exclude-agent must be stripped from the claude render:\n{text}"
    )
    # The path scoping must survive — it is native to Claude.
    assert "paths:" in text, f"paths: must be present in cleaned claude render:\n{text}"
    assert "**/*.rs" in text, f"glob must be present in cleaned claude render:\n{text}"
    # A plain top-level key (keywords) must be preserved.
    assert "keywords: rust,style" in text, (
        f"plain metadata keys must be kept in cleaned claude render:\n{text}"
    )
    # No provenance comment — Claude does not inject one.
    assert "<!-- generated by grim from" not in text, (
        f"provenance comment must NOT appear in claude rule render:\n{text}"
    )
    # Body must be intact.
    assert "Use 4 spaces." in text


def test_copilot_rule_maps_paths_to_apply_to(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Copilot: ``.instructions.md`` has ``applyTo`` frontmatter mapping ``paths``."""
    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "copilot")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    instr = project_dir / ".github/instructions/rust-style.instructions.md"
    assert instr.is_file(), "instructions.md must exist for copilot"
    text = instr.read_text()

    # Frontmatter: applyTo mapping, no paths:.
    assert 'applyTo: "**/*.rs"' in text, (
        f"expected applyTo mapping in copilot instructions:\n{text}"
    )
    assert "paths:" not in text, (
        f"paths: must not appear in copilot instructions:\n{text}"
    )
    # Provenance comment: appears after the closing --- fence.
    assert "<!-- generated by grim from " in text
    assert "edits will be overwritten -->" in text, (
        f"provenance comment must carry the overwrite warning:\n{text}"
    )
    # Provenance comment is the first content after the frontmatter fence.
    lines = text.splitlines()
    fence_close_idx = next(
        i for i, l in enumerate(lines) if i > 0 and l.strip() == "---"
    )
    provenance_line = lines[fence_close_idx + 1]
    assert provenance_line.startswith("<!-- generated by grim from "), (
        f"line immediately after closing --- must be the provenance comment, got: {provenance_line!r}"
    )
    # Body preserved.
    assert "Use 4 spaces." in text


def test_copilot_rule_with_exclude_agent_emits_exclude_agent_frontmatter(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``copilot.exclude-agent: code-review`` maps to ``excludeAgent:`` in .instructions.md."""
    _push_rule_with_paths_and_exclude(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "copilot")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    instr = project_dir / ".github/instructions/rust-style.instructions.md"
    text = instr.read_text()
    assert 'excludeAgent: "code-review"' in text, (
        f"expected excludeAgent in copilot instructions:\n{text}"
    )
    assert 'applyTo: "**/*.rs"' in text, "applyTo must still be present"
    assert "copilot.exclude-agent" not in text, (
        "raw copilot.exclude-agent must not appear in the output"
    )


def test_opencode_rule_strips_frontmatter_adds_provenance(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """OpenCode: rule file has no frontmatter; body prefixed with provenance comment."""
    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "opencode")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    rule_md = project_dir / ".opencode/rules/rust-style.md"
    assert rule_md.is_file(), "rule must exist for opencode"
    text = rule_md.read_text()

    assert text.startswith("<!-- generated by grim from "), (
        f"opencode rule must start with provenance comment:\n{text}"
    )
    assert "paths:" not in text, (
        f"paths: must be stripped for opencode:\n{text}"
    )
    assert "Use 4 spaces." in text


# ---------------------------------------------------------------------------
# OpenCode: opencode.json registration and preservation
# ---------------------------------------------------------------------------


def test_opencode_rule_install_registers_glob_in_opencode_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Installing an opencode rule adds the managed glob to opencode.json."""
    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "opencode")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    cfg_path = project_dir / "opencode.json"
    assert cfg_path.is_file(), "opencode.json must be created after opencode rule install"
    cfg = json.loads(cfg_path.read_text())
    assert "instructions" in cfg, f"opencode.json must have instructions key: {cfg}"
    assert ".opencode/rules/*.md" in cfg["instructions"], (
        f"managed glob must be in instructions: {cfg['instructions']}"
    )


def test_opencode_rule_install_preserves_existing_opencode_json_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Pre-existing keys in opencode.json are preserved; only the managed glob is added."""
    # Write a pre-existing opencode.json with a model key and an existing
    # unmanaged instructions entry.
    pre_existing = {
        "model": "anthropic/claude-opus-4",
        "instructions": ["CONTRIBUTING.md"],
    }
    (project_dir / "opencode.json").write_text(json.dumps(pre_existing))

    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "opencode")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    cfg = json.loads((project_dir / "opencode.json").read_text())
    # The pre-existing model key must still be there.
    assert cfg.get("model") == "anthropic/claude-opus-4", (
        f"model key must be preserved: {cfg}"
    )
    instructions = cfg["instructions"]
    assert "CONTRIBUTING.md" in instructions, (
        f"pre-existing instructions entry must be preserved: {instructions}"
    )
    assert ".opencode/rules/*.md" in instructions, (
        f"managed glob must be added: {instructions}"
    )


def test_opencode_rule_uninstall_removes_glob_from_opencode_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Uninstalling the last opencode rule removes the managed glob entry.

    Other keys in opencode.json (including other instructions entries) are
    preserved; only the managed glob is removed.
    """
    # Pre-seed opencode.json with an unmanaged instructions entry.
    pre_existing = {
        "model": "anthropic/claude-opus-4",
        "instructions": ["CONTRIBUTING.md"],
    }
    (project_dir / "opencode.json").write_text(json.dumps(pre_existing))

    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.json("install", "--client", "opencode")

    # Verify the glob was added.
    cfg = json.loads((project_dir / "opencode.json").read_text())
    assert ".opencode/rules/*.md" in cfg["instructions"]

    # Uninstall the rule.
    out = runner.json("uninstall", "rule", "rust-style")
    assert out["status"] == "uninstalled"

    # The rule file must be gone.
    rule_md = project_dir / ".opencode/rules/rust-style.md"
    assert not rule_md.exists(), "rule file must be removed after uninstall"

    # opencode.json: managed glob gone; pre-existing entries preserved.
    cfg_after = json.loads((project_dir / "opencode.json").read_text())
    assert ".opencode/rules/*.md" not in cfg_after.get("instructions", []), (
        f"managed glob must be removed after last rule uninstall: {cfg_after}"
    )
    assert cfg_after.get("model") == "anthropic/claude-opus-4", (
        f"model key must still be present: {cfg_after}"
    )
    assert "CONTRIBUTING.md" in cfg_after.get("instructions", []), (
        f"pre-existing CONTRIBUTING.md must still be in instructions: {cfg_after}"
    )


def test_update_prune_with_client_subset_removes_opencode_glob(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An OpenCode rule pruned by ``update --client claude`` drops the glob.

    Regression: the post-update vendor config sync only covered the clients
    selected for the run, so an orphan recorded for another client was
    pruned from disk while its managed ``instructions`` entry lingered in
    ``opencode.json``, pointing at a deleted file.
    """
    _push_rule_paths_only(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "opencode")
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows
    cfg = json.loads((project_dir / "opencode.json").read_text())
    assert ".opencode/rules/*.md" in cfg["instructions"]

    # Drop the rule from the declaration, then update for a *different*
    # client: the prune pass removes the orphaned OpenCode rule, and the
    # config sync must converge opencode.json even though opencode was not
    # in this run's client set.
    write_config(project_dir)
    rows = runner.json("update", "--client", "claude")
    assert any(r["action"] == "removed" for r in rows), rows
    assert not (project_dir / ".opencode/rules/rust-style.md").exists(), (
        "pruned rule file must be deleted"
    )
    cfg_after = json.loads((project_dir / "opencode.json").read_text())
    assert ".opencode/rules/*.md" not in cfg_after.get("instructions", []), (
        f"managed glob must be removed when the pruned rule was the last opencode rule: {cfg_after}"
    )


# ---------------------------------------------------------------------------
# Publish-time gate — grim release must fail with exit 65 for bad metadata
# ---------------------------------------------------------------------------


def test_release_fails_with_bad_bool_in_skill_metadata(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``claude.user-invocable: "maybe"`` is an invalid bool; release exits 65."""
    skill_dir = project_dir / "bad-skill"
    _write(
        skill_dir / "SKILL.md",
        '---\nname: bad-skill\ndescription: d\nmetadata:\n  claude.user-invocable: "maybe"\n---\nbody\n',
    )
    repo = f"{registry}/{unique_repo}/bad-skill"
    runner = grim_at(project_dir)
    result = runner.run("release", str(skill_dir), f"{repo}:1.0.0", check=False)
    assert result.returncode == 65, (
        f"bad bool in skill metadata must exit 65, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_release_fails_with_bad_enum_in_skill_metadata(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``claude.effort: "warp"`` is an invalid enum value; release exits 65."""
    skill_dir = project_dir / "bad-skill"
    _write(
        skill_dir / "SKILL.md",
        '---\nname: bad-skill\ndescription: d\nmetadata:\n  claude.effort: "warp"\n---\nbody\n',
    )
    repo = f"{registry}/{unique_repo}/bad-skill"
    runner = grim_at(project_dir)
    result = runner.run("release", str(skill_dir), f"{repo}:1.0.0", check=False)
    assert result.returncode == 65, (
        f"bad enum in skill metadata must exit 65, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_release_fails_with_bad_exclude_agent_in_rule(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """``copilot.exclude-agent: everything`` under ``metadata:`` is invalid; release exits 65.

    The vendor key must be authored inside the ``metadata:`` map for the
    publish-time gate to fire.  A top-level ``copilot.exclude-agent`` is not
    projected and only earns a migration warning — it does not fail the build.
    """
    rule_file = project_dir / "bad-rule.md"
    _write(
        rule_file,
        '---\npaths: ["**/*.rs"]\nmetadata:\n  copilot.exclude-agent: everything\n---\nbody\n',
    )
    repo = f"{registry}/{unique_repo}/bad-rule"
    runner = grim_at(project_dir)
    result = runner.run("release", str(rule_file), f"{repo}:1.0.0", check=False)
    assert result.returncode == 65, (
        f"bad copilot.exclude-agent in metadata must exit 65, got {result.returncode}; "
        f"{result.stderr}"
    )


def test_release_warns_but_succeeds_for_unknown_namespaced_key(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """An unknown key like ``claude.modle`` (typo) only warns and succeeds.

    The artifact is pushed successfully; the warning appears on stderr.
    """
    skill_dir = project_dir / "warn-skill"
    _write(
        skill_dir / "SKILL.md",
        '---\nname: warn-skill\ndescription: d\nmetadata:\n  claude.modle: opus\n---\nbody\n',
    )
    repo = f"{registry}/{unique_repo}/warn-skill"
    runner = grim_at(project_dir)
    result = runner.run("release", str(skill_dir), f"{repo}:1.0.0", check=False)
    assert result.returncode == 0, (
        f"unknown key must only warn and succeed, got {result.returncode}; "
        f"{result.stderr}"
    )
    # The warning must mention the unknown key.
    assert "claude.modle" in result.stderr, (
        f"expected typo-guard warning for 'claude.modle' in stderr:\n{result.stderr}"
    )
