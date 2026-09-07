# Research: URL contract, catalog drift and VS Code facts

**Date:** 2026-09-06
**Run:** hex-plan xhigh, docs redesign (`.agents/plans/plan_docs_site_redesign.md`)
**Phase:** Discover, explorer drift
**Consumers:** `.agents/specs/design_docs_site_redesign.md`, the plan.

# Discover: URL contract, catalog drift, VS Code facts, CI constraints

## 0. CRITICAL CORRECTION: docs/src is mdBook, not Astro

`.github/workflows/docs.yml` builds `docs/` (the `grimoire.rs` site) via
**mdBook 0.5.3** (`task docs:build` → `mdbook build docs` then
`docs/seo.py` for canonical/OG tags + sitemap). There is **no Astro
anywhere in this repo**. Astro lives only in the sibling `grimoire-index`
worktree (`.agents/worktrees/grimoire-index/src/renderer/astro/...`),
which builds the **separate** `index.grimoire.rs` site from a different
repo/pipeline entirely. A CI check "after an Astro build" as framed can
only apply to that other repo's pipeline, not to `docs.yml` in this repo.
If the plan is to add a link/anchor-check step to *this* repo's
`docs.yml`, it must run **after `mdbook build`**, checking `docs/book/`,
not a `dist/` directory — mdBook has no `dist/` concept.

## 1. URL inventory (grimoire.rs / index.grimoire.rs / setup.grimoire.rs)

`setup.grimoire.rs` — **zero occurrences** anywhere in `src/`, `catalog/`,
`README.md`, `docs/src`, `test/`. Only mentioned historically in
`catalog/CHANGELOG.md:490-491` ("Setup.grimoire.rs installers... Lead
README install with setup.grimoire.rs installers") — a changelog entry,
not a live reference. Nothing to assert for this host today.

### A. Schema `$id` (self-referential, generated JSON, served as static files)
Built at `task docs:build` time into `docs/book/schemas/*.json` (never
committed — regenerated from parser structs, see `docs.yml` comment).

| URL | Source | file:line |
|---|---|---|
| `https://grimoire.rs/schemas/grimoire-config.schema.json` | `SCHEMA_BASE_URL` + `CONFIG_SCHEMA_FILE` | `src/command/schema.rs:27,165` (also `init.rs:205`, `add.rs:1664` test asserts of the `#:schema` header) |
| `https://grimoire.rs/schemas/grim-mcp.schema.json` | same const | `src/command/schema.rs:176` |
| `https://grimoire.rs/schemas/grim-publish.schema.json` | same const | `src/command/schema.rs:193` |
| `https://grimoire.rs/schemas/grimoire-lock.schema.json` | same const | `src/command/schema.rs:293` |

CI check: assert `docs/book/schemas/<file>.json` exists post-build AND its
`$id` field equals the URL above (byte-for-byte contract, per
`adr_render_layout_stability.md`/Principle 9 additive-only framing).

### B. CLI output text — doc links with fragments (must resolve to a real anchor)

| URL | Kind | file:line |
|---|---|---|
| `https://grimoire.rs/configuration.html#browse-filters` | error remedy text | `src/catalog/catalog_service.rs:506,1629,1719` (1719 is a test asserting the exact string) |
| `https://grimoire.rs/configuration.html#registry-compatibility` | `REGISTRY_COMPAT_DOCS_URL` const, CLI output | `src/catalog/registry_catalog.rs:61` |

CI check: `docs/src/configuration.md` must contain `{#browse-filters}` and
`{#registry-compatibility}` heading anchors (confirmed present — grep
below).

### C. Functional default (not a doc link — a live registry endpoint)

| URL | Kind | file:line |
|---|---|---|
| `https://index.grimoire.rs` | `FALLBACK_INDEX` const — the built-in browse fallback registry | `src/command.rs:255` |

Not a "page must exist" check — this is a network address the binary
calls at runtime, out of scope for a static-build link checker.

### D. Test/fixture data (not real contract — sample values only)
`src/config/registry_resolve.rs` (~20 lines), `src/command/config.rs`
(4 lines), `src/command/init.rs:308-315`, `src/config/project_config.rs:1214,1221`,
`src/tui/markdown.rs:440-441` (generic markdown-link render test, string is
arbitrary), `src/catalog/index_announce.rs:289` (`user.email=announce@grimoire.rs`
— a git commit author for announce commits, not a URL) — all use
`https://index.grimoire.rs` or `grimoire.rs` as **realistic sample data**,
not references the CI needs to assert resolve. Excluded from the contract
list.

### E. Doc comment (non-user-facing)
`src/catalog/index_source.rs:12` — `//!` doc comment mentioning
`https://index.grimoire.rs` as an example transport. Not shipped output.

### F. docs/src/*.md — page + cross-link inventory
Total `{#custom-id}` headings across `docs/src/*.md`: **289** (confirmed:
per-file counts sum to 289 — `publishing.md` 44, `commands.md` 40,
`vendor-metadata.md` 22, `json-interface.md` 21, `artifacts.md` 20,
`clients.md` 18, `package-index.md` 14, `mcp-servers.md` 13,
`configuration.md` 13, `stability.md` 12, `hosting-an-index.md` 12,
`upgrading.md` 11, `ratings.md` 11, `agents.md` 8, `concepts.md` 7,
`authentication.md` 7, `self-hosted-gitlab.md` 6, `ci.md` 5,
`installation.md` 3, `quickstart.md` 2.

External-site links inside docs/src pointing at the *other* domain
(`index.grimoire.rs`): `commands.md:147,1533`, `package-index.md:13,32,61,465,484`,
`configuration.md:238,244,462,956`, `hosting-an-index.md:25,476`,
`privacy.html:12,135,147` (raw HTML page, **not in SUMMARY.md** — mdBook
copies it verbatim from `src/`; a link checker scoped to SUMMARY.md pages
will silently skip it), `start.html:275,291` (same — also not in
SUMMARY.md).

`docs/seo.py:40-43,77-78` independently hardcodes `privacy.html` and
`start.html` in its `SKIP`/footer-injection logic — a third place (besides
SUMMARY.md-absence and raw-HTML-copy) that "knows" these two pages are
special. A page rename/regroup that misses `seo.py` breaks canonical-tag
injection silently (no build failure).

### G. catalog/publish.toml
`catalog/publish.toml:34`: `url = "https://grimoire.rs"` — the top-level
manifest homepage field, ships in every published package's OCI annotations.

## 2. Catalog drift duty (catalog/README.md "Keeping content honest")

Procedure (verbatim policy): when `docs/src/{artifacts,clients,publishing,
vendor-metadata,commands,package-index}.md` or `src/command/**` or
`src/mcp/**` change, review `catalog/skills/grim-usage`,
`catalog/skills/grim-authoring`, and (for `clients.md`/`vendor-metadata.md`)
`catalog/skills/ai-config-authoring` for drift.

Files embedding docs URLs/fragments/nav (all Tier-3 "link only" per the
content-drift table) — a docs page rename or heading-id change makes these
stale silently (no build-time check catches a dead `[label]: url` fragment
today):

- `catalog/skills/grim-authoring/references/agent-spec.md` (6 fragment refs into `agents.html`, `artifacts.html`, `clients.html`, `vendor-metadata.html`, `publishing.html`)
- `catalog/skills/grim-authoring/references/bootstrap-existing-repo.md` (`ci.html`)
- `catalog/skills/grim-authoring/references/bundle-spec.md` (`artifacts.html#bundles`, `publishing.html#bundles`/`#pin`)
- `catalog/skills/grim-authoring/references/mcp-spec.md` (`mcp-servers.html`, `clients.html#matrix`)
- `catalog/skills/grim-authoring/references/release-checklist.md` (8 `publishing.html`/`vendor-metadata.html`/`package-index.html` fragments — also embeds nav-adjacent prose in `catalog/README.md` itself, "Repository Layout" cross-link)
- `catalog/skills/grim-authoring/references/rule-spec.md` (9 fragments)
- `catalog/skills/grim-authoring/references/skill-spec.md` (11 fragments)
- `catalog/skills/grim-authoring/references/updating.md` (7 page-level links — this file is explicitly the "re-research protocol," so it's the intended fix point, not itself drift)
- `catalog/skills/grim-authoring/references/vendor-metadata.md` (13 fragments, all `vendor-metadata.html#*`)
- `catalog/skills/grim-authoring/SKILL.md` (5 page links)
- `catalog/skills/grim-usage/SKILL.md:128`, `references/registries.md` (5 refs), `references/consume.md:26` — all `index.grimoire.rs`/registry docs
- `catalog/descriptions/*.md` (grim-essentials, grim-authoring, grim-mcp, grim-usage, ai-config-authoring) — each has a bare `Documentation: <https://grimoire.rs>` line, not fragment-scoped, low drift risk.

None of these are read by any automated check — `task catalog:verify` runs
`grim build` schema validation only, not link resolution. A docs regroup
that moves e.g. `vendor-metadata.md` fragments needs a **manual** grep
across `catalog/skills/grim-authoring/references/*.md` today.

## 3. VS Code extension facts (`grimoire-vscode`, read from sibling repo)

- **Marketplace id**: publisher `grimoire-rs`, name `grimoire-vscode` →
  `grimoire-rs.grimoire-vscode`. `displayName`: "Grimoire Marketplace".
  Current version `0.3.5` (package.json); repo root also has stray built
  `.vsix` files at `0.1.0`/`0.3.3` (stale local artifacts, not the source
  of truth — package.json is authoritative).
- **Activation**: `onStartupFinished`, `onWebviewPanel:grimoire.details`, `onUri`.
- **Commands** (Command Palette, category "Grimoire"): Search,
  Refresh Catalog, Check for Updates, Update All, Initialize Project,
  Install grim, Open Settings, Show Output, Show Info, Store Rating
  Token…, Clear Rating Token…, Report Bug, Request Feature, plus
  view-only ones (Compact Rows / Comfortable Cards / Tree View / Flat
  List / Group / Ungroup / Expand All / Collapse All). `openDetails` is
  deliberately palette-hidden — reachable only via the `vscode://` deep link.
- **Settings**: `grimoire.path.executable`, `grimoire.defaultScope`,
  `grimoire.watchForChanges`, `grimoire.prefetchDetails`,
  `grimoire.checkForUpdates`, `grimoire.checkArtifactUpdates`,
  `grimoire.extraEnv`.
- **Deep links** (both go through `catalog.ts::addRegistryUrl` in the
  index-site Astro renderer, `.agents/worktrees/grimoire-index/src/renderer/astro/lib/catalog.ts:89-104`):
  - `vscode://grimoire-rs.grimoire-vscode/vote?repo=<url-encoded repo>` —
    needs grim ≥0.14.0; opens the artifact, asks to confirm an upvote (a
    public forge post under the user's own account).
  - `vscode://grimoire-rs.grimoire-vscode/add-registry?index=<https-url>&alias=<name>[&include=...][&exclude=...][&scope=project|global]` —
    `addRegistryUrl` only emits this when: `registry.alias` passes
    `REGISTRY_ALIAS` regex, `index` parses as a URL with `https:` protocol,
    no embedded userinfo, and href ≤2048 chars. The extension itself always
    shows a confirm modal naming the exact index URL/alias/patterns/target
    `grimoire.toml` before writing — the link authorizes nothing by itself.
- **First-time user flow** (from README "What it does"): install the
  extension → trust the workspace (installs require a trusted workspace,
  since they shell out to `grim`) → Browse tab searches all configured
  registries at once, filterable by kind chips → click an artifact to open
  README/CONTENTS/CHANGELOG tabs with a metadata rail → Install (choose
  Project/Global scope, or both) → Installed tab shows what's present with
  a Project/Global toggle and last-synced status line → Updates tab +
  daily background check surface pending updates via an activity-bar badge.
  If `grim` isn't on `PATH`, the extension offers a checksum-verified
  download from GitHub.

## 4. subsystem-ci.md / quality-security.md constraints for a Node build in docs.yml

Both rules share `paths: .github/workflows/**, .github/actions/**,
.github/dependabot.yml` (declared overlap group in `.claude/rules.md`), so
both fire together on any `docs.yml` edit.

- **SHA-pin every `uses:`** with a `# vX.Y.Z` comment (existing `docs.yml`
  already does this for every action — a Node setup step must match,
  e.g. `actions/setup-node@<sha> # vN`).
- **Minimal permissions**: current job block declares only
  `contents: read, pages: write, id-token: write` at workflow level — a
  Node/npm step needs no additional permission; don't broaden.
- **Concurrency**: `docs.yml` already sets `group: pages,
  cancel-in-progress: false` — any added job must stay inside this
  existing workflow's concurrency group, not spawn a second workflow with
  its own (Pages deployments must serialize).
- **Taskfile is the source of truth**: per subsystem-ci.md principle 1, a
  Node/Astro build step must be wrapped as a task
  (`taskfiles/docs.taskfile.yml` or a new one) and CI must call `task
  <name>`, never raw `npm run build` in the workflow YAML.
- **No secrets in `run:` steps** — if the Node build needs any token, pass
  via `env:`.
- **Dependabot gap**: `.github/dependabot.yml` currently declares only two
  ecosystems — `github-actions` (root `/` + `/.github/actions/build-rust`)
  and `cargo` (root `/`). **There is no `npm`/`ecosystem: npm` entry
  anywhere.** Adding any `package.json` to this repo (for an Astro/Node
  docs step) requires a new dependabot block with its own `directory` and
  `groups:` (matching the existing pattern — one group per ecosystem,
  weekly interval, `commit-message.prefix`), or new dependencies go
  unpatched indefinitely.
- **Cost/runner guidance**: Linux runner only: docs.yml already runs on
  `ubuntu-latest`; a Node step is cheap, no new runner OS needed.
- Anti-pattern to avoid per the CI rule's checklist: don't let a new
  Node/lint step block the Pages deploy — if it's a link/anchor checker,
  gate it as a separate job or a `continue-on-error` + explicit final-gate
  step, consistent with "never let lint block test results."
