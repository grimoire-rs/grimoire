# Research: docs content survey, frontmatter drafts and check baseline

**Date:** 2026-09-06
**Run:** hex-plan xhigh, docs redesign (`.agents/plans/plan_docs_site_redesign.md`)
**Phase:** Discover, explorer content
**Consumers:** `.agents/specs/design_docs_site_redesign.md`, the plan.

# Discover: content and checks survey

Worktree: `.agents/worktrees/docs-plan` (branch `docs/use-case-discovery`).
21 content pages under `docs/src/*.md` (excludes `SUMMARY.md`, the nav file,
which is a 22nd markdown file but not a page). All 21 already carry a
`<!-- doc_type: ... -->` line-1 comment.

## 1. mdBook-specific syntax inventory

| Syntax | Count | Detail |
|---|---|---|
| `{{#include ...}}` | 0 | none used anywhere |
| `{{ path_to_root }}` / other `{{...}}` mdBook templating | 0 | the 4 `{{...}}` hits found are GitHub Actions YAML (`${{ secrets.X }}`) inside fenced code blocks in `ci.md` (3) and `ratings.md` (1) — false positives, not mdBook syntax |
| mdBook admonition `<div class="warning">` etc. | 0 | none |
| Raw HTML block (`<div>`, `<table>`, `<details>`...) | 1 | `docs/src/clients.md:39` `<div class="matrix-table">` wraps the client support-matrix table; styled by `docs/theme/clients-matrix.css` (theme file, not portable — table CSS classes need a Starlight equivalent or custom component). No visible closing `</div>` search needed — mdBook allows unclosed HTML blocks that markdown parsers may not tolerate; verify balance before porting. |
| `{#custom-id}` heading-anchor suffix | 289 total across 20 of 21 files (introduction.md has 0 — no non-H1 headings) | Per-file counts: publishing 44, commands 40, artifacts 20, vendor-metadata 22, json-interface 21, clients 18, package-index 14, configuration 13, mcp-servers 13, hosting-an-index 12, stability 12, ratings 11, upgrading 11, agents 8, concepts 7, authentication 7, self-hosted-gitlab 6, ci 5, installation 3, quickstart 2. This is docs-style.md's own mandated convention (`{#custom-anchor}`, `{#parent-subsection}`) — Starlight's remark autolink-heading generates slugs automatically, so these become either dead weight or must be preserved for anchor-link stability (394 relative-link anchors depend on them, see below). |
| Relative `./page.md` and `./page.md#anchor` links | 394 across 18 files (+ 21 in SUMMARY.md's nav list) | Starlight/Astro markdown does **not** rewrite `.md` links — every one needs either a remark-rehype link-rewrite plugin (strip `.md`, keep `#anchor`) or a mechanical find-replace pass. Heaviest: commands.md 91, publishing.md 48, configuration.md 52, artifacts.md 32, concepts.md 30, mcp-servers.md 21. Lightest: authentication.md 1, clients.md 2, hosting-an-index.md 2, upgrading.md 2. |
| Raw HTML inline (`<kbd>`) | 0 | none |
| Footnotes (`[^n]`) | 0 | none |
| Images (`![]()`\) | 0 in markdown body | static images exist (`og-card.png`, `favicon.png/svg`) referenced only from the mdBook theme (`index.hbs`), not from page markdown — theme-level, not a per-page migration concern |
| Unusual code-fence info strings | 0 unusual | Standard tags only: (blank) 222, sh 64, toml 46, yaml 27, json 25, console 23, text 9, powershell 2, markdown 2, jsonc 2. The 222 blank-fenced blocks feed DOC-EX-05 (below). |
| `robots.txt`, `privacy.html`, `start.html`, `demo.cast`, `.css`/`.js` assets | n/a | non-markdown static assets under `docs/src/`; carry over as static files, out of scope for markdown-syntax migration but need an Astro static-asset routing decision |

**Nothing blocks a mechanical migration on syntax alone** — no `{{#include}}`, no admonition divs, no footnotes. The two real costs are (a) rewriting 394+21 relative `.md` links (a remark plugin is cheaper than manual edits), and (b) deciding whether to keep or drop 289 manual `{#anchor}` suffixes now that Starlight autogenerates heading slugs — dropping them without updating the 394 links that target them would break anchors silently.

## 2. Frontmatter draft (title + description ≤160 chars, from each page's own first paragraph)

```yaml
- file: agents.md
  title: Agent Artifacts
  description: "Skills teach an agent a capability and rules constrain it — an agent bundles both into a reusable persona Grimoire can install."
- file: artifacts.md
  title: Artifact Reference
  description: "Grimoire ships five artifact kinds — skills, rules, agents, MCP servers, and bundles — each with its own file layout and install target."
- file: authentication.md
  title: Authentication
  description: "Most public skills and rules pull anonymously, but a private registry needs credentials — how grim resolves and stores them."
- file: ci.md
  title: Publishing from CI
  description: "Publishing by hand works until a second contributor bumps a version and conflicts arise — automate it from CI instead."
- file: clients.md
  title: Client Compatibility
  description: "grim installs one canonical artifact into many AI clients, and not every client supports every artifact kind equally."
- file: commands.md
  title: Command Reference
  description: "Every grim command follows the same shape: parse references into typed values, run the operation, then report the result."
- file: concepts.md
  title: Concepts
  description: "Grimoire borrows its mental model from package managers you already use, then adapts it for AI-agent configuration."
- file: configuration.md
  title: Configuration
  description: "Grimoire keeps configuration in two small files and a handful of environment variables, layered by scope."
- file: hosting-an-index.md
  title: Host Your Own Index
  description: "An index is the phone book grim browses — it answers what packages exist. Here is how to host your own."
- file: installation.md
  title: Installation
  description: "grim is a single self-contained binary. Once it is on your PATH there is nothing else to install."
- file: introduction.md
  title: Introduction
  description: "Grimoire is a package manager for AI-agent configuration, distributed through standard OCI registries."
- file: json-interface.md
  title: The JSON Interface
  description: "Every grim command that reports something offers --format json, forming a stable machine-readable interface."
- file: mcp-servers.md
  title: MCP Server Artifacts
  description: "Skills teach a capability, rules constrain behavior, and agents define a persona — MCP servers extend an agent with tools."
- file: package-index.md
  title: The Package Index
  description: "Most OCI registries cannot answer what packages exist — the package index is Grimoire's answer to that question."
- file: publishing.md
  title: Publishing Skills and Rules
  description: "Consuming artifacts is only half of Grimoire. The other half is producing and publishing them to a registry."
- file: quickstart.md
  title: Quick Start
  description: "This walkthrough declares a skill, installs it into a project, and shows the result end to end."
- file: ratings.md
  title: Artifact Ratings
  description: "An index lists what exists but says nothing about what is any good — ratings close that gap."
- file: self-hosted-gitlab.md
  title: Self-Hosted GitLab Setup
  description: "Everything grim does on github.com also works on a corporate GitLab instance, with a few setup differences."
- file: stability.md
  title: Stability and Versioning
  description: "Grimoire is pre-1.0 — this page documents which CLI, format, and pipeline contracts are frozen versus still evolving."
- file: upgrading.md
  title: Upgrading
  description: "CHANGELOG.md lists every change, one line per commit — this page covers upgrading grim itself version to version."
- file: vendor-metadata.md
  title: Vendor-Specific Metadata
  description: "Each AI client tool adds its own capability fields on top of the shared skill spec, namespaced under a metadata map."
```

Note: `vendor-metadata.md` has **no lead paragraph** — its first line after the H1 is directly a `##` subheading (`Why tool keys live in metadata`). The description above is drafted from that first subsection's opening sentence instead; the page itself may want a one-line intro added regardless of the migration (also relevant to DOC-TYPE-07/DOC-TYPE-26 shape below, though not currently flagged since it's a reference page with no preamble to measure).

## 3. docs-quality check demands, run over `docs/src/` as-is

Ran `page_type.py`, `landing_check.py`, `doc_examples.py`, `nav_depth.py`,
`doc_declaration.py` from `.claude/rules/docs-quality/checks/` against the
current tree. Counts per rule ID (current findings, before any new page is added):

| Rule ID | Count | Where |
|---|---|---|
| DOC-TYPE-07 (concept preamble >150 words before first `##`) | 5 | agents.md (152w), clients.md (185w), hosting-an-index.md (230w), mcp-servers.md (191w), package-index.md (153w) |
| DOC-TYPE-08 (troubleshooting entries not titled `Error:`/`Warning:`) | 1 | upgrading.md — 11 of 11 entries |
| DOC-TYPE-04 (narrative voice in reference prose) | 1 | json-interface.md:347 |
| DOC-DISC-16 (first-steps page, no runnable command block) | 1 | introduction.md |
| DOC-TYPE-10 (landing lead-in prose shape) | 1 | introduction.md:4 — 4 sentences vs. measured 1-sentence shape |
| DOC-TYPE-11 (landing reachable-action budget, word 150) | 1 | introduction.md:38 — first command/link menu at word 222 |
| DOC-EX-05 (fence carries no language tag, invisible to drift checks) | 11 | commands.md ×3, publishing.md ×3, vendor-metadata.md ×2, concepts.md, package-index.md, ratings.md ×1 each |
| DOC-NAV-01..* | n/a | "not applicable: no docs-site generator config found" — nav_depth.py needs a generator config (mdBook `book.toml` / Starlight `astro.config`) to run at all; currently inert until the Starlight scaffold exists |
| doc_declaration.py findings | 0 | all 21 pages already declare `doc_type` correctly |

**What a NEW how-to page must satisfy** (page_type.py, cites page-types.md):
- **DOC-TYPE-22** (MUST): state the goal/scope in prose before the first `##` — zero stripped-prose words between H1 and first `##` fails.
- **DOC-TYPE-23** (CONSIDER): state a hard prerequisite before step 1; only header it at 3+ prerequisites.
- **DOC-TYPE-24** (SHOULD): numbered reader actions for a fixed-order procedure, one heading per independent choice, one fenced command per action.
- **DOC-TYPE-25** (SHOULD): close with a "See also"/"Next steps"/"What's next"/"Related" heading.
- **DOC-TYPE-03** (MUST): no learning-opener + conditional-instruction mix (that's tutorial/how-to conflation).
- Fence marking (examples.md): a bound example needs a `# doc: <id>` / `# title: ...` / `# expect_exit: N` header comment so `doc_examples.py --harness` can run it. **Zero pages in the repo currently use `# doc:` fence binding** — this is a new convention the plan introduces, not a migration of existing content.

**What a NEW tutorial page must satisfy**:
- **DOC-DISC-17** (MUST): no branching choices, including prose branches ("or, with", "alternatively", "if you prefer") — one path only; alternatives belong in a how-to instead.
- **DOC-DISC-18** (SHOULD, reading heuristic): every step shows a visible result (printed value, new file, rendered page) — a `>>>` line or `# prints` comment satisfies it.
- **DOC-DISC-19** (CONSIDER): only type as tutorial when the reader must assemble 2+ interacting concepts, not a single linear setup.
- **DOC-DISC-16** (SHOULD): under ~100 words before the first command.
- Zero real tutorials exist in the calibration corpus per page-types.md's own "Not studied" section — the contract has never run against a real tutorial page, so treat DOC-DISC-17/18/19 as unvalidated on first real use.

**Landing page** (landing_check.py, `introduction.md` is the only `doc_type: landing` page):
- DOC-TYPE-10 (lead-in prose shape), DOC-TYPE-11 (reachable action by word 150), DOC-TYPE-12 (CTA/task-link budgets), DOC-TYPE-13 (who-it's-for, true-zero case only) — scoped to `doc_type: landing` pages only.
- DOC-TYPE-14 (placeholder text) and DOC-TYPE-15 (unsourced adoption claims) — run on **every** page, not just landing.
- `introduction.md` currently fails DOC-TYPE-10 and DOC-TYPE-11 (see table above) — cite both directly as acceptance criteria if the plan rewrites this page.

## 4. use-cases.yaml drift rows

`drift:` list header at `.agents/discovery/use-cases.yaml:805`, five entries, `.agents/discovery/use-cases.yaml:806-824`:

1. **`commands.md:806-810`** — page `docs/src/commands.md`, where "grim search": *"one paragraph says the plain table names the serving registry in a source column, a later paragraph says source is JSON-only and the table keeps five columns; the binary prints no such column"* (log T06).
2. **`quickstart.md:811-814`** — page `docs/src/quickstart.md`, where "step 2 / step 3": *"promises materialisation 'into the clients it detects'; in a fresh project with no client marker the artifact lands in the agents pool, which Claude Code does not read, with no warning printed"* (log T02, T03, T08).
3. **`commands.md:815-818`** — page `docs/src/commands.md`, where "grim install": *"the stale-lock refusal (exit 65, declaration_hash mismatch) is not documented under the command that raises it"* (log T09) — this is the same gap surfaced independently in the coverage table (`use-cases.yaml:775`) and `product_observations` (`use-cases.yaml:831`).
4. **`json-interface.md:819-822`** — page `docs/src/json-interface.md`, where "status report": *"state value 'stale' is listed and defined nowhere; the outputs array drops a dropped client's still-on-disk output silently"* (log T08, T07).
5. **`configuration.md:823-824`** — page `docs/src/configuration.md`, where "Qualified references": *"alias/repo syntax is shown only for oci-type aliases; against an index-type alias it fails with a parse error and the page does not say whether that is intended"* (log T06).

## 5. commands.md sections and line ranges

| Topic | Lines | Notes |
|---|---|---|
| `grim tui` key map / badges | 1387–1650 (section), key-map table at ~1462 (`\| Key \| Action \|` header), badges at 1554–1650 (`### Detail tabs and repository docs`) plus badge prose at ~1465-1660 (`+ pending` badge, `↑ outdated` badge) | Full `## grim tui {#tui}` section runs 1387 to 1650 (next `##` is `grim build` at 1651). |
| `grim status` vocabulary | 592–742 | `## grim status {#status}` 592–677; `### grim status --check {#status-check}` 678–742. Vocabulary itself (installed/outdated/locally modified/integrity-missing/not installed) stated at lines 592–595. |
| `grim install` stale-lock refusal | **not present in commands.md** | The `## grim install {#install}` section (461–529) documents the modified-artifact refusal and the not-owned-path refusal, but **not** the stale-lock refusal. That behavior is real (exit 65, message *"grimoire.lock is stale (declaration_hash {locked} does not match current {current}); run `grim lock` before installing"*, defined at `src/command/command_error.rs:24`) but is a **documentation gap**, confirmed by `use-cases.yaml:274/775/816/831` above. The plan should treat this as new content to write into the install section, not text to migrate. |

## Blockers for a mechanical migration

None on markdown syntax (no includes, no admonition HTML, no footnotes). Two
non-mechanical items the plan must decide explicitly:
1. Whether to keep the 289 `{#anchor}` suffixes (Starlight autogenerates slugs) or drop them — dropping breaks the 394+ relative-link anchors unless links are re-derived from Starlight's own slug algorithm.
2. `clients.md`'s `<div class="matrix-table">` + `docs/theme/clients-matrix.css` pairing needs a Starlight-side equivalent (custom CSS import or a component) since Starlight doesn't ship an mdBook-theme CSS hook.
```
