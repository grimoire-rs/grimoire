# ADR: Rules and agents no longer pack a support directory

## Metadata

**Status:** Accepted
**Date:** 2026-11-15
**Deciders:** Michael Herwig (owner)
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI transport unchanged; the rule and agent layer shrinks to one file)
**Domain Tags:** integration, api
**Supersedes:** [adr_multifile_rules.md](./adr_multifile_rules.md)
**Superseded By:** N/A

## Context

[adr_multifile_rules.md](./adr_multifile_rules.md) let a rule pack a sibling
directory (`rules/<name>/`). That put depth files where clients read rules.
Clients that read `rules/` recursively load every file in the directory as a
rule of its own. grim's repair was Claude-only: a `claudeMdExcludes` entry in
one `settings.json`, which Claude ignores when the repository is loaded from
another directory. Qoder has no usable exclude key. Other clients are
unaudited. Dependencies (`grim.requires.*`, new in 0.16.0) now give the
publisher a safe place for depth files: a skill the rule requires.

The per-kind catalog files also disagreed. A rule packed its whole directory,
an agent packed an allowlist (`README.md`, `logo.png`, `logo.svg`) and ignored
the rest silently, and the single-file kinds packed nothing beside the file.

## Decision Drivers

- Depth files must never load as rules, in any client, from any directory.
- One rule per kind for what a package contains, easy to state and to test.
- Publishers need a supported replacement before the old shape goes.

## Considered Options

### Option 1: Keep packing, widen the per-client exclusions

**Description:** Add exclude handling for every client that reads `rules/`.

| Pros | Cons |
|------|------|
| No breaking change | Qoder has no key; other clients unaudited |
| | Claude's exclusion stays directory-sensitive |

### Option 2: Refuse a support directory at pack time, replace it with a required skill (chosen)

**Description:** A rule and an agent are their `.md` alone. Depth files go in
a skill that the rule requires.

| Pros | Cons |
|------|------|
| No client can load depth files as rules | Breaking for publishers of support-dir rules |
| Same model for every client | Six vendored rules need migrating |

## Decision Outcome

**Chosen Option:** Option 2.

**Exception to Principle 9.** This is a deliberate breaking change, authorised
by the owner on 2026-11-15, despite the pre-1.0 freeze on breaking changes in
`AGENTS.md`. It ships in the next version, 0.16.0.

### Package content

| Kind | Packed |
|---|---|
| skill, hook | the whole directory, unchanged |
| rule, agent | the `.md` only |
| mcp, bundle, settings | the `.toml`, unchanged |

The directory beside a rule or agent that shares its stem (`rules/<name>/`,
`agents/<name>/`) may hold only `README.md`, `logo.png` and `logo.svg`. None
of them is packed. Any other file refuses the build with exit 65
(`SkillError::RuleSupportDir`, `SkillError::AgentStrayFiles`). This holds for
`grim build`, `release` and `publish`, and for a rule declared by local path
(`grim lock`, `update`, `add <path>`, `install <path>`). Messages link
`https://grimoire.rs/upgrading.html#rule-support-dir` and `#agent-companions`.

### Installing an old package

A rule already published with a support directory installs as before. grim
prints one warning per such rule when it writes files
(`support_dir_deprecation`). Updating to a single-file version removes the
directory and Claude's `claudeMdExcludes` entry.

### Replacement pattern

Depth files go in a support skill. The rule requires it with
`grim.requires.skills.<name>` in its `metadata` and names it in the body. The
skill stays model-invocable: never `claude.disable-model-invocation: "true"`,
optionally `claude.user-invocable: "false"`. Worked example:
`test/manual/catalog/rules/architecture-guide.md`.

### Catalog description lookup (`grim publish`)

First hit wins. README and logo fall back independently to the top-level
companion.

| Kind | README | Logo |
|---|---|---|
| skill, hook (dir `P`, named `S`) | `P/README.md`, `S.README.md` beside `P`, root | `P/logo.svg`, `P/logo.png`, `S.svg`, `S.png` beside `P`, root |
| rule, agent (`D/S.md`) | `D/S/README.md`, `D/S.README.md`, root | `D/S/logo.svg`, `D/S/logo.png`, `D/S.svg`, `D/S.png`, root |
| mcp, bundle, settings (`D/S.toml`) | `D/S.README.md`, root | `D/S.svg`, `D/S.png`, root |

`[description] entry_readme` is boolean only; the logo lookup has no toggle.

### Owner decisions (2026-11-15)

- (a) Refuse at pack time for every source, path-declared rules included,
  knowing it is breaking, shipping in the next version.
- (b) A rule's directory may hold README and logo; nothing in it is packed.
- (c) Agents follow: companions are no longer packed, a stray file refuses.
- (d) No `.grimignore` escape in those directories.
- (e) README and logo lookup with flat siblings for every kind; the directory
  form stays allowed for rules and agents.
- (f) Single-file kinds get the sibling lookup.
- (g) Migrating the six `ocx-sh/grimoire-lore` rules is a follow-up issue, out
  of scope.
- (h) First-party skills teach the pattern and the vendor options.
- (i) SVG is preferred over PNG at every level, including the top-level
  lookup (`assets/logo.svg`, `assets/logo.png`, `logo.svg`, `logo.png`). The
  top-level order shipped png first, so a repository holding both formats in
  one place publishes a different logo on its next `grim publish`.

### Implementer rulings, not owner decisions (open to being overturned)

| # | Situation | Ruling | Alternative |
|---|---|---|---|
| 1 | `.DS_Store` and similar junk in the directory | Built-in junk defaults are skipped, so no refusal | Refuse every file, junk included |
| 2 | A `.grimignore` in the directory | Counts as a stray file | Read it, which decision (d) rules out |
| 3 | The unreleased string form of `entry_readme` | Dropped, boolean only | Keep the string form for a custom README name |
| 4 | Turning the logo lookup off | No toggle | Add `entry_logo = false` |
| 5 | JSON `reason` for the refusal | No new slug; `code: data`, exit 65 | A dedicated slug per refusal |
| 6 | When the install warning fires | Only when files are written, not on a no-op install | Warn on every install, which repeats forever |
| 7 | `grim status` on a locked path rule that gained a support file | Warns, reports `outdated`, exits 0; `grim install` there is a no-op because the lock still describes the single-file rule | Fail `status` with 65 |

### Consequences

**Positive:**
- No client can load a rule's depth files as rules.
- One content rule per kind.

**Negative:**
- Breaking for publishers of support-dir rules and of agents with companions.
- Republishing produces a new digest.

**Risks:**
- The vendored rules under `.claude/rules/` (docs-quality, code-docs,
  rust-quality, rust-cargo, python-quality, typescript-quality) are old-format
  packages from `ocx-sh/grimoire-lore` and warn on their next update until
  migrated (follow-up, decision (g)).
- `claudeMdExcludes` handling stays, for old packages only.

## Links

- [adr_multifile_rules.md](./adr_multifile_rules.md) — superseded
- `src/skill/skill_package.rs`, `src/skill/skill_error.rs`,
  `src/install/installer.rs` (`support_dir_deprecation`),
  `src/command/publish.rs` (description lookup)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-11-15 | Michael Herwig | Initial draft, accepted |
