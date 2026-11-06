# Research: splitting two over-budget rules (#280, #281)

## Metadata

**Date:** 2026-11-05
**Domain:** devops (AI configuration)
**Triggered by:** `/hex-plan` for `.agents/plans/plan_settings_follow_ups.md`, issues [#280](https://github.com/grimoire-rs/grimoire/issues/280) and [#281](https://github.com/grimoire-rs/grimoire/issues/281); `.claude/rules/meta-ai-config.md` › Research Protocol
**Expires:** 2027-05-05

## Direct Answer

1. Claude Code discovers every `.md` under `.claude/rules/` recursively. A file with no
   `paths` loads at launch like `CLAUDE.md` ([memory docs](https://code.claude.com/docs/en/memory#set-up-rules)).
   So a data file inside `.claude/rules/`, even in a subdirectory, loads as a rule unless
   it carries a never-matching `paths:` glob.
2. `paths` takes a YAML list or a comma-separated string, with `**` and brace expansion.
   A rule loads on Read, Write or Edit of a matching file, or a single-file `cat`/`head`
   (Write/Edit since v2.1.288, Bash reads since v2.1.293, per the
   [changelog](https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md)).
   After compaction it reloads on demand. Only `paths` is read from frontmatter
   ([frontmatter reference](https://code.claude.com/docs/en/memory#rule-frontmatter-reference)).
3. Nothing documents a plain markdown link in a rule as loading the target. `@path`
   imports are documented for `CLAUDE.md` only; do not rely on them in rules.
4. Claude Code has no mechanism for non-rule data inside `.claude/rules/`. A directory
   outside it is the safe home for data a rule points to. This repo's precedent for dated
   upstream data is the ledger `.agents/upstream-checks.md`, read by the same script.
5. No upstream guidance on many small rules versus a few large ones beyond "one topic per
   file" and `paths` to cut context noise.

## Key Findings (local)

- **No rule-budget test.** The 200-line budget is prose in `meta-ai-config.md:44-52`;
  `test_ai_config.py` checks only `AGENTS.md` (`:481`, at most 280) and the `CLAUDE.md` stub.
- **Tests see top-level rules only.** Every rule test globs `rules/*.md`. Relevant checks:
  dead globs (`:411-435`), no subsystem glob ending `/*.rs` (`:437-451`), declared overlaps
  by exact glob string (`:951-1003`), catalog coverage (`:604-630`), AGENTS.md subsystem names
  in the catalog (`:748`), backticked `*.md` names resolve anywhere (`:700-740`).
- **The staleness script.** `upstream_stale.py:23` hard-codes
  `WATCHLIST = ".claude/rules/vendor-capability-watchlist.md"`; `report()` globs, and a
  pattern matching nothing reports `MISSING`. Tables count when the header matches
  `^\|\s*(Capability|Variable)\s*\|`; a row's date is its inline `verified YYYY-MM-DD`, else
  the section's ``All rows `verified DATE` `` sentence, which any heading resets.
  `test_upstream_stale.py:23`, `:33`, `:336`, `:357` pin the path.
- **Watchlist (375 lines).** Rule body is about 98 lines (intro, re-verify procedure,
  compensation classes, fragility note); the dated sections run 86-375 (277 lines).
- **File-structure rule (984 lines), section map.** Design Rationale 1-27; Install Layout:
  Skills 28-39, Rules 40-158, MCP 159-202, Hooks 203-330, Agents 331-391, Settings 392-449,
  Global-scope paths 450-573; Install State 574-648 (project, global, telemetry id); PathAnchor
  set 649-731; anchor table 732-800; containment guard 801-835; Client Detection 836-944;
  Catalog Cache 945-968; Constraints 969-976; Cross-References 977-984.
- **Inbound references** to the file-structure rule: `AGENTS.md:84`, `:102`, `:111`;
  `.claude/rules.md:76`, `:87`, `:96`, `:142`, `:145`; skills `code-check:39`, `builder:40`,
  `security-auditor:31`, `meta-validate-context:33,63`; `deep-pass.md:18`; `hex.md:34`;
  `post_tool_use_tracker.py:32` (a dead glob `src/file_structure/**`); source comments in
  `path_anchor.rs`, `hook_dispatch.rs`, `hook_registrar.rs`. To the watchlist:
  `.claude/rules.md:40`, `:97`, `:105`; `arch-threat-model.md:171`; `docs-style.md:92`;
  `hex.md:36`; `domains.md:17-19`; `SKILL.md:23`; `researcher-brief.md:32`;
  `upstream_stale.py:23`; `post_tool_use_tracker.py:121`, `:131`.
- **Latent defect, out of scope.** Vendored subdirectories `.claude/rules/{rust-quality,python-quality,typescript-quality,docs-quality,rust-cargo}/`
  hold about 60 `.md` files (about 8,000 lines plus 40 fixtures) with no `paths:`, so Claude
  Code loads them always-on, while their index rules route to them as on-demand depth. The
  vendored `code-docs/*.md` already use a never-matching glob (`**/.code-docs-depth-on-demand`).
  Not verified with `/context`. Fix belongs upstream (`ocx-sh/grimoire-lore`).

## Recommendation

- **#280:** keep the rule body in `.claude/rules/vendor-capability-watchlist.md` with its
  `paths:`; move every dated section verbatim, heading and default sentence included, to
  `.agents/upstream-watchlist.md`, beside the ledger the same script reads. Re-point
  `upstream_stale.py` and its test; compare the report before and after.
- **#281:** keep `subsystem-file-structure.md` as a small core and index on `src/**` and
  `crates/**`, so every inbound reference, `AGENTS.md` included, stays valid. Move the other
  sections verbatim into flat `subsystem-*.md` parts with narrower `paths:` (no glob ending
  `/*.rs`, each matching at least one file), each under 200 lines by `wc -l`. Declare any
  shared glob string in `.claude/rules.md`.
- File the vendored-subdirectory defect as its own issue.

## Sources

- https://code.claude.com/docs/en/memory — rules, `paths`, recursion, imports, compaction (current docs)
- https://code.claude.com/docs/en/large-codebases — reference content belongs in skills (current docs)
- https://code.claude.com/docs/en/skills — supporting files load on demand (current docs)
- https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md — v2.1.84, v2.1.211, v2.1.288, v2.1.293
- Local: `.claude/rules/meta-ai-config.md`, `.claude/tests/test_ai_config.py`, `.claude/tests/test_upstream_stale.py`, `.claude/skills/upstream-refresh/scripts/upstream_stale.py`

Search terms: `claude code .claude/rules recursive`, `claude code rule paths frontmatter`,
`claude code memory imports rules`.
