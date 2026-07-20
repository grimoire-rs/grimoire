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
import re
from pathlib import Path

from src.helpers import make_artifact, write_config


def _loads_jsonc(text: str) -> dict:
    """Parse JSONC by stripping ``//``-to-end-of-line comments before
    ``json.loads``. Test fixtures keep comments on their own lines (never
    inside a string), so a plain substitution is safe here."""
    return json.loads(re.sub(r"//[^\n]*", "", text))


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
    rows = runner.json("install", "--client", "claude")["items"]
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
    rows = runner.json("install", "--client", "opencode")["items"]
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
    rows = runner.json("install", "--client", "copilot")["items"]
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
    rows = runner.json("install", "--client", "opencode,copilot")["items"]
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
    rows = runner.json("install", "--client", "claude,opencode,copilot")["items"]
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
    rows = runner.json("install", "--client", "claude")["items"]
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
    rows = runner.json("install", "--client", "claude")["items"]
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
    rows = runner.json("install", "--client", "copilot")["items"]
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
    rows = runner.json("install", "--client", "copilot")["items"]
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
    rows = runner.json("install", "--client", "opencode")["items"]
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
    rows = runner.json("install", "--client", "opencode")["items"]
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
    rows = runner.json("install", "--client", "opencode")["items"]
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
    rows = runner.json("install", "--client", "opencode")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows
    cfg = json.loads((project_dir / "opencode.json").read_text())
    assert ".opencode/rules/*.md" in cfg["instructions"]

    # Drop the rule from the declaration, then update for a *different*
    # client: the prune pass removes the orphaned OpenCode rule, and the
    # config sync must converge opencode.json even though opencode was not
    # in this run's client set.
    write_config(project_dir)
    rows = runner.json("update", "--client", "claude")["items"]
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


# ===========================================================================
# Wave-1 vendor render surfaces (Cursor, Kiro, Junie, Gemini, Zed, Amp)
#
# Contract sources: adr_vendor_wave_expansion.md (§1 mapping table) +
# research_vendor_verification_{cursor_kiro,junie_gemini,zed_amp}.md
# (live-verified 2026-07-19).  Each vendor's native path and transform is
# pinned there; these tests encode the WHAT, not the stub bodies.
# ===========================================================================


def _push_rule(unique_repo: str, name: str, doc: str) -> "src.registry.PublishedArtifact":
    return make_artifact(f"{unique_repo}/{name}", "rule", {f"{name}.md": doc}, tag="v1")


def _push_multi_path_rule(unique_repo: str, name: str = "rust-style") -> "src.registry.PublishedArtifact":
    """A scoped rule with two glob paths (tests comma-joining, not YAML array)."""
    doc = (
        "---\n"
        'paths: ["**/*.rs", "**/*.md"]\n'
        "---\n"
        "# Rust Style\n"
        "Use 4 spaces.\n"
    )
    return _push_rule(unique_repo, name, doc)


def _push_unscoped_rule(unique_repo: str, name: str = "always-rule") -> "src.registry.PublishedArtifact":
    """A rule carrying no ``paths:`` scoping (applies everywhere)."""
    return _push_rule(unique_repo, name, "# Always On\nApplies everywhere.\n")


def _push_agent(
    unique_repo: str, name: str, extra_metadata: str, model: str = "sonnet"
) -> "src.registry.PublishedArtifact":
    """Push an agent whose ``metadata:`` map carries vendor-namespaced keys."""
    doc = (
        f"---\n"
        f"name: {name}\n"
        f"description: A test agent.\n"
        f"model: {model}\n"
        f"tools: Bash,Read\n"
        f"metadata:\n"
        f"{extra_metadata}"
        f"---\n"
        f"# {name}\n"
        f"Agent body text.\n"
    )
    return make_artifact(f"{unique_repo}/{name}", "agent", {f"{name}.md": doc}, tag="v1")


_MCP_STDIO_DESCRIPTOR = (
    'description = "A stdio MCP server."\n\n'
    "[server]\n"
    'transport = "stdio"\n'
    'command = "grim"\n'
    'args = ["mcp"]\n'
)

_MCP_HTTP_DESCRIPTOR = (
    'description = "A remote HTTP MCP server."\n\n'
    "[server]\n"
    'transport = "http"\n'
    'url = "https://api.example.com/mcp"\n'
)


def _add_mcp(
    runner, project_dir: Path, registry: str, unique_repo: str, body: str = _MCP_STDIO_DESCRIPTOR
) -> str:
    """Release an MCP descriptor and declare it (locked, not yet installed).

    Mirrors the release→add pattern in ``test_mcp_artifact.py``; the caller
    then runs ``install --client <vendor>`` to exercise a single vendor's
    MCP config splice in isolation.
    """
    descriptor = project_dir / "src" / "mcp" / "grim-mcp.toml"
    descriptor.parent.mkdir(parents=True, exist_ok=True)
    descriptor.write_text(body)
    ref = f"{registry}/{unique_repo}/mcp/grim-mcp:1.0.0"
    runner.json("release", str(descriptor), ref, "--kind", "mcp")
    write_config(project_dir)
    runner.json("add", "--no-install", ref)
    return ref


# ---------------------------------------------------------------------------
# Cursor
# ---------------------------------------------------------------------------


def test_cursor_scoped_rule_maps_paths_to_comma_string_globs(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Cursor scoped rule → ``.cursor/rules/<name>.mdc`` with ``globs`` as a
    comma-separated STRING (not a YAML array) and ``alwaysApply: false``."""
    _push_multi_path_rule(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "cursor")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    mdc = project_dir / ".cursor/rules/rust-style.mdc"
    assert mdc.is_file(), "Cursor rule must materialize at .cursor/rules/<name>.mdc"
    text = mdc.read_text()

    # Both globs on a SINGLE `globs:` line proves a comma-joined string, not a
    # YAML sequence (which would put each glob on its own `- ` line). Tolerant
    # of `,` vs `, ` spacing.
    globs_line = next(
        (l for l in text.splitlines() if l.strip().startswith("globs:")), ""
    )
    assert "**/*.rs" in globs_line and "**/*.md" in globs_line, (
        f"Cursor globs must be one comma-joined string line, got globs line {globs_line!r}:\n{text}"
    )
    assert "," in globs_line, f"multi-path globs must be comma-separated:\n{text}"
    assert "alwaysApply: false" in text, f"scoped rule must set alwaysApply: false:\n{text}"
    # `paths:` is not a Cursor-native key.
    assert "paths:" not in text, f"paths: must not survive into the Cursor render:\n{text}"
    assert "Use 4 spaces." in text


def test_cursor_unscoped_rule_sets_always_apply_true(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Cursor unscoped rule (no ``paths:``) → ``alwaysApply: true``."""
    _push_unscoped_rule(unique_repo)
    write_config(project_dir, rules={"always-rule": f"{registry}/{unique_repo}/always-rule:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "cursor")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    mdc = project_dir / ".cursor/rules/always-rule.mdc"
    assert mdc.is_file(), "Cursor rule must materialize at .cursor/rules/<name>.mdc"
    text = mdc.read_text()
    assert "alwaysApply: true" in text, f"unscoped rule must set alwaysApply: true:\n{text}"
    assert "Applies everywhere." in text


def test_cursor_agent_lifts_cursor_namespaced_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Cursor agent → ``.cursor/agents/<name>.md`` with ``cursor.*`` keys lifted
    to native typed frontmatter; the raw namespaced keys are gone."""
    _push_agent(
        unique_repo,
        "cur-agent",
        extra_metadata=(
            "  cursor.model: opus\n"
            '  cursor.readonly: "true"\n'
            '  cursor.is-background: "false"\n'
        ),
    )
    write_config(project_dir, agents={"cur-agent": f"{registry}/{unique_repo}/cur-agent:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "cursor")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    agent_md = project_dir / ".cursor/agents/cur-agent.md"
    assert agent_md.is_file(), "Cursor agent must materialize at .cursor/agents/<name>.md"
    text = agent_md.read_text()

    # cursor.model overrides the common model (claude.model precedent).
    assert "model: opus" in text, f"cursor.model must lift to native model:\n{text}"
    # Bool keys lift to native UNQUOTED YAML bools.
    assert "readonly: true" in text, f"cursor.readonly must lift to a native bool:\n{text}"
    assert "background: false" in text, (
        f"cursor.is-background must lift to a native bool (unquoted):\n{text}"
    )
    # No raw namespaced keys survive.
    assert "cursor." not in text, f"cursor.* namespaced keys must be gone:\n{text}"
    assert "Agent body text." in text


def test_cursor_skill_strips_foreign_claude_key(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Cursor skill → ``.cursor/skills/<name>/SKILL.md`` with a foreign
    ``claude.*`` metadata key stripped (Cursor's registry is empty in wave 1)."""
    _push_namespaced_skill(unique_repo)
    write_config(project_dir, skills={"my-skill": f"{registry}/{unique_repo}/my-skill:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "cursor")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    skill_md = project_dir / ".cursor/skills/my-skill/SKILL.md"
    assert skill_md.is_file(), "Cursor skill must materialize at .cursor/skills/<name>/SKILL.md"
    text = skill_md.read_text()
    assert "claude." not in text, f"foreign claude.* keys must be stripped for cursor:\n{text}"
    # Claude-native lifted field must not leak into the Cursor render either.
    assert "user-invocable" not in text, f"claude native field must not appear:\n{text}"
    # Plain metadata survives.
    assert "keywords: testing,automation" in text, f"plain metadata must be kept:\n{text}"


def test_cursor_mcp_entry_lands_in_cursor_mcp_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Cursor MCP → ``.cursor/mcp.json``, container key ``mcpServers``,
    stdio entry carries ``type: "stdio"``."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "cursor")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".cursor/mcp.json"
    assert cfg.is_file(), "Cursor MCP entry must land in .cursor/mcp.json"
    entry = json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["command"] == "grim"
    assert entry["type"] == "stdio", f"Cursor stdio entry needs type: stdio; got {entry}"


# ---------------------------------------------------------------------------
# Kiro
# ---------------------------------------------------------------------------


def test_kiro_scoped_rule_maps_to_filematch_array(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Kiro scoped rule → ``.kiro/steering/<name>.md`` with
    ``inclusion: fileMatch`` and a ``fileMatchPattern`` ARRAY."""
    _push_multi_path_rule(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "kiro")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    steering = project_dir / ".kiro/steering/rust-style.md"
    assert steering.is_file(), "Kiro rule must materialize at .kiro/steering/<name>.md"
    text = steering.read_text()
    assert "inclusion: fileMatch" in text, f"scoped Kiro rule needs inclusion: fileMatch:\n{text}"
    assert "fileMatchPattern:" in text, f"scoped Kiro rule needs fileMatchPattern:\n{text}"
    # Array form — tolerant of YAML block (`fileMatchPattern:\n  - **/*.rs`)
    # and flow (`fileMatchPattern: ["**/*.rs"]`) sequences; rejects a bare
    # scalar string value.
    fm_tail = text.split("fileMatchPattern:", 1)[1].splitlines()
    same_line = fm_tail[0].strip()
    next_line = fm_tail[1].strip() if len(fm_tail) > 1 else ""
    assert same_line.startswith("[") or next_line.startswith("-"), (
        f"fileMatchPattern must be an array (flow or block), not a scalar:\n{text}"
    )
    assert "**/*.rs" in text, f"the scoped glob must survive into fileMatchPattern:\n{text}"
    assert "Use 4 spaces." in text


def test_kiro_unscoped_rule_sets_inclusion_always(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Kiro unscoped rule → ``inclusion: always`` (project scope, no warning)."""
    _push_unscoped_rule(unique_repo)
    write_config(project_dir, rules={"always-rule": f"{registry}/{unique_repo}/always-rule:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    result = runner.run("install", "--client", "kiro", format="json", log_level="warn")
    rows = json.loads(result.stdout)["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    steering = project_dir / ".kiro/steering/always-rule.md"
    assert steering.is_file(), "Kiro rule must materialize at .kiro/steering/<name>.md"
    text = steering.read_text()
    # Either inclusion: always is emitted, or frontmatter is omitted entirely
    # (always is Kiro's default) — but never a fileMatch.
    assert "fileMatch" not in text, f"unscoped Kiro rule must not carry fileMatch:\n{text}"
    assert "Applies everywhere." in text
    # Project scope: the global inert-#9176 warning must NOT fire.
    assert "9176" not in result.stderr, (
        f"project-scope Kiro rule must not emit the global-inertness warning:\n{result.stderr}"
    )


def test_kiro_mcp_entry_lands_in_kiro_settings_mcp_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Kiro MCP → ``.kiro/settings/mcp.json``, container key ``mcpServers``."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "kiro")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".kiro/settings/mcp.json"
    assert cfg.is_file(), "Kiro MCP entry must land in .kiro/settings/mcp.json"
    entry = json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["command"] == "grim"


# ---------------------------------------------------------------------------
# Junie
# ---------------------------------------------------------------------------


def test_junie_mcp_entry_lands_in_junie_mcp_mcp_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Junie MCP → ``.junie/mcp/mcp.json``, container key ``mcpServers``."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "junie")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".junie/mcp/mcp.json"
    assert cfg.is_file(), "Junie MCP entry must land in .junie/mcp/mcp.json"
    entry = json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["command"] == "grim"


# ---------------------------------------------------------------------------
# Gemini
# ---------------------------------------------------------------------------


def test_gemini_agent_lifts_gemini_namespaced_keys(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Gemini agent → ``.gemini/agents/<name>.md`` with ``gemini.*`` keys lifted."""
    _push_agent(
        unique_repo,
        "gem-agent",
        extra_metadata=(
            "  gemini.model: gemini-2.5-pro\n"
            '  gemini.temperature: "0.5"\n'
            '  gemini.max-turns: "10"\n'
        ),
    )
    write_config(project_dir, agents={"gem-agent": f"{registry}/{unique_repo}/gem-agent:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    rows = runner.json("install", "--client", "gemini")["items"]
    assert all(r["status"] in ("installed", "unchanged") for r in rows), rows

    agent_md = project_dir / ".gemini/agents/gem-agent.md"
    assert agent_md.is_file(), "Gemini agent must materialize at .gemini/agents/<name>.md"
    text = agent_md.read_text()
    assert "model: gemini-2.5-pro" in text, f"gemini.model must lift to native model:\n{text}"
    # Numeric key lifts to a native UNQUOTED number.
    assert "temperature: 0.5" in text, f"gemini.temperature must lift to a native float:\n{text}"
    assert "gemini." not in text, f"gemini.* namespaced keys must be gone:\n{text}"
    assert "Agent body text." in text


def test_gemini_mcp_stdio_entry_lands_in_gemini_settings_json(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Gemini MCP (stdio) → ``.gemini/settings.json``, key ``mcpServers``,
    stdio transport uses ``command``."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "gemini")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".gemini/settings.json"
    assert cfg.is_file(), "Gemini MCP entry must land in .gemini/settings.json"
    entry = json.loads(cfg.read_text())["mcpServers"]["grim-mcp"]
    assert entry["command"] == "grim"


def test_gemini_mcp_http_transport_maps_to_http_url_not_url(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Gemini MCP (http) → ``httpUrl`` key, NOT ``url`` (which Gemini reserves
    for SSE). Wrong key = dead server entry."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo, body=_MCP_HTTP_DESCRIPTOR)
    rows = runner.json("install", "--client", "gemini")["items"]
    assert rows[0]["status"] == "installed", rows

    entry = json.loads((project_dir / ".gemini/settings.json").read_text())["mcpServers"]["grim-mcp"]
    assert entry.get("httpUrl") == "https://api.example.com/mcp", (
        f"grim http transport must map to Gemini's httpUrl key; got {entry}"
    )
    assert "url" not in entry, f"http transport must NOT use the SSE `url` key: {entry}"


# ---------------------------------------------------------------------------
# Zed
# ---------------------------------------------------------------------------


def test_zed_mcp_entry_lands_in_zed_settings_context_servers(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Zed MCP → ``.zed/settings.json``, container key ``context_servers``,
    flat entry shape (``command`` at the top level)."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "zed")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".zed/settings.json"
    assert cfg.is_file(), "Zed MCP entry must land in .zed/settings.json"
    doc = json.loads(cfg.read_text())
    assert "context_servers" in doc, f"Zed container key must be context_servers: {doc}"
    entry = doc["context_servers"]["grim-mcp"]
    # Flat shape — command sits directly on the entry, not nested under command:{path}.
    assert entry["command"] == "grim", f"Zed entry must be flat with top-level command: {entry}"


def test_zed_mcp_install_preserves_existing_settings_comment_and_sibling(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Installing a Zed MCP entry into a PRE-EXISTING ``.zed/settings.json``
    (JSONC) preserves a user ``//`` comment and a sibling ``context_servers``
    entry — the span-preserving splice touches only grim's own member."""
    zed_dir = project_dir / ".zed"
    zed_dir.mkdir(parents=True, exist_ok=True)
    settings = zed_dir / "settings.json"
    settings.write_text(
        "{\n"
        "  // user's Zed settings\n"
        '  "theme": "One Dark",\n'
        '  "context_servers": {\n'
        '    "my-server": { "command": "my-tool" }\n'
        "  }\n"
        "}\n"
    )

    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "zed")["items"]
    assert rows[0]["status"] == "installed", rows

    text = settings.read_text()
    # The user's JSONC comment and their sibling server survive verbatim.
    assert "// user's Zed settings" in text, f"user comment must survive: {text}"
    assert '"my-server"' in text, f"user's sibling context server must survive: {text}"
    # grim's entry lands alongside, under the same container key.
    doc = _loads_jsonc(text)
    assert doc["context_servers"]["grim-mcp"]["command"] == "grim", f"grim entry added: {doc}"
    assert doc["context_servers"]["my-server"]["command"] == "my-tool", "sibling still readable"
    assert doc["theme"] == "One Dark", "unrelated user key survives"


# ---------------------------------------------------------------------------
# Amp
# ---------------------------------------------------------------------------


def test_amp_mcp_entry_lands_in_amp_settings_dotted_key(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Amp MCP → ``.amp/settings.json``, literal dotted container key
    ``amp.mcpServers`` (a single JSON key, not nested)."""
    runner = grim_at(project_dir)
    _add_mcp(runner, project_dir, registry, unique_repo)
    rows = runner.json("install", "--client", "amp")["items"]
    assert rows[0]["status"] == "installed", rows

    cfg = project_dir / ".amp/settings.json"
    assert cfg.is_file(), "Amp MCP entry must land in .amp/settings.json"
    doc = json.loads(cfg.read_text())
    assert "amp.mcpServers" in doc, (
        f"Amp container key must be the literal dotted key 'amp.mcpServers': {list(doc)}"
    )
    # It must be a single literal key, not a nested {amp: {mcpServers: ...}}.
    assert "amp" not in doc or not isinstance(doc.get("amp"), dict), (
        f"'amp.mcpServers' must be one literal key, not nested amp.mcpServers: {doc}"
    )
    entry = doc["amp.mcpServers"]["grim-mcp"]
    assert entry["command"] == "grim"


# ---------------------------------------------------------------------------
# Modified-output refusal is unchanged for a new-vendor render surface
# (plan Acceptance table: "modified-output refusal unchanged")
# ---------------------------------------------------------------------------


def test_cursor_rule_drift_refused_then_forced(
    grim_at, project_dir: Path, registry: str, unique_repo: str
) -> None:
    """Editing an installed Cursor ``.mdc`` render is detected as drift:
    a plain install refuses (65); ``--force`` restores the rendered content."""
    _push_multi_path_rule(unique_repo)
    write_config(project_dir, rules={"rust-style": f"{registry}/{unique_repo}/rust-style:v1"})
    runner = grim_at(project_dir)
    runner.run("lock", check=False)
    runner.json("install", "--client", "cursor")

    installed = project_dir / ".cursor/rules/rust-style.mdc"
    installed.write_text("hand edited\n")

    refused = runner.run("install", "--client", "cursor", check=False)
    assert refused.returncode == 65, (
        f"modified Cursor render must refuse with 65, got {refused.returncode}; {refused.stderr}"
    )
    assert installed.read_text() == "hand edited\n", "a refused install must not overwrite the edit"

    forced = runner.run("install", "--client", "cursor", "--force", check=False)
    assert forced.returncode == 0, forced.stderr
    assert "alwaysApply: false" in installed.read_text(), "force must restore the rendered content"
