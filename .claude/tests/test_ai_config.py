"""Structural validation tests for .claude/ AI configuration.

These tests ensure the AI configuration files are internally consistent:
paths resolve, cross-references are valid, frontmatter conventions are met,
and documented counts match reality. No LLM invocation needed — pure
filesystem checks.

Run:
    cd .claude/tests && uv run pytest test_ai_config.py -v
"""

from __future__ import annotations

import glob
import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
CLAUDE_DIR = ROOT / ".claude"
CONTEXT_MD = ROOT / "AGENTS.md"
CLAUDE_MD = ROOT / "CLAUDE.md"


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def context_md_text() -> str:
    return CONTEXT_MD.read_text()


@pytest.fixture(scope="module")
def context_md_lines() -> list[str]:
    return CONTEXT_MD.read_text().splitlines()


# ---------------------------------------------------------------------------
# Shareable quality rules: project-independent, path-scoped, no Grimoire leak
# ---------------------------------------------------------------------------


class TestShareableQualityRules:
    """Quality rules must be project-independent for cross-repo sharing.

    The `quality-core.md` root and `quality-{lang}.md` leaves are intended to
    be copyable into other repositories Michael owns. They must contain zero
    Grimoire-specific strings — Grimoire patterns live in `arch-principles.md`
    and `subsystem-*.md`.
    """

    _GRIMOIRE_FORBIDDEN_STRINGS = [
        "PackageErrorKind",
        "ReferenceManager",
        "PackageManager",
        "GrimConfigView",
        "apply_grim_config",
        "GrimRunner",
        "to_relaxed_slug",
        "DIGEST_FILENAME",
        "DirWalker",
        "Printable",
    ]

    _SHAREABLE_RULES = [
        "quality-core.md",
        "quality-rust.md",
        "quality-python.md",
        "quality-bash.md",
    ]

    def test_shareable_rules_no_grimoire_leak(self) -> None:
        """Shareable quality rules must not reference Grimoire-specific names."""
        violations = []
        for name in self._SHAREABLE_RULES:
            path = CLAUDE_DIR / "rules" / name
            if not path.exists():
                continue  # rule not yet created
            text = path.read_text()
            for forbidden in self._GRIMOIRE_FORBIDDEN_STRINGS:
                if forbidden in text:
                    violations.append((name, forbidden))
        assert not violations, (
            f"Shareable quality rules contain Grimoire-specific strings: {violations}. "
            f"Grimoire-specific patterns belong in arch-principles.md or "
            f"subsystem-*.md rules, not in shareable quality rules "
            f"(see meta-ai-config.md Anti-Pattern #10)."
        )

    def test_all_quality_rules_have_paths_frontmatter(self) -> None:
        """Every `quality-{lang}.md` rule must be path-scoped.

        The root `quality-core.md` is global (no paths:) — that's by design,
        it's cross-language. The language leaves must be scoped so they only
        load when editing files of that language.
        """
        missing = []
        for name in self._SHAREABLE_RULES:
            if name == "quality-core.md":
                continue  # root is intentionally global
            path = CLAUDE_DIR / "rules" / name
            if not path.exists():
                continue
            paths_in_frontmatter = TestRuleGlobs._extract_paths(path)
            if not paths_in_frontmatter:
                missing.append(name)
        assert not missing, (
            f"Quality rules missing `paths:` frontmatter: {missing}. "
            f"Language quality rules must be path-scoped so they only load "
            f"when editing that language."
        )

    def test_no_references_to_deleted_language_skills(self) -> None:
        """No files should reference the removed language skills.

        After the reorg, `.claude/skills/{python,rust,typescript,bash,vite}/`
        are gone. Any remaining reference is a broken link.

        Historical artifacts (`.agents/`) and ephemeral plan
        scratch (`.claude/state/`) are exempt — they preserve prior-state
        references intentionally with header notes.
        """
        deleted_skills = [
            "skills/python/SKILL.md",
            "skills/rust/SKILL.md",
            "skills/typescript/SKILL.md",
            "skills/bash/SKILL.md",
            "skills/vite/SKILL.md",
        ]
        violations = []
        for rule_file in CLAUDE_DIR.rglob("*.md"):
            # Skip historical artifacts + ephemeral state — they preserve old references
            if "artifacts" in rule_file.parts or "state" in rule_file.parts:
                continue
            text = rule_file.read_text()
            for deleted in deleted_skills:
                if deleted in text:
                    violations.append((str(rule_file.relative_to(ROOT)), deleted))
        assert not violations, (
            f"Files reference deleted language skills: {violations}. "
            f"Update to reference the corresponding quality-*.md rule."
        )


# ---------------------------------------------------------------------------
# Skills layout: flat directory structure (no category subdirectories)
# ---------------------------------------------------------------------------


class TestSkillsLayout:
    """Enforce the canonical flat `.claude/skills/<name>/SKILL.md` layout.

    Claude Code discovers skills at `.claude/skills/<name>/SKILL.md` exactly —
    it does not recurse into nested skill directories for in-project skills.
    These tests lock in the flat layout.
    """

    _CATEGORY_DIRS = {
        "personas",
        "operations",
        "languages",
        "core-engineering",
        "product",
    }

    def test_all_skills_at_flat_layout(self) -> None:
        """SKILL.md files must live at `.claude/skills/<name>/SKILL.md`."""
        flat = list((CLAUDE_DIR / "skills").glob("*/SKILL.md"))
        nested = list((CLAUDE_DIR / "skills").glob("*/*/SKILL.md"))
        assert flat, "No SKILL.md files found at .claude/skills/<name>/SKILL.md"
        assert not nested, (
            f"Skills must live at `.claude/skills/<name>/SKILL.md` — no "
            f"category subdirectories. Claude Code does not discover nested "
            f"skill paths. Found: {[str(p.relative_to(ROOT)) for p in nested]}"
        )

    def test_skill_dir_matches_frontmatter_name(self) -> None:
        """Each skill's directory name must equal its frontmatter `name:` field."""
        mismatches = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            text = skill_md.read_text()
            if not text.startswith("---"):
                continue
            _, front, _ = text.split("---", 2)
            frontmatter_name = None
            for line in front.splitlines():
                if line.strip().startswith("name:"):
                    frontmatter_name = line.split(":", 1)[1].strip().strip('"').strip("'")
                    break
            if frontmatter_name is None:
                continue
            dir_name = skill_md.parent.name
            if frontmatter_name != dir_name:
                mismatches.append((dir_name, frontmatter_name))
        assert not mismatches, (
            f"Skill directory name must match frontmatter `name:` field — "
            f"Claude Code uses the directory name as the slash command "
            f"identifier. Mismatches (dir, name): {mismatches}"
        )

    def test_no_category_directories_under_skills(self) -> None:
        """No `personas/`, `operations/`, `languages/`, etc. under `.claude/skills/`."""
        skills_dir = CLAUDE_DIR / "skills"
        violating = [
            name
            for name in self._CATEGORY_DIRS
            if (skills_dir / name).is_dir()
        ]
        assert not violating, (
            f"Category subdirectories are forbidden under `.claude/skills/` "
            f"— they break slash command discovery. Found: {violating}"
        )

    def test_action_skills_disable_model_invocation(self) -> None:
        """Skills with side-effectful argument hints must opt out of auto-invocation.

        Any skill whose `argument-hint` contains action verbs
        (deploy|release|sync|create|update|commit|push) must set
        `disable-model-invocation: true` in its frontmatter. This prevents
        Claude from triggering destructive or network-touching workflows
        without explicit user intent.
        """
        action_verbs = re.compile(
            r"\b(deploy|release|sync|create|update|commit|push|mirror)\b",
            re.IGNORECASE,
        )
        violations: list[tuple[str, str]] = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            text = skill_md.read_text()
            if not text.startswith("---"):
                continue
            _, front, _ = text.split("---", 2)
            frontmatter: dict[str, str] = {}
            for line in front.splitlines():
                if ":" in line:
                    key, _, value = line.partition(":")
                    frontmatter[key.strip()] = value.strip().strip('"').strip("'")
            arg_hint = frontmatter.get("argument-hint", "")
            name = frontmatter.get("name", skill_md.parent.name)
            if not arg_hint:
                continue
            if not action_verbs.search(arg_hint) and not action_verbs.search(name):
                continue
            if frontmatter.get("disable-model-invocation", "").lower() != "true":
                violations.append((name, arg_hint))
        assert not violations, (
            f"Skills with action-verb argument hints must set "
            f"`disable-model-invocation: true`. Violations (name, hint): "
            f"{violations}"
        )


# ---------------------------------------------------------------------------
# Rule frontmatter: paths: globs must match at least one file
# ---------------------------------------------------------------------------


class TestRuleGlobs:
    """Verify that scoped rules' paths: globs match actual files."""

    @staticmethod
    def _extract_paths(rule_path: Path) -> list[str]:
        """Parse YAML frontmatter paths: list from a rule file."""
        text = rule_path.read_text()
        if not text.startswith("---"):
            return []
        _, front, _ = text.split("---", 2)
        paths = []
        in_paths = False
        for line in front.splitlines():
            if line.strip().startswith("paths:"):
                in_paths = True
                continue
            if in_paths:
                stripped = line.strip()
                if stripped.startswith("- "):
                    paths.append(stripped[2:].strip().strip('"').strip("'"))
                elif stripped and not stripped.startswith("#"):
                    break
        return paths

    def test_all_rule_globs_match_files(self) -> None:
        """Every paths: glob in .claude/rules/*.md must match >= 1 file.

        Shareable `quality-*.md` rules are exempt: they are designed to match
        file types that may exist in *other* repositories where this rule gets
        copied, not just Grimoire. A missing match in Grimoire doesn't mean the glob is
        dead — it means that file type isn't used here. Dead-glob detection
        still applies to Grimoire-specific rules (subsystem-*.md, architecture-
        principles.md, product-context.md, etc.).
        """
        shareable_prefixes = ("quality-",)
        dead_globs = []
        for rule in sorted(CLAUDE_DIR.glob("rules/*.md")):
            if rule.name.startswith(shareable_prefixes):
                continue
            for pattern in self._extract_paths(rule):
                matches = glob.glob(str(ROOT / pattern), recursive=True)
                if not matches:
                    dead_globs.append((rule.name, pattern))
        assert not dead_globs, f"Rules with dead glob patterns: {dead_globs}"

    def test_subsystem_globs_not_catch_all_rs(self) -> None:
        """No subsystem rule should use a catch-all `*.rs` glob at the crate
        root — that would load subsystem context for unrelated code. Broad
        `src/**` is allowed (the single binary crate); a literal `src/*.rs`
        or `**/*.rs` on a subsystem rule is not.
        """
        offenders = []
        for rule in sorted(CLAUDE_DIR.glob("rules/subsystem-*.md")):
            for p in self._extract_paths(rule):
                if p.endswith("/*.rs") or p == "**/*.rs":
                    offenders.append((rule.name, p))
        assert not offenders, (
            f"Subsystem rules with overly broad `*.rs` glob(s): {offenders}. "
            f"Scope to a module subtree or use `src/**`."
        )


# ---------------------------------------------------------------------------
# AGENTS.md consistency
# ---------------------------------------------------------------------------


class TestContextMd:
    """AGENTS.md must be internally consistent."""

    def test_claude_md_is_a_thin_import_of_agents_md(self) -> None:
        """`CLAUDE.md` holds no content of its own — it imports `AGENTS.md`.

        Project context lives in exactly one file so it cannot drift. The
        Claude Code harness auto-loads `CLAUDE.md`, so a one-line `@` import
        is all it may contain.
        """
        text = CLAUDE_MD.read_text()
        lines = text.splitlines()
        assert "@AGENTS.md" in text, (
            "CLAUDE.md must import AGENTS.md via the `@AGENTS.md` syntax"
        )
        assert len(lines) <= 12, (
            f"CLAUDE.md is {len(lines)} lines — it must stay a pointer, not a "
            f"second copy of project context. Put the content in AGENTS.md."
        )

    def test_line_budget(self, context_md_lines: list[str]) -> None:
        """AGENTS.md must stay under 280 lines (context budget rule)."""
        assert len(context_md_lines) <= 280, (
            f"AGENTS.md is {len(context_md_lines)} lines (budget: 280)"
        )

    def test_principle_count_matches_headings(self, context_md_text: str) -> None:
        """The stated number of principles must match the actual count.

        Bug captured: Says 'seven principles' but there are eight headings
        (### 1 through ### 8).
        """
        # Find the stated count. Match both "These eight principles distill ..."
        # (canonical form) and "Eight principles distill ..." (caveman-compressed
        # form, with the leading "These" filler dropped).
        match = re.search(r"(?:These\s+)?(\w+) principles distill\b", context_md_text)
        assert match, "Could not find 'N principles distill' in AGENTS.md"
        stated = match.group(1)

        # Count actual principle headings (### N.)
        headings = re.findall(r"^### \d+\.", context_md_text, re.MULTILINE)
        actual = len(headings)

        word_to_num = {
            "one": 1, "two": 2, "three": 3, "four": 4, "five": 5,
            "six": 6, "seven": 7, "eight": 8, "nine": 9, "ten": 10,
        }
        stated_num = word_to_num.get(stated.lower(), int(stated) if stated.isdigit() else None)
        assert stated_num == actual, (
            f"AGENTS.md says '{stated}' principles but has {actual} headings"
        )

    def test_worktrees_documented_as_ad_hoc(self, context_md_text: str) -> None:
        """Worktrees are created ad hoc (../grimoire-wt-<topic>), not a fixed
        roster — AGENTS.md must not reassert a stale fixed count/table.

        Bug captured: a fixed "N git worktrees" + table pattern drifts out of
        sync with reality (the prior table named worktrees that no longer
        exist) and needs re-verifying by hand on every worktree churn. The
        ad hoc convention makes the count itself not a fact to state.
        """
        assert "grimoire-wt-" in context_md_text, (
            "AGENTS.md should document the ../grimoire-wt-<topic> worktree convention"
        )
        assert not re.search(r"\*\*Worktrees\*\*: \w+ git worktrees", context_md_text), (
            "AGENTS.md should not reassert a fixed worktree count/table"
        )


# ---------------------------------------------------------------------------
# Feature workflow step numbering
# ---------------------------------------------------------------------------


class TestFeatureWorkflow:
    """workflow-feature.md must have sequential step numbers."""

    def test_hex_workflow_step_numbers_are_sequential(self) -> None:
        """Bug captured: Steps go 1, 2, 3, 3, 4, 5, 6, 7 (duplicate 3)."""
        path = CLAUDE_DIR / "rules" / "workflow-feature.md"
        text = path.read_text()

        # Extract numbered list items (N. **Label**)
        steps = re.findall(r"^(\d+)\.\s+\*\*", text, re.MULTILINE)
        numbers = [int(s) for s in steps]

        # Check first workflow section only (before "## Agent Team")
        agent_team_idx = text.find("## Agent Team")
        first_section = text[:agent_team_idx]
        first_steps = re.findall(r"^(\d+)\.\s+\*\*", first_section, re.MULTILINE)
        first_numbers = [int(s) for s in first_steps]

        expected = list(range(1, len(first_numbers) + 1))
        assert first_numbers == expected, (
            f"Swarm workflow steps are not sequential: {first_numbers} "
            f"(expected {expected})"
        )


# ---------------------------------------------------------------------------
# Artifact paths in skills
# ---------------------------------------------------------------------------


class TestArtifactPaths:
    """Skills must reference correct artifact directories."""

    def test_security_auditor_artifact_path(self) -> None:
        """Bug captured: security-auditor says './artifacts/' instead of
        '.agents/'."""
        path = CLAUDE_DIR / "skills" / "security-auditor" / "SKILL.md"
        text = path.read_text()

        # Must not reference ./artifacts/ (wrong path)
        wrong_refs = re.findall(r"\./artifacts/", text)
        correct_refs = re.findall(r"\.agents/", text)

        assert not wrong_refs, (
            f"security-auditor references wrong artifact path './artifacts/' "
            f"(should be '.agents/')"
        )
        assert correct_refs, "security-auditor should reference .agents/"


# ---------------------------------------------------------------------------
# Rule catalog: `.claude/rules.md` must mirror `.claude/rules/`
# ---------------------------------------------------------------------------


class TestRuleCatalog:
    """`.claude/rules.md` is the discoverability entry point for rules.

    It must stay in sync with the contents of `.claude/rules/` so that
    plan-phase and research-phase readers get an accurate map of what
    rules exist before any file is open.
    """

    CATALOG = CLAUDE_DIR / "rules.md"

    def test_catalog_exists(self) -> None:
        assert self.CATALOG.exists(), (
            "`.claude/rules.md` must exist — it is the authoritative "
            "rule catalog pointed to from AGENTS.md."
        )

    def test_catalog_covers_all_rules(self) -> None:
        """Every rule file in `.claude/rules/*.md` must be referenced
        somewhere in `.claude/rules.md`."""
        catalog_text = self.CATALOG.read_text()
        missing = []
        for rule in sorted(CLAUDE_DIR.glob("rules/*.md")):
            if rule.name not in catalog_text:
                missing.append(rule.name)
        assert not missing, (
            f"Rules missing from `.claude/rules.md`: {missing}. "
            f"Add each new rule to the relevant table in the catalog."
        )

    def test_catalog_references_resolve(self) -> None:
        """Every `*.md` reference in the catalog must resolve to a real
        file under `.claude/rules/` (or be a clearly non-rule link)."""
        text = self.CATALOG.read_text()
        # Match backticked rule filenames like `quality-rust.md`
        refs = set(re.findall(r"`([a-z][a-z0-9-]*\.md)`", text))
        missing = []
        for ref in refs:
            candidate = CLAUDE_DIR / "rules" / ref
            if not candidate.exists():
                missing.append(ref)
        assert not missing, (
            f"Catalog references non-existent rule files: {missing}"
        )

    def test_claude_md_points_to_catalog(self) -> None:
        """AGENTS.md must link to `.claude/rules.md` so the catalog stays
        discoverable for every session."""
        text = CONTEXT_MD.read_text()
        assert ".claude/rules.md" in text, (
            "AGENTS.md must contain a link to `.claude/rules.md` — the "
            "catalog is only valuable if every session sees the pointer."
        )

    def test_all_markdown_refs_resolve(self) -> None:
        """Every backticked `*.md` reference in `.claude/**/*.md` and
        `AGENTS.md` must resolve to a real file on disk.

        Catches drift after renames: if a rule is renamed but a worker,
        skill, or catalog still points at the old name, that reference
        is effectively dead — the rule will never be discovered from
        that path. Skips historical artifacts (they preserve old state).

        **Provisional**: this is a regex-based fallback. It only catches
        backticked bare filenames, not real markdown links or anchor
        fragments. Replace with `task claude:lint:links` (lychee) once lychee
        is mirrored via Grimoire, installed across dev + CI, and wired into
        the `claude:tests` task. At that point delete this test.
        Tracking: the `mirrors/lychee/` config exists but has not yet
        been sync'd to a registry.
        """
        # Candidate directories for resolving a bare filename
        search_dirs = [
            CLAUDE_DIR / "rules",
            CLAUDE_DIR / "references",
            CLAUDE_DIR / "hooks",
            CLAUDE_DIR / "templates",
            CLAUDE_DIR,
            ROOT,
        ]

        # Allowlist: filenames that are external, third-party, or
        # documentation-style references we don't need to resolve.
        allowlist = {
            "file.md",
            "somefile.md",
            "N.md",
            "README.md",
            "CHANGELOG.md",
            # Generated at build time, not in repo
            "dependencies.md",
            # Runtime state file written by /hex-plan, deleted by /finalize
            "current_plan.md",
        }

        # Prefixes for artifact/plan/memory example patterns — these show
        # up in tables and prose as naming examples, not real references.
        example_prefixes = (
            "adr_",
            "plan_",
            "system_design_",
            "design_spec_",
            "security_audit_",
            "research_",
            "feedback_",
            "project_",
            "user_",
        )

        def find_file(name: str) -> bool:
            if name in allowlist:
                return True
            if name.startswith(example_prefixes):
                return True
            for d in search_dirs:
                if (d / name).exists():
                    return True
            # Also allow anywhere under .claude or root (catch templates/ etc.)
            matches = list(CLAUDE_DIR.rglob(name))
            if matches:
                return True
            matches = list((ROOT / "website").rglob(name)) if (ROOT / "website").exists() else []
            if matches:
                return True
            return False

        ref_pattern = re.compile(r"`([a-z][a-z0-9_-]*\.md)`")

        targets: list[Path] = [CONTEXT_MD, CLAUDE_MD]
        for md in CLAUDE_DIR.rglob("*.md"):
            if "artifacts" in md.parts or "state" in md.parts:
                continue  # historical/ephemeral — preserves old references
            if "tests" in md.parts:
                continue  # test file itself doesn't reference rule files
            if "worktrees" in md.parts or "node_modules" in md.parts:
                continue  # nested worktrees + their node_modules are not Grimoire config
            targets.append(md)

        missing: list[tuple[str, str]] = []
        for md in targets:
            text = md.read_text()
            for match in ref_pattern.findall(text):
                if not find_file(match):
                    missing.append((str(md.relative_to(ROOT)), match))

        assert not missing, (
            f"Markdown files reference non-existent `.md` files. Each tuple "
            f"is (file with broken ref, missing target):\n" +
            "\n".join(f"  {src} → {tgt}" for src, tgt in missing)
        )

    def test_catalog_subsystem_coverage(self) -> None:
        """Every subsystem listed in AGENTS.md's subsystem table must also
        appear in the catalog's `By subsystem` section. The catalog is
        allowed to list more subsystems than AGENTS.md (it's the fuller
        reference), but it must never list fewer."""
        context_text = CONTEXT_MD.read_text()
        catalog_text = self.CATALOG.read_text()

        # Extract rule names from AGENTS.md subsystem table rows
        # Lines look like: "| OCI registry/index | `subsystem-oci.md` | ... |"
        context_rules = set(re.findall(r"\[?(subsystem-[a-z-]+\.md)", context_text))
        catalog_rules = set(re.findall(r"\[?(subsystem-[a-z-]+\.md)", catalog_text))

        missing = context_rules - catalog_rules
        assert not missing, (
            f"Subsystems listed in AGENTS.md but missing from catalog "
            f"`By subsystem` section: {missing}"
        )


# ---------------------------------------------------------------------------
# Release implementation references
# ---------------------------------------------------------------------------


class TestReleaseImplementation:
    """workflow-release.md must reference actual workflow filenames."""

    def test_workflow_filenames_match_disk(self) -> None:
        """Bug captured: body references 'publish-to-registry.yml' but actual
        file is 'post-release-oci-publish.yml'."""
        rule = CLAUDE_DIR / "rules" / "workflow-release.md"
        text = rule.read_text()
        workflows_dir = ROOT / ".github" / "workflows"

        # Find workflow filenames referenced in context of .github/workflows/
        _, _, body = text.split("---", 2)
        # Match explicit workflow references (e.g., "workflows/foo.yml" or
        # names that appear in workflow-related context). Exclude non-workflow
        # .yml files like dependabot.yml by only checking names that appear
        # near workflow-related text or are in the frontmatter paths list.
        referenced = set(re.findall(r"workflows?/(\w[\w-]+\.yml)", body))
        # Also capture standalone .yml refs that look like workflow names
        # (contain "release", "verify", "publish", "test")
        for match in re.findall(r"`(\w[\w-]+\.yml)`", body):
            if any(w in match for w in ("release", "verify", "publish", "test", "install")):
                referenced.add(match)

        # Check each referenced workflow exists
        missing = []
        for wf in referenced:
            if not (workflows_dir / wf).exists():
                missing.append(wf)

        assert not missing, (
            f"workflow-release.md references non-existent workflows: "
            f"{missing}"
        )


# ---------------------------------------------------------------------------
# Hook script safety
# ---------------------------------------------------------------------------


class TestHookScript:
    """Hook scripts must follow safety rules and conventions."""

    def test_all_hooks_are_python(self) -> None:
        """All hooks should be Python files, not bash scripts."""
        hooks_dir = CLAUDE_DIR / "hooks"
        sh_files = list(hooks_dir.glob("*.sh"))
        ts_files = list(hooks_dir.glob("*.ts"))
        assert not sh_files, f"Bash hooks still exist: {[f.name for f in sh_files]}"
        assert not ts_files, f"TypeScript hooks still exist: {[f.name for f in ts_files]}"

    def test_all_hooks_have_pep723_header(self) -> None:
        """Every Python hook must have a PEP 723 inline script header."""
        hooks_dir = CLAUDE_DIR / "hooks"
        missing = []
        for py_file in sorted(hooks_dir.glob("*.py")):
            if py_file.name == "hook_utils.py":
                continue  # utils module, not a standalone script
            text = py_file.read_text()
            if "# /// script" not in text:
                missing.append(py_file.name)
        assert not missing, f"Hooks missing PEP 723 header: {missing}"

    def test_post_tool_use_tracker_has_try_except(self) -> None:
        """PostToolUse hook must never exit non-zero — needs try/except."""
        hook = CLAUDE_DIR / "hooks" / "post_tool_use_tracker.py"
        text = hook.read_text()
        assert "try:" in text and "except" in text, (
            "post_tool_use_tracker.py must wrap main logic in try/except "
            "to satisfy the PostToolUse non-blocking contract."
        )

    def test_hooks_use_project_dir_env(self) -> None:
        """Hooks must use CLAUDE_PROJECT_DIR, not os.getcwd()."""
        hooks_dir = CLAUDE_DIR / "hooks"
        violations = []
        for py_file in sorted(hooks_dir.glob("*.py")):
            text = py_file.read_text()
            if "os.getcwd()" in text or "Path.cwd()" in text:
                violations.append(py_file.name)
        assert not violations, (
            f"Hooks using cwd instead of CLAUDE_PROJECT_DIR: {violations}"
        )


# ---------------------------------------------------------------------------
# Taskfile lint: empty file list guard
# ---------------------------------------------------------------------------


class TestTaskfileLint:
    """The shell-lint taskfile must handle empty file lists gracefully."""

    def test_shell_lint_handles_no_scripts(self) -> None:
        """Bug captured: when git ls-files '*.sh' returns nothing, shellcheck
        is called with no arguments and exits non-zero, failing task verify.
        The shell taskfile must guard against an empty file list."""
        taskfile = ROOT / "taskfiles" / "shell.taskfile.yml"
        text = taskfile.read_text()

        # There should be a precondition or guard for empty file lists
        has_guard = bool(re.search(
            r"(preconditions|status|\[ -[nz]|\[\[ -[nz]|test -[nz]|exit 0)",
            text,
        ))
        assert has_guard, (
            "grimoire.taskfile.yml has no guard for empty file lists. "
            "When no matching files exist, the tool is called with no "
            "arguments and fails."
        )


# ---------------------------------------------------------------------------
# AI config overhaul — Phase 1 invariants
# ---------------------------------------------------------------------------


class TestAiConfigOverhaulPhase1:
    """Post-Phase-1 invariants for the AI config overhaul.

    Locks in the path-scope correction (workflow rules scoped, not global),
    the 3-global enumeration in meta-ai-config.md, and the declared overlap
    table in rules.md. See .agents/plans/plan_ai_config_overhaul.md.
    """

    def test_workflow_rules_have_paths(self) -> None:
        """workflow-bugfix.md and workflow-refactor.md must be path-scoped.

        They self-label as catalog-only; without `paths:` frontmatter they load
        every session and contribute ~230 undocumented lines to the always-loaded
        baseline. Enforced post-Phase-1 of the AI config overhaul.
        """
        missing = []
        for name in ("workflow-bugfix.md", "workflow-refactor.md"):
            path = CLAUDE_DIR / "rules" / name
            assert path.exists(), f"{name} missing"
            paths = TestRuleGlobs._extract_paths(path)
            if not paths:
                missing.append(name)
        assert not missing, (
            f"Rules missing `paths:` frontmatter: {missing}. "
            f"See adr_ai_config_path_scope_correction.md."
        )

    def test_global_rule_count_matches(self) -> None:
        """Stated global-rule count in meta-ai-config.md must match actual.

        A global rule is any `.claude/rules/*.md` file without a non-empty
        `paths:` frontmatter entry. Post Phase 1 of the AI config overhaul,
        the authoritative count is 3 and the list appears in meta-ai-config.md
        under `### Current Global Rules`. `rules.md` and `AGENTS.md` reach
        Claude by a different mechanism (`@`-import / root instructions) and
        are not counted here.
        """
        rules_dir = CLAUDE_DIR / "rules"
        globals_found = []
        for rule in sorted(rules_dir.glob("*.md")):
            paths = TestRuleGlobs._extract_paths(rule)
            if not paths:
                globals_found.append(rule.name)
        assert len(globals_found) == 3, (
            f"Expected exactly 3 global rules (no `paths:` frontmatter), "
            f"got {len(globals_found)}: {globals_found}"
        )
        meta_text = (rules_dir / "meta-ai-config.md").read_text()
        assert "### Current Global Rules" in meta_text, (
            "meta-ai-config.md must contain `### Current Global Rules` "
            "enumeration (Phase 1 T3)"
        )
        # Every global must be explicitly named in the enumeration
        for name in globals_found:
            assert name in meta_text, (
                f"Global rule {name} is not enumerated in meta-ai-config.md "
                f"`### Current Global Rules` — stated/actual drift"
            )

    def test_path_overlaps_declared_or_absent(self) -> None:
        """Any two rules sharing a `paths:` pattern must be declared in rules.md.

        Exempt rules (intended broad coupling):
        - `quality-*.md` — language quality rules co-fire with subsystem rules
          on `**/*.rs`, `**/*.py`, etc. (e.g., quality-rust.md + subsystem-cli.md
          on `**/*.rs` is intended).
        - `workflow-bugfix.md`, `workflow-refactor.md` — source-work-surface
          scope (`src/**`, `test/**`, `.claude/**`). Co-firing with subsystem
          rules on their respective scopes is the intended coupling.
        """
        _exempt_prefixes = ("quality-",)
        _exempt_names = {"workflow-bugfix.md", "workflow-refactor.md"}
        rules_dir = CLAUDE_DIR / "rules"
        pattern_owners: dict[str, list[str]] = {}
        for rule in sorted(rules_dir.glob("*.md")):
            if rule.name.startswith(_exempt_prefixes):
                continue
            if rule.name in _exempt_names:
                continue
            for p in TestRuleGlobs._extract_paths(rule):
                pattern_owners.setdefault(p, []).append(rule.name)

        catalog = (CLAUDE_DIR / "rules.md").read_text()
        # Extract declared pairs from the overlap table: lines like
        # `| \`file-a.md\` + \`file-b.md\` | ...`
        declared_pairs: set[frozenset[str]] = set()
        for line in catalog.splitlines():
            if "+" not in line or "`" not in line or not line.startswith("|"):
                continue
            files = re.findall(r"`([^`]+\.md)`", line)
            if len(files) >= 2:
                # Treat all filenames in the left cell as one declared group
                declared_pairs.add(frozenset(files))

        undeclared: list[tuple[str, list[str]]] = []
        for pattern, owners in pattern_owners.items():
            if len(owners) < 2:
                continue
            # Any pair drawn from owners must appear as a subset of a declared group
            for i in range(len(owners)):
                for j in range(i + 1, len(owners)):
                    pair = frozenset({owners[i], owners[j]})
                    if not any(pair <= group for group in declared_pairs):
                        undeclared.append((pattern, [owners[i], owners[j]]))
                        break
        assert not undeclared, (
            f"Undeclared path-scope overlaps: {undeclared}. "
            f"Declare in rules.md `## Declared Path-Scope Overlaps` table or "
            f"narrow one of the rule's `paths:` patterns."
        )


# ---------------------------------------------------------------------------
# AI config overhaul — Phase 2 invariants (CSO description audit)
# ---------------------------------------------------------------------------


class TestAiConfigOverhaulPhase2:
    """Post-Phase-2 invariants for the AI config overhaul.

    Locks in the Contextual Signal Only (CSO) policy for skill descriptions:
    descriptions describe trigger conditions (what the user says / what the
    task looks like), never the workflow itself. See
    `.agents/adr/adr_ai_config_skill_description_csopolicy.md`.
    """

    # Hyphen-aware word boundary: require a whitespace / punctuation boundary
    # on both sides so hyphen-joined fragments like `dry-runs` or `re-runs`
    # do not falsely trigger the CSO filter.
    _FORBIDDEN_VERB_RE = re.compile(
        r"(?<![\w-])(dispatches|runs|iterates|orchestrates|performs|executes|handles)(?![\w-])",
        re.IGNORECASE,
    )

    # Per-skill `disable-model-invocation` intent table. Prevents accidental
    # flips (action skill losing the flag, or pure-advisory skill gaining it).
    _EXPECTED_DISABLE_MODEL_INVOCATION = {
        # Action skills with side effects — must disable auto-invocation
        "commit": True,
        # Owner-flipped 2026-08-10 so an autonomous run can reach /finalize
        # without a human in the loop. Still an action skill with side
        # effects — re-flip both this entry and the frontmatter together if
        # the manual-only policy is restored.
        "finalize": False,
            "meta-maintain-config": True,
                        # Pure analysis / advisory — auto-invocation safe
            "bugfix": False,
        "builder": False,
        "code-check": False,
            "docs": False,
        "meta-validate-context": False,
        "next": True,
        "qa-engineer": False,
        "security-auditor": False,
        }

    @staticmethod
    def _parse_frontmatter(skill_md: Path) -> dict[str, str]:
        """Parse single-line key: value pairs from the first `---` block.

        Intentionally simple — CSO descriptions are required to be single-line
        so this parser is sufficient. Multi-line (block-scalar) descriptions
        would produce a truncated value, which the CSO tests flag.
        """
        text = skill_md.read_text()
        if not text.startswith("---"):
            return {}
        _, front, _ = text.split("---", 2)
        fm: dict[str, str] = {}
        for line in front.splitlines():
            if ":" not in line:
                continue
            key, _, value = line.partition(":")
            fm[key.strip()] = value.strip().strip('"').strip("'")
        return fm

    def test_skill_descriptions_are_cso_compliant(self) -> None:
        """Every skill description must describe trigger conditions, not the
        workflow itself. Forbidden literal verbs: dispatches, runs, iterates,
        orchestrates, performs, executes, handles (case-insensitive).

        Per meta-ai-config.md budget: each description ≤1024 chars. CSO
        descriptions are single-line; multi-line (block-scalar) is disallowed
        because the simple parser above would truncate and the real context
        loader would concatenate — both paths degrade discoverability.
        """
        violations: list[tuple[str, str]] = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            name = skill_md.parent.name
            fm = self._parse_frontmatter(skill_md)
            desc = fm.get("description", "")
            if not desc:
                violations.append((name, "missing or empty description"))
                continue
            if len(desc) > 1024:
                violations.append(
                    (name, f"description too long: {len(desc)} > 1024 chars")
                )
            match = self._FORBIDDEN_VERB_RE.search(desc)
            if match:
                violations.append(
                    (name, f"contains forbidden verb {match.group(0)!r}")
                )
        assert not violations, (
            f"Skill descriptions violate CSO policy: {violations}. "
            f"See `.agents/adr/adr_ai_config_skill_description_csopolicy.md`."
        )

    def test_skill_description_budget_under_cap(self) -> None:
        """Sum of all skill description chars must stay under the 4000-char
        cap (buffer below Anthropic's 1% context-window description budget).

        Pre-Phase-2 baseline was 5004 chars; Phase 2 target is ≤4000, giving
        ≈20% headroom for future skill growth before hitting the cap.
        """
        total = 0
        per_skill: list[tuple[str, int]] = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            fm = self._parse_frontmatter(skill_md)
            desc = fm.get("description", "")
            total += len(desc)
            per_skill.append((skill_md.parent.name, len(desc)))
        assert total <= 4000, (
            f"Total skill description budget {total} chars exceeds 4000-char "
            f"cap. Per-skill lengths: {per_skill}"
        )

    def test_skill_disable_model_invocation_intent(self) -> None:
        """Fixture-based stability test: every skill's
        `disable-model-invocation` flag must match its declared intent.

        Prevents accidental flips — e.g. an action skill losing the flag
        (silently auto-invoked by Claude), or a pure-advisory skill gaining
        it (needlessly removed from auto-invocation).
        """
        mismatches: list[tuple[str, object, object]] = []
        unlisted: list[str] = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            name = skill_md.parent.name
            if name not in self._EXPECTED_DISABLE_MODEL_INVOCATION:
                unlisted.append(name)
                continue
            fm = self._parse_frontmatter(skill_md)
            actual = fm.get("disable-model-invocation", "false").lower() == "true"
            expected = self._EXPECTED_DISABLE_MODEL_INVOCATION[name]
            if actual != expected:
                mismatches.append((name, expected, actual))
        assert not unlisted, (
            f"Skills not in `_EXPECTED_DISABLE_MODEL_INVOCATION` intent table: "
            f"{unlisted}. Add each to the table with the correct expected "
            f"flag value before it will pass the stability check."
        )
        assert not mismatches, (
            f"Skill `disable-model-invocation` intent mismatches "
            f"(name, expected, actual): {mismatches}. "
            f"Update the skill frontmatter or the intent table in this test "
            f"if the policy has genuinely changed."
        )


# ---------------------------------------------------------------------------
# AI config overhaul — Phase 4 invariants (cross-session learnings store)
# ---------------------------------------------------------------------------


class TestAiConfigOverhaulPhase4:
    """Post-Phase-4 invariants for the AI config overhaul.

    Locks in the project-local learnings store location and the
    `meta-ai-config.md` Cross-Session Learnings section. See
    `.agents/adr/adr_ai_config_cross_session_learnings_store.md`.
    """

    def test_gitignore_contains_state_dir(self) -> None:
        """`.gitignore` must ignore `.claude/state/` so the learnings store
        (and other per-worktree ephemera) is never accidentally committed.

        Phase 3 added the entry; this test locks it in so a future
        gitignore edit cannot silently drop it.
        """
        gitignore = ROOT / ".gitignore"
        assert gitignore.exists(), "`.gitignore` missing at repo root"
        text = gitignore.read_text()
        assert ".claude/state/" in text, (
            "`.gitignore` must contain `.claude/state/` — per-worktree "
            "learnings store / context samples must not be committed. "
            "See `.agents/adr/adr_ai_config_cross_session_learnings_store.md`."
        )

    def test_meta_ai_config_has_cross_session_learnings_section(self) -> None:
        """`meta-ai-config.md` must document the Cross-Session Learnings
        Store section and cite the ADR path."""
        meta = CLAUDE_DIR / "rules" / "meta-ai-config.md"
        text = meta.read_text()
        assert "## Cross-Session Learnings Store" in text, (
            "meta-ai-config.md must contain `## Cross-Session Learnings Store` "
            "header (Phase 4 T4)"
        )
        assert "adr_ai_config_cross_session_learnings_store.md" in text, (
            "meta-ai-config.md Cross-Session Learnings section must cite "
            "the ADR path so readers can find the decision record."
        )


# ---------------------------------------------------------------------------
# AI config overhaul — Phase 5 invariants (Review-Fix Loop parity + skill body budget)
# ---------------------------------------------------------------------------


class TestAiConfigOverhaulPhase5:
    """Post-Phase-5 invariants for the AI config overhaul.

    Locks in the two-carrier byte-identical Review-Fix Loop parity
    (`workflow-bugfix.md` is canonical, `workflow-refactor.md` mirrors it)
    and the 200-line ceiling for every `SKILL.md`. See
    `.agents/adr/adr_ai_config_review_loop_dedup.md` and
    `.agents/plans/plan_ai_config_overhaul.md` (Phase 5).
    """

    _CANONICAL_CARRIERS = (
        CLAUDE_DIR / "rules" / "workflow-bugfix.md",
        CLAUDE_DIR / "rules" / "workflow-refactor.md",
    )

    # Files that must point at the canonical Review-Fix Loop but NOT contain
    # the canonical markers themselves. Prevents accidental fourth carrier.
    _POINTER_ONLY_FILES = (CLAUDE_DIR / "rules" / "workflow-feature.md",)

    _BEGIN_MARKER = "<!-- REVIEW_FIX_LOOP_CANONICAL_BEGIN -->"
    _END_MARKER = "<!-- REVIEW_FIX_LOOP_CANONICAL_END -->"

    # Explicit exception list for `test_skill_body_budget`. Every entry needs
    # a docstring comment (in this class) explaining why it is exempt.
    # Current state: empty. Every SKILL.md is ≤200 lines after Phase 5.
    _SKILL_BODY_BUDGET_EXCEPTIONS: tuple[str, ...] = ()

    def test_review_fix_loop_parity(self) -> None:
        """Both canonical carriers must contain byte-identical Review-Fix
        Loop blocks between the HTML comment markers.

        Carriers: `workflow-bugfix.md` (canonical source) and
        `workflow-refactor.md`. Pointer-only files must NOT contain the
        markers — they link to the canonical block instead.
        """
        # Every carrier must have exactly one BEGIN and one END marker
        carrier_blocks: dict[str, str] = {}
        for carrier in self._CANONICAL_CARRIERS:
            assert carrier.exists(), f"Canonical carrier missing: {carrier}"
            text = carrier.read_text()
            begin_count = text.count(self._BEGIN_MARKER)
            end_count = text.count(self._END_MARKER)
            assert begin_count == 1, (
                f"{carrier.name} must contain exactly one "
                f"{self._BEGIN_MARKER} marker (got {begin_count})."
            )
            assert end_count == 1, (
                f"{carrier.name} must contain exactly one "
                f"{self._END_MARKER} marker (got {end_count})."
            )
            begin_idx = text.index(self._BEGIN_MARKER)
            end_idx = text.index(self._END_MARKER) + len(self._END_MARKER)
            assert begin_idx < end_idx, (
                f"{carrier.name}: BEGIN marker must precede END marker."
            )
            carrier_blocks[carrier.name] = text[begin_idx:end_idx]

        # Byte-identity across all three carriers
        reference_name, reference_block = next(iter(carrier_blocks.items()))
        divergent: list[tuple[str, str]] = []
        for name, block in carrier_blocks.items():
            if block != reference_block:
                divergent.append((name, reference_name))
        assert not divergent, (
            f"Canonical Review-Fix Loop blocks diverged across carriers. "
            f"Every carrier must contain byte-identical prose between the "
            f"markers. Divergent carriers: {divergent}. "
            f"See `.agents/adr/adr_ai_config_review_loop_dedup.md`. "
            f"Quick fix: `task claude:fix:canonical-block` (re-syncs from "
            f"workflow-bugfix.md into workflow-refactor.md)."
        )

        # Pointer-only files must NOT contain the markers (no fourth carrier)
        illegal_carriers: list[str] = []
        for pointer in self._POINTER_ONLY_FILES:
            assert pointer.exists(), f"Pointer-only file missing: {pointer}"
            text = pointer.read_text()
            if self._BEGIN_MARKER in text or self._END_MARKER in text:
                illegal_carriers.append(str(pointer.relative_to(ROOT)))
        assert not illegal_carriers, (
            f"Pointer-only files contain canonical Review-Fix Loop markers "
            f"(would create a fourth carrier): {illegal_carriers}. "
            f"Replace the marker block with a pointer to the canonical "
            f"Review-Fix Loop in `workflow-bugfix.md`."
        )


    def test_skill_body_budget(self) -> None:
        """Every `.claude/skills/*/SKILL.md` must be ≤200 lines.

        SKILL.md is loaded only when invoked — per meta-ai-config.md the
        budget is <500 lines — but post-Phase-5 every SKILL.md in Grimoire
        stays ≤200 via progressive disclosure (`references/` subdir for
        detail material). Exceptions live in
        `_SKILL_BODY_BUDGET_EXCEPTIONS` with a docstring justification.
        """
        violations: list[tuple[str, int]] = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            name = skill_md.parent.name
            if name in self._SKILL_BODY_BUDGET_EXCEPTIONS:
                continue
            line_count = len(skill_md.read_text().splitlines())
            if line_count > 200:
                violations.append((name, line_count))
        assert not violations, (
            f"SKILL.md files exceed the 200-line progressive-disclosure "
            f"budget: {violations}. Extract detail sections into "
            f"`<skill-dir>/references/*.md` and replace with pointers, or "
            f"add the skill to `_SKILL_BODY_BUDGET_EXCEPTIONS` with a "
            f"justification."
        )


# ---------------------------------------------------------------------------
# UserPromptSubmit routing hook: triggers contract + hook sanity
# ---------------------------------------------------------------------------


class TestPromptRoutingTriggers:
    """Enforce the `triggers:` contract for user-invocable skills.

    The `user_prompt_router.py` UserPromptSubmit hook reads the
    `triggers:` frontmatter field from each skill at runtime. Any
    user-invocable skill without triggers silently drops out of the
    matcher, so natural-language prompts never route to it.
    """

    _ALLOWED_SINGLE_WORD_TRIGGERS = {"deps", "commit", "finalize"}
    _MIN_TRIGGERS = 3
    _MAX_TRIGGERS = 7

    @staticmethod
    def _parse_frontmatter(text: str) -> dict:
        if not text.startswith("---"):
            return {}
        lines = text.splitlines()
        end = None
        for i in range(1, len(lines)):
            if lines[i].strip() == "---":
                end = i
                break
        if end is None:
            return {}
        result: dict = {}
        current_list_key: str | None = None
        for raw in lines[1:end]:
            if not raw.strip():
                current_list_key = None
                continue
            if raw.startswith("  - ") or raw.startswith("- "):
                if current_list_key is None:
                    continue
                item = raw.split("- ", 1)[1].strip()
                if (item.startswith('"') and item.endswith('"')) or (
                    item.startswith("'") and item.endswith("'")
                ):
                    item = item[1:-1]
                result.setdefault(current_list_key, []).append(item)
                continue
            if ":" in raw and not raw.startswith(" "):
                key, _, value = raw.partition(":")
                key = key.strip()
                value = value.strip()
                if not value:
                    current_list_key = key
                    continue
                current_list_key = None
                if (value.startswith('"') and value.endswith('"')) or (
                    value.startswith("'") and value.endswith("'")
                ):
                    value = value[1:-1]
                result[key] = value
        return result

    @classmethod
    def _user_invocable_skills(cls) -> list[tuple[str, dict]]:
        out: list[tuple[str, dict]] = []
        for skill_md in sorted((CLAUDE_DIR / "skills").glob("*/SKILL.md")):
            fm = cls._parse_frontmatter(skill_md.read_text())
            if fm.get("user-invocable") == "true":
                out.append((skill_md.parent.name, fm))
        return out

    def test_user_invocable_skills_have_triggers(self) -> None:
        """Every user-invocable skill must declare 3–7 triggers."""
        violations: list[tuple[str, str]] = []
        for name, fm in self._user_invocable_skills():
            triggers = fm.get("triggers")
            if not isinstance(triggers, list) or not triggers:
                violations.append((name, "missing or empty triggers: list"))
                continue
            if not (self._MIN_TRIGGERS <= len(triggers) <= self._MAX_TRIGGERS):
                violations.append(
                    (name, f"has {len(triggers)} triggers (want 3–7)")
                )
        assert not violations, (
            f"User-invocable skills missing or malformed `triggers:` "
            f"frontmatter: {violations}. The UserPromptSubmit routing hook "
            f"reads this list at runtime — without it, natural-language "
            f"prompts never route to the skill. See "
            f"`.claude/rules/meta-ai-config.md` Anti-Pattern #12."
        )

    def test_triggers_unique_across_skills(self) -> None:
        """No trigger phrase may appear in two skills' `triggers:` lists.

        The routing hook uses first-match-wins at runtime, but
        cross-skill duplicates are ambiguous by design — they silently
        bias routing on glob order. The structural gate fails loud.
        """
        seen: dict[str, str] = {}
        collisions: list[tuple[str, str, str]] = []
        for name, fm in self._user_invocable_skills():
            for trigger in fm.get("triggers") or []:
                key = trigger.strip().lower()
                if not key:
                    continue
                if key in seen and seen[key] != name:
                    collisions.append((key, seen[key], name))
                else:
                    seen[key] = name
        assert not collisions, (
            f"Duplicate triggers across skills "
            f"(trigger, first-skill, second-skill): {collisions}"
        )

    def test_triggers_are_discriminating(self) -> None:
        """Each trigger must be ≥2 words OR an allowed single-word domain token."""
        violations: list[tuple[str, str]] = []
        for name, fm in self._user_invocable_skills():
            for trigger in fm.get("triggers") or []:
                key = trigger.strip()
                words = key.split()
                if len(words) >= 2:
                    continue
                if key.lower() in self._ALLOWED_SINGLE_WORD_TRIGGERS:
                    continue
                violations.append((name, trigger))
        assert not violations, (
            f"Single-word triggers are only allowed from the domain-token "
            f"set {sorted(self._ALLOWED_SINGLE_WORD_TRIGGERS)}. Violations "
            f"(skill, trigger): {violations}. Use a 2+ word phrase to reduce "
            f"false positives."
        )


class TestUserPromptRouter:
    """Sanity checks on the user_prompt_router.py hook script."""

    _HOOK = CLAUDE_DIR / "hooks" / "user_prompt_router.py"

    def test_user_prompt_router_has_pep723_header(self) -> None:
        text = self._HOOK.read_text()
        assert "# /// script" in text, (
            "user_prompt_router.py must start with a PEP 723 inline "
            "script header (`# /// script` … `# ///`)."
        )

    def test_user_prompt_router_uses_project_dir_env(self) -> None:
        text = self._HOOK.read_text()
        assert "get_project_dir" in text, (
            "user_prompt_router.py must resolve the project directory via "
            "`hook_utils.get_project_dir()` — not `os.getcwd()` / `Path.cwd()`."
        )
        assert "os.getcwd()" not in text and "Path.cwd()" not in text, (
            "user_prompt_router.py must not use `os.getcwd()` or `Path.cwd()`."
        )

    def test_user_prompt_router_exits_zero(self) -> None:
        """Every `sys.exit(...)` in the router must pass `0`.

        The hook is advisory; a non-zero exit would make Claude Code
        treat the prompt as blocked. AST scan to catch future drift.
        """
        import ast

        tree = ast.parse(self._HOOK.read_text())
        bad: list[tuple[int, str]] = []
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            name = None
            if isinstance(func, ast.Attribute) and func.attr == "exit":
                if isinstance(func.value, ast.Name) and func.value.id == "sys":
                    name = "sys.exit"
            elif isinstance(func, ast.Name) and func.id == "exit":
                name = "exit"
            if name is None:
                continue
            if not node.args:
                continue
            arg = node.args[0]
            if isinstance(arg, ast.Constant) and arg.value == 0:
                continue
            bad.append((node.lineno, ast.unparse(node)))
        assert not bad, (
            f"user_prompt_router.py must only ever `sys.exit(0)` — the hook "
            f"is advisory, not gating. Non-zero exits found: {bad}"
        )

    def test_user_prompt_router_registered_in_settings(self) -> None:
        import json

        settings_path = CLAUDE_DIR / "settings.json"
        settings = json.loads(settings_path.read_text())
        hooks = settings.get("hooks", {})
        ups = hooks.get("UserPromptSubmit") or []
        commands = [
            h.get("command", "")
            for entry in ups
            for h in entry.get("hooks", [])
        ]
        assert any(
            "user_prompt_router.py" in cmd for cmd in commands
        ), (
            "user_prompt_router.py is not registered under "
            "`hooks.UserPromptSubmit` in `.claude/settings.json`."
        )

    def test_user_prompt_router_output_is_single_line(self) -> None:
        """Every `print(...)` call in the router emits a single line.

        AST scan: the printed expression must be a constant or f-string
        whose literal parts contain no newline. Guards the "zero context
        bloat" invariant from the item 3 design.
        """
        import ast

        tree = ast.parse(self._HOOK.read_text())
        bad: list[tuple[int, str]] = []
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            if not (isinstance(func, ast.Name) and func.id == "print"):
                continue
            if not node.args:
                continue
            arg = node.args[0]
            if isinstance(arg, ast.Constant):
                if isinstance(arg.value, str) and "\n" in arg.value:
                    bad.append((node.lineno, repr(arg.value)))
                continue
            if isinstance(arg, ast.JoinedStr):
                for part in arg.values:
                    if isinstance(part, ast.Constant) and isinstance(
                        part.value, str
                    ) and "\n" in part.value:
                        bad.append((node.lineno, ast.unparse(arg)))
                        break
                continue
            bad.append((node.lineno, ast.unparse(node)))
        assert not bad, (
            f"user_prompt_router.py `print(...)` calls must emit a single "
            f"line with no newlines — zero context bloat is a load-bearing "
            f"invariant. Violations: {bad}"
        )


# ---------------------------------------------------------------------------
# Plan Status block — top-of-plan progress signal for /next
# ---------------------------------------------------------------------------


class TestPlanStatusBlock:
    """Every plan file in `.agents/plans/plan_*.md` must carry a `## Status`
    block at the top with the four mandatory fields. Schema and protocol live in
    `.claude/rules/meta-ai-config.md` "Plan Status Protocol".

    `.agents/plans/` is committed — plans are team-shared, not per-worktree.
    Tests skip silently when no plan files exist (fresh checkout).
    Excludes `meta-plan_*.md` files (skill-internal scratch artifacts).
    """

    _MANDATORY_FIELDS = (
        "**Plan:**",
        "**Active phase:**",
        "**Step:**",
        "**Last update:**",
    )

    def _plan_files(self) -> list[Path]:
        plans_dir = ROOT / ".agents" / "plans"
        if not plans_dir.exists():
            return []
        return [
            p
            for p in sorted(plans_dir.glob("plan_*.md"))
            if not p.name.startswith("meta-plan_")
        ]

    def _extract_status_block(self, text: str) -> str | None:
        """Return content between `## Status` and the next `## ` heading, or None."""
        lines = text.splitlines()
        start = None
        for i, line in enumerate(lines):
            if line.strip() == "## Status":
                start = i + 1
                break
        if start is None:
            return None
        end = len(lines)
        for j in range(start, len(lines)):
            if lines[j].startswith("## ") and lines[j].strip() != "## Status":
                end = j
                break
        return "\n".join(lines[start:end])

    def test_every_plan_has_status_block(self) -> None:
        """Each plan_*.md must contain a `## Status` heading."""
        plans = self._plan_files()
        if not plans:
            pytest.skip("No plan files in .agents/plans/ (fresh checkout)")
        missing = [
            p.relative_to(ROOT) for p in plans if "## Status" not in p.read_text()
        ]
        assert not missing, (
            f"Plans missing `## Status` block: {missing}. "
            f"Add one per `.claude/templates/artifacts/plan.template.md` schema. "
            f"Protocol: `.claude/rules/meta-ai-config.md` `Plan Status Protocol`."
        )

    def test_status_block_has_all_mandatory_fields(self) -> None:
        """Status block must contain Plan / Active phase / Step / Last update."""
        plans = self._plan_files()
        if not plans:
            pytest.skip("No plan files in .agents/plans/ (fresh checkout)")
        violations: list[tuple[Path, list[str]]] = []
        for plan in plans:
            block = self._extract_status_block(plan.read_text())
            if block is None:
                # covered by previous test
                continue
            missing_fields = [f for f in self._MANDATORY_FIELDS if f not in block]
            if missing_fields:
                violations.append((plan.relative_to(ROOT), missing_fields))
        assert not violations, (
            f"Plans with incomplete Status block: {violations}. "
            f"Required fields: {list(self._MANDATORY_FIELDS)}."
        )

    def test_status_block_in_first_30_lines(self) -> None:
        """Status block must be near the top — /next reads first 30 lines only."""
        plans = self._plan_files()
        if not plans:
            pytest.skip("No plan files in .agents/plans/ (fresh checkout)")
        too_late: list[tuple[Path, int]] = []
        for plan in plans:
            lines = plan.read_text().splitlines()
            for i, line in enumerate(lines[:30], start=1):
                if line.strip() == "## Status":
                    break
            else:
                # not found in first 30 lines
                for j, line in enumerate(lines, start=1):
                    if line.strip() == "## Status":
                        too_late.append((plan.relative_to(ROOT), j))
                        break
        assert not too_late, (
            f"`## Status` block must appear within first 30 lines for /next "
            f"to read it cheaply. Late blocks: {too_late}."
        )

    def test_template_has_status_block(self) -> None:
        """Plan templates must seed the Status block so new plans get one for free."""
        templates = [
            CLAUDE_DIR / "templates" / "artifacts" / "plan.template.md",
            CLAUDE_DIR / "templates" / "artifacts" / "bugfix_plan.template.md",
        ]
        for tmpl in templates:
            assert tmpl.exists(), f"Template missing: {tmpl}"
            text = tmpl.read_text()
            assert "## Status" in text, (
                f"{tmpl.relative_to(ROOT)} missing `## Status` block — "
                f"new plans created from this template would fail "
                f"TestPlanStatusBlock invariants."
            )
            block = self._extract_status_block(text)
            assert block is not None, f"Status heading present but block parse failed: {tmpl}"
            for field in self._MANDATORY_FIELDS:
                assert field in block, (
                    f"{tmpl.relative_to(ROOT)} Status block missing field {field}"
                )


# ---------------------------------------------------------------------------
# ADR Index: every ADR indexed, no ADR referenced by a finalized plan stays
# `Status: Proposed`
# ---------------------------------------------------------------------------


class TestAdrIndex:
    """`arch-principles.md`'s "ADR Index" table is the map this repo tells
    every agent to consult before deciding in a domain (see AGENTS.md
    "Architecture" and the rule's own header). Two invariants keep the table
    itself, and the ADRs it indexes, from drifting silently out of sync with
    what actually shipped.

    Both tests scan every ADR / plan in the repo, not just one pair — a gap
    found here is real drift to report, never something to paper over by
    editing an unrelated ADR or narrowing the assertion (see
    `.claude/rules/meta-ai-config.md` "Principle: test the contract, not the
    content").
    """

    _ARCH_PRINCIPLES = CLAUDE_DIR / "rules" / "arch-principles.md"
    _ADR_DIR = ROOT / ".agents" / "adr"
    _PLANS_DIR = ROOT / ".agents" / "plans"

    # Frozen allowlist of pre-existing drift, recorded 2026-08-10 (round-2
    # review-fix wave, S-17). Every name below already lacked an ADR Index
    # row before these two tests existed — they are debt, not new failures,
    # and this list is the only thing keeping the debt from blocking every
    # future branch. It may only SHRINK, as each entry gets a real index row
    # backfilled; it must never grow. A file that is not already named here
    # is a real, new gap — fix the source, never add it to this list.
    _MISSING_INDEX_ROW_ALLOWLIST = frozenset(
        {
            "adr_anchor_escape_recovery.md",
            "adr_client_compat_matrix.md",
            "adr_description_companion.md",
            "adr_grim_publish.md",
            "adr_hooks_support.md",
            "adr_local_path_sources.md",
            "adr_managed_context_block.md",
            "adr_projection_over_index.md",
            "adr_registry_default_dedup.md",
            "adr_render_layout_stability.md",
            "adr_structured_vendor_metadata.md",
            "adr_vendor_wave_expansion.md",
        }
    )

    # Frozen allowlist of (plan, adr) pairs where a finalized plan already
    # references a still-`Proposed` ADR, recorded 2026-08-10 — same freeze
    # rule as above: shrink-only, one named pair per known case, never a
    # count or a glob.
    #
    # The second pair was surfaced the same day by widening `_referenced_adrs`
    # to the backticked-bare-filename citation shape (round-3 review, T-1): it
    # cites the SAME already-recorded ADR from a second finalized plan, so it
    # is the same debt seen through a wider lens, not a new regression. Both
    # clear together when `adr_projection_over_index.md` gets a real status.
    #
    # The third pair is the same ADR again, seen from a third finalized plan
    # that only *mentions* it — `plan_registry_browse_filters` cites it as
    # background for withdrawing its own D7, and ships no part of that
    # decision. This is the "genuine exception" `_referenced_adrs` documents:
    # the widened regex cannot tell a citation from a mention, and the safe
    # direction is to over-match and record the exception here. All three
    # clear together.
    _STALE_PROPOSED_ALLOWLIST = frozenset(
        {
            ("plan_tui_tree_view_phase2.md", "adr_projection_over_index.md"),
            ("plan_tui_member_nodes.md", "adr_projection_over_index.md"),
            ("plan_registry_browse_filters.md", "adr_projection_over_index.md"),
        }
    )

    @staticmethod
    def _indexed_adrs(arch_principles_text: str) -> set[str]:
        """ADR filenames that have a row in the "## ADR Index" table.

        Every such row starts its line with `| [adr_<name>.md](...)` — no
        other table in this file leads a row with an `adr_*.md` link, so a
        whole-file scan is equivalent to (and simpler than) extracting the
        section first.
        """
        return set(re.findall(r"^\| \[(adr_[^\]]+\.md)\]", arch_principles_text, re.MULTILINE))

    @staticmethod
    def _adr_status(adr_text: str) -> str | None:
        """The value of an ADR's `**Status:**` metadata field, or None."""
        m = re.search(r"\*\*Status:\*\*\s*(.+)", adr_text)
        return m.group(1).strip() if m else None

    @staticmethod
    def _plan_step(plan_text: str) -> str | None:
        """The value of a plan's `**Step:**` Status-block field, or None."""
        m = re.search(r"\*\*Step:\*\*\s*(.+)", plan_text)
        return m.group(1).strip() if m else None

    @staticmethod
    def _referenced_adrs(plan_text: str) -> set[str]:
        """Every ADR filename a plan names, in any shape.

        Deliberately the bare filename and not the `(../adr/adr_*.md)` link
        it started as: that narrower regex described one citation style the
        repo does not consistently use. Measured over the 12 finalized plans,
        it saw 5 (plan, adr) pairs while three more plans cite an ADR as a
        backticked bare filename (`adr_projection_over_index.md`) or a
        backticked `.agents/adr/…` path — 8 pairs once widened. One of the
        three was real drift nobody had recorded.

        The filename is unambiguous enough to match on its own (`adr_*.md`,
        underscores only, one flat directory), and it subsumes the link shape
        rather than being OR-ed with it. The trade is that a plan merely
        *mentioning* an ADR is read as citing it; that is the safe direction
        here — an ADR named by a plan that shipped should have a decided
        status either way, and a genuine exception belongs in
        `_STALE_PROPOSED_ALLOWLIST` where it is visible.
        """
        return set(re.findall(r"(adr_[A-Za-z0-9_]+\.md)", plan_text))

    def _plan_files(self) -> list[Path]:
        if not self._PLANS_DIR.exists():
            return []
        return [
            p
            for p in sorted(self._PLANS_DIR.glob("plan_*.md"))
            if not p.name.startswith("meta-plan_")
        ]

    def test_every_adr_has_an_index_row(self) -> None:
        """Every `.agents/adr/adr_*.md` file must have a row in
        `arch-principles.md`'s ADR Index table.

        A missing row is real drift: do not add rows for unrelated ADRs to
        make this pass, and do not narrow the assertion with a skip-list —
        report the gap instead. `_MISSING_INDEX_ROW_ALLOWLIST` is the one
        exception: a frozen, shrink-only record of pre-existing debt (see
        its docstring) — a NEW file missing a row still fails here.
        """
        adr_files = sorted(p.name for p in self._ADR_DIR.glob("adr_*.md"))
        if not adr_files:
            pytest.skip("No ADR files in .agents/adr/ (fresh checkout)")
        indexed = self._indexed_adrs(self._ARCH_PRINCIPLES.read_text())
        missing = [
            a
            for a in adr_files
            if a not in indexed and a not in self._MISSING_INDEX_ROW_ALLOWLIST
        ]
        assert not missing, (
            f"ADRs with no row in arch-principles.md's ADR Index: {missing}. "
            f"Add one row per ADR, in the table's existing format: "
            f"`| [adr_x.md](../../.agents/adr/adr_x.md) | <one-line decision> |`."
        )

    def test_no_adr_referenced_by_a_finalized_plan_stays_proposed(self) -> None:
        """A plan whose Status block reads `Step: finalized` has landed on
        main — the ADR it names documents a decision that shipped, so it
        cannot still read `Status: Proposed`.

        Scans every finalized plan, not just one feature's own — a stale
        status on an unrelated ADR is real drift to report, not something
        to quietly fix as a side effect of this test. `_STALE_PROPOSED_ALLOWLIST`
        is the one exception: a frozen, shrink-only record of pre-existing
        debt (see its docstring) — a NEW (plan, adr) pair still fails here.

        **This is a `/finalize`-time gate, by construction, not a branch
        gate.** `Step: finalized` is written by `/finalize` as the last act
        of landing a plan (see `meta-ai-config.md` "Plan Status Protocol"),
        so an in-flight branch's own plan reads something else and its ADR is
        never inspected here — measured on the branch that introduced this
        test: reverting its ADR to `Status: Proposed` left the suite green.
        That is the intended shape (an ADR is legitimately `Proposed` while
        its plan is in flight), but it means a stale status is caught on the
        way out, not on the way in. A per-branch check is a different test
        with a different condition — the plan named by
        `.claude/state/current_plan.md`, which is gitignored and per-worktree,
        so it can only ever be advisory.
        """
        plans = self._plan_files()
        if not plans:
            pytest.skip("No plan files in .agents/plans/ (fresh checkout)")
        stale: list[tuple[str, str]] = []
        for plan in plans:
            text = plan.read_text()
            step = self._plan_step(text)
            if not step or not step.startswith("finalized"):
                continue
            for adr_name in self._referenced_adrs(text):
                adr_path = self._ADR_DIR / adr_name
                if not adr_path.exists():
                    continue
                status = self._adr_status(adr_path.read_text())
                if (
                    status
                    and status.startswith("Proposed")
                    and (plan.name, adr_name) not in self._STALE_PROPOSED_ALLOWLIST
                ):
                    stale.append((plan.name, adr_name))
        assert not stale, (
            f"ADRs referenced by a finalized plan but still Status: Proposed "
            f"(plan, adr): {stale}. A plan that shipped documents a decision "
            f"that shipped — flip the ADR's Status (Accepted / Superseded / "
            f"Rejected, as appropriate)."
        )
