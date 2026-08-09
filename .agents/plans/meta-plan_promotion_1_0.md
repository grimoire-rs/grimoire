# Meta-Plan: 1.0 Promotion & Public-Surface Readiness

## Status

- **Plan:** meta-plan_promotion_1_0
- **Active phase:** 3 — **every workstream marked "Blocks 1.0: yes" is done.**
  W1-A, W1-B, W2-A, W2-B, W2-C, W3-A, W3-B, W4 all complete. What remains is
  non-blocking (W1-C spec'd not built, W3-C, W6 in flight, W7 retag) or human
  (W9 launch, W10 ecosystem, the social-preview uploads, Open VSX).
- **W3-A done** (`afde8ad`): `SECURITY.md` at the repo root — `grimoire-rs/.github`
  does not exist (404), so the org-default pattern was unavailable and the repo
  root is both where `README.md:75` already links and what 5/5 peers do. The
  policy leads with integrity-vs-authenticity: grim pins and verifies SHA-256,
  and verifies **no signatures at all** — confirmed against source, every
  cosign/sigstore/notation grep hit was a false positive. Structure follows uv
  (closest peer by scale, explicit non-guarantees, no SLA) rather than the CNCF
  committee template. Also names the previously undocumented surface: an `mcp`
  descriptor's `command` is written into the client config verbatim from the
  registry and launched by the *client*.
  Same commit corrects `.claude/rules/quality-security.md`, which claimed
  manifest signature validation as a control that does not exist, and warned
  about macOS Mach-O signing, `${installPath}` templating, decompression bombs,
  and setuid preservation — none of which correspond to shipped code.
- **W3-B done** (repo settings, authenticated `gh`, 2026-07-26): private
  vulnerability reporting `{"enabled": true}`, secret scanning `enabled`, push
  protection `enabled`, Dependabot security updates `enabled` (needed
  `vulnerability-alerts` enabled first — the `automated-security-fixes` PUT
  returns 422 otherwise). All four were `disabled` before this pass.
- **W5 outcome:** shipped on `main` at `6fc2bea` (docs deploy run 30206864421
  green). Landing page, vendor-direct client marks, left-aligned compat matrix
  with client marks, `docs/seo.py` (canonical + OG + Twitter + sitemap),
  `robots.txt`, mdBook pinned to 0.5.3 in CI and `docs/README.md`, and
  `lfs: true` on the docs checkout — without which `docs/src/og-card.png`
  (LFS-tracked) would have deployed as a three-line pointer and every share
  card would have resolved to ASCII. **Not verified:** the live `og:image`
  round-trip; the fetch tool converts to markdown and drops `<head>`.
  **Human-only remainder:** upload `docs/src/og-card.png` at Settings →
  General → Social preview, per repo. GitHub exposes no API for it.
- **W8 outcome:** topics were already applied on all four repos before this
  pass (grimoire 20, -vscode 20, setup-grimoire 14, index 11) and the D9
  `gimoire.rs` typo was already fixed — both no-ops. Applied: the 299-char
  About on `grimoire`, `homepage` on `setup-grimoire` and `index` (both were
  null). `grimoire-components` is not a GitHub repo under `grimoire-rs` (404),
  so W8 is a three-repo workstream, not four. **Human-only remainder:** Open
  VSX publish (needs a token) and a Marketplace republish — `package.json`
  already lists `AI` first, the live v0.2.4 listing predates it.
- **W1-B outcome:** grim never required the kind segment, so the fix was a
  *recommendation*, not an interface change. No alias, no republish, no ADR.
  Published paths stay frozen. Shipped `70a39f7`. Consequence for W4: the
  README keeps the `skills/grim-usage` form — a bare short id will never
  resolve for first-party packages, by decision.
- **Branch convention (user, 2026-07-26):** `feat/1.0-readiness` on the primary
  checkout is the **long-lived root branch** for this meta-plan. Every
  workstream merges into it; it lands on `main` once, not per workstream. Only
  applies to `grimoire` — sibling repos take their own branch each.
- **Step:** W2-B and W2-C committed, **not pushed**. `feat/1.0-readiness` holds
  `ce384f8` (CONTRIBUTING `## License`: Apache-2.0 inbound=outbound, DCO
  sign-off, no CLA, "The Grimoire Authors" = `git shortlog -sne`) and `622e306`
  (`task git:dco` + a pull-request-only CI job enforcing it). Sibling repos:
  `setup-grimoire` `chore/license` (`b8a8556`, 15-line stub → full Apache
  text), `grimoire-index` `chore/license` (`a2618e1`, had no LICENSE at all).
  All three LICENSE files byte-identical to `grimoire/LICENSE` (11317 bytes,
  `cmp` verified). `task verify` green on the committed tree; `task git:dco`
  green on the branch's own commits. Sibling repos have no gate to run and
  neither diff touches executable code.
  **DCO scope:** the check is `pull_request`-only — main's existing history
  predates the term and is not retroactively in scope. Commits on
  `feat/1.0-readiness` are signed off; **anything merged into it from here on
  must be too**, or its PR fails.
- **W4 done** (`b02d318`): the headline "A package manager for AI-agent config
  — skills, rules, agents, and MCP servers, installed into every coding agent
  you use" replaces "An OCI-backed package manager for AI skills and rules" on
  all five surfaces (README, `Cargo.toml`, `docs/book.toml`, `CATALOG.md`,
  `docs/src/introduction.md`), plus `CLAUDE.md` and `product-context.md`, which
  now records both the headline and the fixed sentence two as canonical so they
  cannot drift apart again. A grep for the old string returns nothing outside
  stale worktree dirs and generated `docs/book/`. The landing-page hero ("A
  lockfile for your AI-agent config.") was deliberately left alone — it is a
  hero line, not the headline, and it already reads well.
  Also `40bfa57`: `verify-basic.yml` gains the `concurrency:` guard that
  `subsystem-ci.md` requires and it alone was missing.
- **Sibling repos done, pushed to `main`** (explicit user grant, that scope
  only): `setup-grimoire` at `a3eaffd` (full Apache LICENSE, CONTRIBUTING, DCO
  via `.github/scripts/dco.sh` — no task runner there — job added to the
  existing `test.yml`); `grimoire-index` at `ca464ce` (LICENSE, CONTRIBUTING,
  `dco:` task + its own `dco.yml`). **Both verified against the GitHub API, not
  the agents' reports.** The index agent found that `validate-pr.yml` runs on
  `pull_request_target`, so the job I told it to put there would have executed
  a PR-controlled `taskfile.yml` under a privileged event — it correctly used a
  separate `pull_request` workflow instead. It also excluded `index/**` from
  the DCO trigger, verified against `60c18ed`: a real announce commit, non-bot
  author, unsigned, index-only. If **DCO Sign-off** ever becomes a required
  check there, an index-only PR will hang waiting for a skipped job.
- **W6 done** (`f34f05c`): a real recorded demo on the landing page. The
  `.cast` is produced by `test/recordings/` driving the actual binary against
  the acceptance suite's live registry — `grim init` → `grim add` → a `find`
  across `.claude/`, `.cursor/`, `.opencode/` → `grim status`, with a real
  digest (`sha256:95984318b0a6`) and the same skill in three client trees.
  **Reworked at user request** (`ed477f2`) — the GIF was rejected on two
  counts, both fair. It shipped as a raster animation (no pause, no copy) and
  its tables were misaligned. Root cause of the misalignment: `printer.rs`
  pads each column to the width it actually rendered, and the recorder's
  `sanitize()`/`shorten_digests()` rewrote those bytes afterwards, so headers
  sized for a long tmp path and a 64-hex digest stood over shortened values.
  Fixed at the source — all post-hoc rewriting deleted, recorded against the
  real public `ghcr.io/grimoire-rs/skills/grim-usage` (anonymous pull verified
  via GHCR token issuance: 200 for the real repo, 403 for an invented one), a
  short flat tmp dir instead of pytest's nested `tmp_path`, full digest shown.
  `assert_tables_column_aligned()` now runs in `task test:demo` and was proven
  to catch a synthetic misaligned cast.
  Ships as vendored asciinema-player 3.17.0 (Apache-2.0, sha1+sha512 verified
  before extraction), no external host, `<noscript>` → raw `.cast`.
  **Cost: 203.6 KiB vs the GIF's 48 KiB — 4.2×.** No smaller real build keeps
  pause/seek/copy; deferring the script behind an IntersectionObserver is the
  available lever if the weight matters.
  **W9-a Console.dev now has all three inputs (W4 + W5 + W6).**
- **W3-C done** (`a42a6b2`): cargo-deny was vendored, configured, and wrapped
  in a task that **CI never ran** — the deny.toml allow-list gated only a
  developer's machine, and advisories were checked nowhere. New `task
  rust:audit` (advisories, bans, sources) plus a `Supply Chain` job running it
  beside the licence check. `unmaintained = "workspace"` was measured, not
  assumed: at `all` the tree fails on RUSTSEC-2024-0370 / RUSTSEC-2026-0173
  (proc-macro-error and fork) four levels down via getset → oci-spec →
  oci-client, where RUSTSEC records no safe upgrade. Both unmaintained
  notices; **zero vulnerability advisories in the tree today.**
- **W7 prerequisite now measured, not assumed** (2026-07-26): `gh api
  repos/grimoire-rs/setup-grimoire/tags` returns `v1 → 4de3a95`, which is
  `v1.1.0`, while `v1.2.0 → a1c530c` exists. So the floating major tag is one
  release behind and misses `a1c530c` ("remove `${{ secrets }}` from the action
  manifest description") — exactly why arcana pins `@v1.2.0`. Meanwhile
  `README.md:39` tells every reader to use `@v1`. **The retag is a one-command
  fix (`v1 → a1c530c`) but is deliberately NOT taken autonomously:** moving a
  floating major tag changes the code that runs in every consumer's CI, which
  is outside the "push to main in the sibling repos" grant. Needs an explicit
  yes.
- **Correction to an earlier entry:** the three `.agents/worktrees/` dirs are
  **not** orphaned — `git worktree list` through the rtk wrapper truncated its
  output. All three are live registered worktrees with uncommitted changes
  (`fix/vendor-env-resolution`, `feat/per-vendor-config`,
  `fix/client-detection`), and `grimoire-duo` is on `feat/vendor-coverage-wave2`
  at `d55351f` (parent `6fc2bea`, i.e. current), not on `duo`. Do not remove
  any of them. Verify git state through `rtk proxy git ...`, never the wrapper.
  **Next: W3-A** (deep, opus — no `SECURITY.md` exists, and `README.md:69`
  links to it today, so the gap is already a 404 on the public README),
  **then W4** (critical path to W9-b).
- **Last update:** 2026-07-26 (after 6fc2bea: docs: name the compatibility matrix on the landing page)

## Evidence base

**All findings, verified defects (D1–D10), refuted claims, competitive
landscape, copy drafts, channel mechanics, and licensing analysis live in
[`research_promotion_positioning.md`](../research/research_promotion_positioning.md).**
This file is orchestration only — it does not restate evidence. Sub-plans
cite `D<n>` identifiers from that artifact.

Re-verify anything in the research artifact older than its 2027-01-26 expiry
before citing it publicly.

## Objective

Ship 1.0 with a public surface that survives a Show HN. Not: grow the CLI, not
: chase enterprise adoption, not: rename anything.

## Scope boundaries (decided, do not relitigate)

- **Keep the name `grim` / `grimoire`.** Rename breaks six frozen 1.0
  contracts. Consequence: crates.io, Homebrew, AUR, nixpkgs, winget are
  permanently off the channel list.
- **Copyright holder is `The Grimoire Authors`,** not a personal name.
- **Keep mdBook.** No VitePress/Astro/Docusaurus migration.
- **No employer, company, or adoption story appears anywhere public.** No
  case study, no anonymized reference, no headcount.
- **Copyright ownership is settled — the former W2-D is closed (2026-07-26).**
  Grimoire was written entirely outside employment: not commissioned, not on
  employer instruction, not within job scope, so §69b UrhG never triggers.
  All 540 human-authored commits carry `contact@michael-herwig.de`; no company
  address appears as author or committer. A third party deploying grim
  received it under Apache-2.0 like any other user — use is not ownership.
  Residual (non-blocking, self-serve): read the employment contract once for a
  Nebentätigkeit / IP-assignment clause, which is contract law and can reach
  further than the statute.
- **No CLA, no NOTICE file.**
- **Drift/CI is struck from positioning** until `grim status --exit-code`
  exists (W1-C).

## Workstreams

Research depth per workstream: **deep** = spawn research agents and produce
findings *before* the sub-plan is written; **light** = verify named facts
only; **none** = act on the research artifact as-is.

| ID | Workstream | Research | Blocks 1.0 | Repos touched |
|---|---|---|---|---|
| **W1-A** | Fix README quick start (D1) + delete stale status block (D10) | none | **yes** | grimoire |
| **W1-B** | Short-ref → kind-segment resolution (D2) | **deep** | **yes** | grimoire |
| **W1-C** | `grim status --exit-code` (additive) | light | no | grimoire |
| **W2-A** | LICENSE holder string (D3) ×3 repos | none | **yes** | grimoire, -vscode, -components |
| **W2-B** | `setup-grimoire/LICENSE` full text + `index` LICENSE (D6) | light | **yes** | setup-grimoire, index |
| **W2-C** | `CONTRIBUTING.md` inbound-license + DCO (D7) | light | **yes** | grimoire |
| **W3-A** | `SECURITY.md` + threat/trust model (D4) | **deep** | **yes** | grimoire |
| **W3-B** | Enable private vuln reporting, secret scanning, Dependabot security; ~~fix dead `/discussions` link~~ **done 2026-07-26 — Discussions enabled by the human (`hasDiscussionsEnabled: true`), which retires the D5 dead link in `.github/ISSUE_TEMPLATE/config.yml:4`; no commit, it was a repo setting** (D4, D5) | none | **yes** | grimoire |
| **W3-C** | CVE scanning in CI + wire `license:check` into a gate (D8) | light | no | grimoire |
| **W4** | README rewrite + headline propagated to 5 surfaces (D10) | none | **yes** | grimoire |
| **W5** | Landing page `index.hbs` + `head.hbs` OG/canonical + sitemap/robots | **deep** | no | grimoire |
| **W6** | Demo cast (asciinema → player + GIF) | light | no | grimoire (+ ocx read) |
| **W7** | Publish-from-CI starter from arcana + `setup-grimoire@v1` retag | **deep** | no | grimoire, setup-grimoire |
| **W8** | Topics, About, social preview; VS Code ext category/keywords/homepage typo/Open VSX (D5, D9) | light | no | grimoire, -vscode |

> **W8 asset note (checked 2026-07-26):** `grimoire/assets/{logo.png,logo.svg}`
> and `grimoire-vscode/assets/{logo.png,logo.svg}` are **byte-identical** — the
> vscode repo holds no newer logo. Its only extra file is `vsc-icon.svg` (a
> marketplace-icon variant, present since its initial commit `9ad5b19`), which
> is a usable source for the missing social-preview / OG card. There is still
> **no** social preview image and no demo asset in either repo.
| **W9** | Launch sequence execution | none | — | none |
| **W10** | Ecosystem narrative: Discussion #292, MCP registry, awesome-claude-code | light | no | none |

## Dependency graph

```
W1-A ──┬─────────────────────────────────────────────► W4 ──┐
       │                                                     │
W1-B ──┘  (quick start must be truthful before README ships) │
                                                             │
W2-A ─── W2-B ─── W2-C                                       │
                                                             ├──► W9-a Console.dev
                                                             │
W3-A ─── W3-B                                                │
                                                             │
W8  (independent, 30 min, run any time) ─────────────────────┤
                                                             │
W5 ──── W6 ────────────────────────────────────────────────► W9-b tag 1.0
                                                             │        │
W7 (independent; gated on setup-grimoire v1 retag) ──────────┘        ▼
                                                              W9-c Show HN + same-day
W1-C, W3-C  (post-launch, unblock the CI claim)                       │
                                                                      ▼
                                                                     W10
```

**Critical path:** W1-A → W1-B → W4 → W9-b.
**Hard deadline:** W9-a (Console.dev) accepts **pre-1.0 tools only** — it must
be sent before the 1.0 tag exists, and it needs W4 + W5 + W6 to be worth
sending.

## Sub-plan spawn contract

Each workstream becomes `.agents/plans/plan_<slug>.md` from
`.claude/templates/artifacts/plan.template.md`, carrying the standard Status
block. On spawn, this meta-plan's Status `Step` records which sub-plan is
active; `.claude/state/current_plan.md` repoints to it; the sub-plan sets
`Parent plan: meta-plan_promotion_1_0`.

For a **deep** workstream the sequence is:

1. **Research first.** Spawn parallel researchers, persist findings to
   `.agents/research/research_<topic>.md`. Do not write the sub-plan from
   memory or from this meta-plan alone.
2. **Verify the load-bearing claim by hand** before it enters the plan. Every
   deep workstream below names its claim explicitly.
3. **Then** write the sub-plan, with backwards-compat, TDD, JSON-interface,
   and exit-code sections (mandatory for this repo).
4. Implement → `task verify` → commit.

### Deep-workstream research gates

| WS | Claim that must be independently verified before planning | Also research |
|---|---|---|
| **W1-B** | ~~Does fixing short-ref expansion change resolution for any already-published reference?~~ **ANSWERED 2026-07-26 → `research_short_ref_resolution.md`. Yes: arcana publishes flat (`…/arcana/hex-core` skill, `…/arcana/hex` bundle, same level) and both resolve today at exit 0. Inserting a kind segment 404s them — breaking, prohibited. Short refs are not broken; they are layout-dependent, and only the first-party catalog is kind-segmented.** Remaining: options B (flat aliases) + D (docs, **done** `c80b38a`), with C (index fallback) as the general fix needing an ADR | Load-bearing open question before any alias is published: does a flat alias plus a bundle-provided member read as one artifact or two under `adr_effective_set_mutations.md`? |
| **W3-A** | What a supply-chain tool's SECURITY.md must contain in 2026 — compare 3–5 peers (ORAS, cosign/sigstore, uv, cargo-dist). | Reuse `quality-security.md`'s already-enumerated surfaces: registry auth, archive extraction / zip-slip, symlink escape from `GRIM_HOME`, `${installPath}` env injection, MCP config writes. State the no-signing limitation honestly — the supply-chain narrative depends on that honesty |
| **W5** | ~~`is_index` must be confirmed~~ **CONFIRMED 2026-07-26 against mdBook 0.5.3 by build test: patched `theme/index.hbs` with `{{#if is_index}}`, root `index.html` → YES, `introduction.html` → NO.** A full-bleed landing page at the site root is possible with no migration and no change to any docs page. Design brief written: `design_brief_landing_page.md` | Still open: whether `sitemap.xml`/`robots.txt` survive `mdbook build`; how grimoire.rs is actually deployed (`.github/workflows/docs.yml` + CNAME/Pages) — needed for the OG/canonical `head.hbs` pass, not for the design |
| **W7** | Is `grimoire-rs/setup-grimoire@v1` still broken? Arcana pins `@v1.2.0` with the comment *"floating @v1 tag is stale/broken"*. **Retag is a prerequisite** — shipping a CI example that tells strangers to use `@v1` while `@v1` is broken is worse than shipping nothing | Diff arcana's workflow against `docs/src/ci.md` for gaps: `--dry-run` on `workflow_dispatch` appears **zero** times in ci.md; classic-vs-fine-grained PAT fork behaviour; exit-69 soft-warn; `DOCKER_CONFIG` isolation. Decide: docs-only, or a reusable workflow in setup-grimoire |

## Model routing

Per `~/.claude/CLAUDE.md`. Every spawn sets `model` explicitly and carries a
`Model rationale:` line.

| Work | Model |
|---|---|
| W1-B design + implementation (resolution semantics, compat) | opus |
| W3-A (security-adjacent judgment) | opus |
| Any review, adversarial, or verification pass | opus |
| W5 mdBook research, W7 CI research, W6 asciinema, W8, channel mechanics, docs drafting | sonnet |
| W1-A, W2-A/B, mechanical multi-repo edits | sonnet |

## Git

- Meta-plan and sub-plans live under `.agents/plans/` — **committed**,
  team-shared. The research artifact under `.agents/research/` **is**
  committed.
- Current branch: `docs/promotion-groundwork` (holds the research artifact).
- Each workstream takes its own branch, or a sibling worktree
  `../grimoire-wt-<topic>` on `<type>/<topic>` for anything multi-commit. Use
  `git -C <worktree>` for every git call — the pre-commit verify hook resolves
  the main checkout otherwise.
- Conventional commits. `chore:` for AI-config and tooling; `docs:` for
  README/site; `fix:` for W1-B; `feat:` for W1-C.
- **No push.** No release tag without explicit approval — W9-b is a human
  decision, not an agent action.

## Not doing

- No rename, no crates.io/Homebrew/AUR/nixpkgs/winget.
- No NOTICE file, no CLA.
- No VitePress migration.
- No public mention of any employer or adoption.
- No Reddit on launch day (all subreddit rules were unverifiable during
  research — opportunistic participation only, sidebars re-checked by hand).
- No competing on registry scale, search quality, or security scoring.
- No `grim status` CI claim until W1-C lands.

## Suggested first move

**W8 + W1-A + W2-A** — roughly 30 minutes total, zero research debt, and W8 is
free discovery that compounds while everything else is in flight. Then
**W1-B** (deep) as the first real sub-plan, because W4 cannot honestly ship
until the quick start is true.
