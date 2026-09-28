# Research: bringing PR #98 (hooks ArtifactKind) current with main

Date: 2026-09-28
Question: what does it take to bring `origin/hex/hooks-artifact-kind` (PR
[grimoire-rs/grimoire#98](https://github.com/grimoire-rs/grimoire/pull/98))
current with `main`, and what has main changed since the merge base
(`d6cecb85`, 2026-08-28) that obliges the PR to change beyond textual
conflicts?
Sources: `git log`/`git diff` between `d6cecb85`, `origin/main`,
`origin/hex/hooks-artifact-kind`; `.claude/rules/docs-style.md`,
`docs-quality.md`; `docs/src/content/docs/*.md` on main;
`.agents/discovery/use-cases.yaml`; `gh pr view/checks 98`.

## PR shape (11 commits, +70,302/-750, 251 files)

| Area | Files |
|---|---|
| `src/` | 76 |
| `.agents/` (planning artifacts) | 76 |
| `test/` | 46 |
| `catalog/` (hooks example artifacts, skill refs) | 25 |
| `.claude/` (rules) | 9 |
| `docs/` | 13 |
| misc (`Cargo.{toml,lock}`, `AGENTS.md`, taskfiles, `.github/`) | 6 |

Roughly 30% code (`src`+`test`), 30% planning artifacts, the rest
docs/catalog/config. The `+70k` line count is dominated by `.agents/`
design/ADR/review-round records and `test/uv.lock`, not reviewable diff.

CI on the PR's head commit (82757f0c) is green (Acceptance, Unit, Deep,
Smoke, Conventional Commits, DCO, Supply Chain all pass) — but those runs
executed against the stale base `d6cecb85`. `gh pr view 98` reports
`mergeable: CONFLICTING`, `mergeStateStatus: DIRTY`. No human review
comments found via `gh pr view --comments` (only bot test-result comments).

## 1. Docs: mdBook → Astro Starlight breaks every docs file the PR touches

Main's `77558454` (2026-09-07) replaced mdBook with Astro Starlight,
`48e70fc9`/`cf86001c` restructured content and added twelve use-case pages.
Result:

- `docs/src/SUMMARY.md` **no longer exists** on main — nav comes from
  Starlight's content collection, not a manual TOC. The PR's commit
  `82757f0c` edits `docs/src/SUMMARY.md` to add a `Writing Hooks` entry;
  that file and that mechanism are gone.
- All docs pages moved from `docs/src/*.md` to
  `docs/src/content/docs/*.md`. The PR still edits/adds at the old path:
  `docs/src/hooks.md`, `docs/src/artifacts.md`, `docs/src/clients.md`,
  `docs/src/commands.md`, `docs/src/concepts.md`,
  `docs/src/configuration.md`, `docs/src/json-interface.md`,
  `docs/src/mcp-servers.md`, `docs/src/package-index.md`,
  `docs/src/publishing.md`, `docs/src/stability.md`,
  `docs/src/vendor-metadata.md`. Every one of these has a live,
  independently-edited counterpart at `docs/src/content/docs/<name>.md` on
  main — this is a rename+diverge conflict, not a clean move.
- New page format is mandatory
  ([docs-style.md](../../.claude/rules/docs-style.md),
  [docs-quality.md](../../.claude/rules/docs-quality.md)): YAML frontmatter
  (`title`, `description`) plus `<!-- doc_type: ... -->` /
  `<!-- doc_tier: ... -->` HTML-comment declarations
  (`doc_type` ∈ tutorial/how-to/reference/explanation/troubleshooting/
  runbook/landing/readme/changelog; `doc_tier` ∈ first-steps/everyday/
  integration). `docs/src/hooks.md` in the PR has none of this — it's
  plain mdBook markdown with a `# Writing Hooks` H1, which Starlight now
  renders from frontmatter and must not duplicate. `task docs:check` (via
  `docs/check_urls.py` and the new `docs-quality/checks/*.py` — prose,
  page_type, doc_declaration, nav_depth, links_raw, landing_check) would
  fail this page as-is.
- **Use-case page**: `.agents/discovery/use-cases.yaml` on main already
  lists `surface: "hooks (issues #85-#97, PR #98)"` as a planned use-case
  — the redesign anticipated this feature but no use-case page for hooks
  exists yet under `docs/src/content/docs/`. The twelve pages added in
  `48e70fc9` (browse, first-skill, seven lifecycle guides,
  catalog-best-practices, team-ci, registries, own-index) don't include
  one for hooks; landing this PR without adding one leaves the gap the
  redesign flagged.

## 2. Rules/AI-config obligations added since the base

`.claude/rules/` diff `d6cecb85..origin/main` (not touched by the PR) adds
whole new rule families: `docs-quality.md` + `docs-quality/*` (checks,
fixtures), `rust-quality.md`/`rust-cargo.md` (+ subdirs), `python-quality.md`/
`python-packaging.md` (+ subdirs), `typescript-quality.md`/
`typescript-packaging.md` (+ subdirs), `hex-state.md`. Modified:
`.claude/rules.md`, `arch-principles.md`, `docs-style.md`,
`meta-ai-config.md`, `product-context.md`, `subsystem-cli.md`,
`subsystem-cli-api.md`, `subsystem-cli-commands.md`,
`subsystem-config-keys.md`, `subsystem-file-structure.md`,
`subsystem-taskfiles.md`, `subsystem-tests.md`,
`vendor-capability-watchlist.md`, `AGENTS.md`.

The PR independently edits (and will textually conflict on) `.claude/rules.md`,
`arch-principles.md`, `product-context.md`, `subsystem-cli-commands.md`,
`subsystem-file-structure.md`, `vendor-capability-watchlist.md`, plus adds
`arch-threat-model.md`.

Obligations that apply regardless of merge strategy:

- **Principle 9 (additive-only freeze)** — AGENTS.md now states 1.0.0
  stabilization is in force on main; any schema/layout/renderer touch by
  the PR (it adds `ArtifactKind::Hook`, a new install-state kind, new JSON
  report shapes in `src/api/hook_report.rs`) needs to be checked against
  `docs/src/content/docs/stability.md` and additive-only rules — a fresh
  variant is fine, changing an existing enum/field is not.
- **vendor-capability-watchlist.md** fires on `src/install/vendor_*.rs`,
  `src/oci/mcp.rs`, `src/catalog/rating_provider.rs` — the PR edits
  `vendor.rs` and `vendor_claude.rs` for hook dispatch; the rule (changed
  on main) must be re-read for current upstream-capability claims before
  hook support per-vendor is written or reviewed.
- **subsystem-config-keys.md** fires on `src/command/config_keys.rs`,
  `src/config/declaration.rs` — the PR touches `config_keys.rs`
  (experimental `options.experimental.hooks` flag), so title/description
  style conventions in this rule (also changed on main) apply.
- **catalog/README.md drift duty** — PR adds `catalog/hooks/*` (two example
  hooks) and catalog skill reference docs; CLI/docs changes require a
  catalog-skill drift review per that file's procedure, gated by
  `task catalog:verify`.
- **docs-quality gate** — see §1; this is a hard CI gate now
  (`task docs:check`) that didn't exist at the PR's base.

## 3. Code-seam collisions (semantic, not just textual)

- **`92699721` "extract declare, client-list, staging, rebind and
  roll-forward seams"** (2026-09-27, the day before this snapshot) rewrote
  `src/command/add.rs` (+1418/-?), `src/install/installer.rs` (+414),
  `src/install/target.rs` (+138), `src/command/update.rs` (+246),
  extracting `declare_reference`, `parse_client_list`,
  `stage_locked_artifact`, `roll_forward`, `rebind_agent_name`/
  `rebind_skill_name`, `Identifier::or_latest`. The PR modifies these same
  files (`add.rs`, `installer.rs`, `target.rs`, `update.rs` are all in the
  PR's `src/` file list) to wire in hook dispatch/install logic written
  against the **pre-refactor** shape of add/install/update — a rebase or
  merge will not just show line conflicts, the hook install path likely
  needs to be re-plumbed through the new seam functions rather than
  reapplied to the old monolithic functions.
- **Vendor client work**: main added `src/install/vendor_qoder.rs` (new
  client) and non-trivially changed `vendor_claude.rs` (+268/-),
  `vendor_codex.rs` (+96/-), `vendor.rs` (+113/-) since base. The PR
  changes the *same* `vendor.rs` (+1307/-) and `vendor_claude.rs` (+130/-)
  to add hook install/dispatch per client. Two independent large rewrites
  of the vendor dispatch surface — conflict resolution requires
  understanding both the new client-registration shape (Qoder) and the
  new hook-registration shape, not a mechanical patch apply.
- **`grim export plugin`** (`714cf920`, `47c0bc0a`, plus follow-ups
  `9ddbb756`/`7e50668c`/`4884a639`/`33538585`): `src/command/export.rs` has
  no `ArtifactKind::` exhaustive match today (spot-checked — no direct
  hits), so it likely iterates artifact kinds generically; still needs
  verification once ArtifactKind::Hook exists — Claude/Codex plugin
  manifests can themselves carry hooks, so whether `grim export plugin`
  should surface a project's grim-managed hooks into the exported
  `plugin.json`/marketplace entry is an open **feature-scope** question,
  not just a merge-conflict question. Not addressed by the PR (predates
  export-plugin) or by main (predates ArtifactKind::Hook).
- **`.grimignore` (`d46709d7`)**: packing/hashing now honors `.grimignore`.
  The PR's `catalog/hooks/*` example artifacts (shell scripts, `hook.toml`)
  and any hook-artifact packing path in `src/install/installer.rs`/
  `src/skill/local_pack.rs` need to be checked against this — hook payload
  files (e.g. `guard.sh`) must not be silently excluded by a `.grimignore`
  pattern, and the PR predates this mechanism entirely.
- **Upstream follow-ups (`3194d0dd`, `81dbed05`)**: vendor capability
  tracking for existing clients — same file (`vendor.rs`) touched by both
  main and the PR; substantive not just line-level, since capability flags
  gate what a hook-aware install path may do per client.
- **`json_splice.rs` escaping (`2043a627`)**: main made a focused 24-line
  fix to `json_splice.rs` escaping. The PR does not touch this file
  (confirmed: 0 lines changed by main is wrong — main changed 27
  lines total in this file since base; the PR's file list does *not*
  include `json_splice.rs`, so no conflict there, but any hook-generated
  JSON splicing added by the PR elsewhere should reuse the fixed escaping
  rather than reintroduce the bug it fixed).
- **`2a436191` "resolve a relative --config to an absolute workspace"** is
  a PR-authored fix touching `src/cli/options.rs`/scope resolution — this
  is exactly the kind of general CLI-scope fix that may have been
  superseded or generalized by unrelated main work; not independently
  verified here, flagged for the merge author to diff against current
  `scope_resolution.rs`.

## 4. Conflict surface size (from prior `git merge-tree`)

36 files conflict textually per the earlier `git merge-tree
--write-tree --name-only`, spanning `src/install/*`,
`src/command/{add,config,config_keys,status,update}.rs`, `src/error.rs`,
`src/main.rs`, `Cargo.lock`, `docs/src/SUMMARY.md`,
`docs/src/clients.md`, three `docs/src/content/docs/*.md` pages, catalog
skills, `.claude/rules*`, `AGENTS.md`, and test files. Of these, the
`src/install/*` and `src/command/{add,update}.rs` conflicts are the ones
with *semantic* depth (§3), not just line drift; the docs conflicts are
total path/format obsolescence (§1), not resolvable by patch application.

## Evidence for/against each strategy

**(A) Rebase 11 commits onto main**
- For: preserves the PR's own commit-level narrative/ADRs
  (`c37639a6` "record the hooks design, ADRs, plan and review rounds");
  keeps `.agents/` planning trail attributable to this feature.
- Against: each of the 11 commits touches the seam-refactored files
  (`add.rs`, `installer.rs`, `target.rs`) or the moved docs — rebase would
  require re-resolving the same semantic conflict at *every* commit that
  touches them (up to 11 times), and the docs commits (`a8818203`,
  `82757f0c`) would need near-total rewrites mid-rebase since the target
  files/format no longer exist.

**(B) Merge main into the branch**
- For: single conflict-resolution pass instead of per-commit; standard
  for a long-lived feature branch 90 commits behind.
- Against: a single giant merge commit conflates "resolve textual
  conflict" with "redesign hook install path against the new seam
  functions" and "rewrite docs for Starlight" — reviewers get one commit
  bundling mechanical and design changes, harder to audit than either A or C.

**(C) Re-cut fresh branch from main, port the feature (possibly split PRs)**
- For: hook install/dispatch logic can be written directly against the
  post-refactor `declare_reference`/`stage_locked_artifact`/`roll_forward`
  seams instead of retrofitted; docs pages authored directly in Starlight
  format with correct `doc_type`/`doc_tier` and a use-case page from the
  start; smaller PRs (e.g. ArtifactKind::Hook core, then catalog examples,
  then docs) are easier to review than 251 files at once, and the
  `.agents/` planning artifacts (76 files, ADRs/design/review rounds) could
  land once as reference rather than needing conflict resolution.
- Against: most expensive option — full reimplementation, and 11 commits
  of authored review/design intent (`c37639a6`) would need to be
  re-attributed or discarded; loses the exact history of what each round
  of adversarial review already caught.

## Numbers for the owner

- 251 files, +70,302/-750; ~76 src, ~76 `.agents/`, 46 test, 25 catalog, 13
  docs, 9 `.claude/`.
- 90 commits / 732 files changed on main since the merge base.
- 36 files conflict per `git merge-tree`; CI green but stale (ran against
  `d6cecb85`, not current main); GitHub reports `CONFLICTING`/`DIRTY`.
- No human PR review comments found.
