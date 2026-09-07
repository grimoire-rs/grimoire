# Research: docs site migration from mdBook to Astro Starlight

**Date:** 2026-09-06
**Question:** what the current grimoire.rs site depends on, how Starlight covers each dependency, and which URLs are shipped contracts.
**Method:** read-only inventory of `docs/`, `.github/workflows/docs.yml`, `src/`, `catalog/`, and the indexer renderer (`grimoire-rs/indexer`, Astro), by a research subagent; Starlight capabilities from its published docs.
**Consumers:** `adr_docs_site_starlight.md`, the docs redesign plan.

> Corrections after review (2026-09-06): the "no page carries a doc_type
> declaration" statement below described `main`; on branch
> `docs/use-case-discovery` every page already carries one and the check
> passes. The schema files are `grimoire-config.schema.json`,
> `grim-publish.schema.json`, `grimoire-lock.schema.json` (plus a
> `grim-mcp.schema.json` `$id` that no generator emits). 289 headings carry a
> `{#custom-id}` suffix feeding 59 anchored deep links from `src/` and
> `catalog/`; see the ADR.

# Starlight Migration Research: Index Site Design + Docs Site Inventory

## A. Index site design inventory

**Stack**: Astro `^7.1.4` + `@astrojs/preact` `^6.0.1` + Preact `^10.29.7` (`.agents/worktrees/grimoire-index/package.json:23-35`). **Not Starlight** — a hand-built layout (`Base.astro`). No `astro.config.mjs` exists; the whole Astro config is generated programmatically per build/dev run via `inlineConfig()` (`src/renderer/index.ts:308-341`), with `configFile: false` so a stray file in a consumer's index repo can never hijack routing.

**Design tokens** — one `@layer grimoire` block in `Base.astro:380-1379`, all overridable by an unlayered user CSS file (the entire customization contract, per the comment at `Base.astro:362-379`):

| Token group | Light | Dark |
|---|---|---|
| `--bg` / `--fg` / `--card` / `--border` / `--muted` | `#f7f6f3` / `#1d1c1a` / `#ffffff` / `#dedbd3` / `#6d6a63` | `#16151a` / `#e8e6e1` / `#201f26` / `#35333d` / `#98948b` |
| `--accent` / `--accent-soft` / `--on-accent` | `#6d4fc4` / `#efeafd` / `#ffffff` | `#a58cf0` / `#2c2540` / `#ffffff` |
| `--chip-bg` | `#edebe5` | `#2a2931` |
| `--kind-{skill,rule,agent,mcp,bundle}` | `#6d4fc4` / `#1a7f6b` / `#b0641a` / `#2565c7` / `#a63d68` | `#a58cf0` / `#4cc2a9` / `#e0a35c` / `#6fa8f5` / `#e07ba6` |
| `--deprecated` / `--banner-{bg,fg,border}` | `#9c5400` / `#fff3cd` / `#6b4c00` / `#e0b84c` | `#e3a640` / `#4a3b0a` / `#f0d78c` / `#8a6d1a` |

Font: `system-ui, -apple-system, "Segoe UI", sans-serif` (`Base.astro:436-441`), monospace accents via `ui-monospace, "SFMono-Regular", monospace`. Radii: `8px` (cmd-bar, cards), `10px` (cards, toast, detail panels), `999px` (pills/chips). `color-scheme` rides the same block so form controls/scrollbars follow (`Base.astro:401,423`).

**Layout components**: `Base.astro` (shell: head, header w/ brand+nav+theme toggle, footer, global styles, copy-toast, code-copy injection); `pages/index.astro` (hero + install/registry command bars + OS-detect picker + `Catalog.tsx` island); `pages/p/[...slug].astro` (per-package detail page — frozen route `/p/<namespace>/<name>/`, `[...slug].astro:3`); `Catalog.tsx` (735-line Preact island — search/filter/sort grid); `CommandField.astro` (43-line copy-command control); `PickerMenu.astro` (68-line `<details>` menu, shared by platform/scope pickers); `VersionMenu.astro` (109-line version-tag popover); `BrandMark.tsx` (12-line `@mdi/js` glyph wrapper for VS Code/Apple/Windows/Linux marks, since Lucide carries no brand marks).

**Branding injection** from `index.config.json` → `SiteConfig` (`src/config.ts:39-105`, defaults `src/config.ts:110-150`): `site`, `brand`, `brandMark`, `description`, `tagline`, `docsUrl`, `installDocsUrl` (defaults to `https://grimoire.rs/installation.html`), `repoUrl`, `favicon`, `logo`, `install[]` (per-OS one-liners), `vscodeExtension`, `registry` (alias+index), `footerNote`, `footerLinks[]`, `attribution` (bool), `customCss` (path, inlined unlayered — `Base.astro:1382-1383`).

**Where a docs site could import tokens**: the `@layer grimoire` block, `Base.astro:380-1379`, is the single canonical token source — copy the custom-property values (not the layer mechanism) into Starlight's `--sl-color-*` overrides. `demo.cast`/`asciinema-player.min.js`/`.css` are vendored (3.17.0, Apache-2.0) only on the docs landing page (`docs/theme/index.hbs:37-44,68,525,528`) — no equivalent asset lives in the index renderer.

## B. Current docs site inventory

**Generator**: mdBook `0.5.3` pinned (`.github/workflows/docs.yml` comment + `taiki-e/install-action … tool: mdbook@0.5.3`), theme `navy`/`navy` dark (`docs/book.toml:8-9`), fold level 1 (`docs/book.toml:12-13`), `edit-url-template` → `.../edit/main/docs/{path}` (`book.toml:11`), `git-repository-url` set (`book.toml:10`), one `additional-css` (`theme/clients-matrix.css`, `book.toml:13`).

**`theme/index.hbs`** (927 lines): the `is_index` branch replaces mdBook's landing page entirely with a self-contained, no-CDN dark hero (asciinema demo, inline SVG client marks from `@lobehub/icons-static-svg` + Simple Icons); the `{{else}}` branch is mdBook's **stock** `init --theme` template kept byte-for-byte so upgrades diff cleanly (comment at top of file). `theme/clients-matrix.css` styles `docs/src/clients.md`'s compatibility table (left-aligns it, sizes/aligns 24 inline client-mark SVGs, adds a lettered fallback tile for Droid).

**`seo.py`** (post-build, `taskfiles/docs.taskfile.yml:14-27`): stamps `data-grim-version` from `Cargo.toml`, injects `canonical`/`og:*`/`twitter:*` per page (root = `website`, chapters = `article` with a scraped first-paragraph description), appends a shared footer to stock chapter pages (skips landing/`start.html`/`privacy.html`, which carry their own), and writes `sitemap.xml`. Skips `404.html`, `print.html`, `toc.html`.

**Generated schemas** (`docs/src/schemas/*.json`): produced by `task schema:generate` (`taskfiles/schema.taskfile.yml:22-29`) from `grim schema --kind {config,publish,lock}`, never committed. Consumed at `https://grimoire.rs/schemas/grimoire-config.schema.json` by the `#:schema` directive `grim init`/`grim add` write into every `grimoire.toml` (`src/command/init.rs:205`, `src/command/add.rs:1664`), and by `SCHEMA_BASE_URL` in `src/command/schema.rs:27`.

**`demo.cast` + asciinema player**: only on the landing page (`theme/index.hbs:37-44,68,525,528`), vendored `asciinema-player.min.js`/`.css` (integrity-checked against npm, not CDN), recorded via `task test:demo` / `test/recordings/test_record_demo.py` against the real published `grim-usage` skill. No `.md` chapter embeds it.

**Installer scripts**: `docs/src/install.sh` / `install.ps1` — thin bootstraps that fetch the cargo-dist-generated GitHub-release installer (`install.sh:1-9`, `install.ps1:1-11`). Served at `https://grimoire.rs/install.sh`/`install.ps1` as alternate front doors to `https://setup.grimoire.rs/{sh,ps1}` (`docs/src/installation.md:53,59,62-63`); both URLs are also the `index.config.json` default `install[]` commands (`config.ts:129-135`).

**Other static files**: `404.html` (mdBook default, generated), `robots.txt` (`Sitemap: https://grimoire.rs/sitemap.xml`), `og-card.png` (LFS-tracked, referenced by CI comment `.github/workflows/docs.yml`), `favicon.png`/`favicon.svg`, `privacy.html` and `start.html` (both standalone, no chapter chrome, self-styled — `start.html`'s comment explicitly says its tokens are "a deliberate copy" of the landing page's, not shared).

**No `CNAME` file** anywhere in `docs/` — the custom domain is a GitHub Pages *repo setting*, deployed via `actions/configure-pages`/`upload-pages-artifact`/`deploy-pages` (`.github/workflows/docs.yml`), not a committed file. Unaffected by a generator swap.

**21 pages in `docs/src/SUMMARY.md`**: introduction, installation, quickstart, concepts, clients, commands, configuration, authentication, package-index, hosting-an-index, ratings, publishing, ci, self-hosted-gitlab, agents, mcp-servers, artifacts, vendor-metadata, json-interface, stability, upgrading — plus non-SUMMARY standalone pages `start.html`, `privacy.html`.

**`grimoire.rs/*.html` cross-references found via `grep -rn 'grimoire\.rs/'`** (`src/`, `catalog/`, `README.md`, `docs/`, excluding `docs/book/`): 19 distinct page URLs are hardcoded as absolute links from source and catalog content — `agents.html, artifacts.html, authentication.html, ci.html, clients.html, commands.html, concepts.html, configuration.html (+ #registry-compatibility, #browse-filters), hosting-an-index.html, installation.html, introduction.html, json-interface.html, mcp-servers.html, package-index.html, publishing.html (+ #batch-publish), quickstart.html, ratings.html, start.html, vendor-metadata.html`. Notable call sites: `src/catalog/registry_catalog.rs:61` (`REGISTRY_COMPAT_DOCS_URL`), `src/catalog/catalog_service.rs:506,1629` (browse-filter warning text), `src/command/init.rs:205` / `src/command/add.rs:1664` (schema directive), `catalog/skills/grim-authoring/SKILL.md:205-209`, `catalog/publish.toml:35`, `catalog/README.md:125`, `catalog/descriptions/grim-mcp.md:28`.

**Doc-page correction**: no `.md` page under `docs/src/` currently carries an `<!-- doc_type: ... -->` line-1 declaration — every page's first line is a plain `# Heading` (verified across all 20 SUMMARY pages). The `doc_type`/`doc_tier` comment convention exists only in the `.claude/rules/docs-quality/` rule set (a generic, not-yet-applied checklist — its own "Not studied" section states "No page in the calibration corpus declares a type"). This is a correction to the premise in the research brief.

## C. Starlight capability map

| B item | Starlight coverage |
|---|---|
| Sidebar / SUMMARY.md | `starlight({ sidebar: [{ label, items: [...] }] })` in `astro.config.mjs`; grouping into the four planned labels (Getting started / Guides / Teams and automation / Reference) is a direct list-of-groups config, autogenerate-by-directory optional. |
| `book.toml` theme/fold | Starlight ships its own light/dark theme + collapsible sidebar groups (`collapsed: true` per group) — no 1:1 `navy`/fold-level equivalent, replaced by `customCss` tokens (below). |
| `edit-url-template` | `starlight({ editLink: { baseUrl: 'https://github.com/grimoire-rs/grimoire/edit/main/docs/' } })` — same templated-base-URL shape. |
| `additional-css` / `clients-matrix.css` | `starlight({ customCss: ['./src/styles/clients-matrix.css', ...] })` — array of plain CSS files, imported globally. |
| `theme/index.hbs` landing branch | A custom Astro page at `src/pages/index.astro` **outside** the Starlight content collection (same pattern the current landing page already uses to bypass mdBook chapter chrome) — full control, no Starlight `Hero`/`splash` constraint needed since it's not a docs-collection page. |
| `seo.py` canonical/OG/Twitter | Astro `site` config gives canonical URLs natively; OG/Twitter tags need either `starlight({ head: [...] })` per-site defaults or a `head` frontmatter/remark plugin per page for the chapter-specific description `seo.py` currently scrapes — no automatic first-paragraph extraction exists in Starlight, this is the one piece with no drop-in equivalent. |
| Sitemap | `@astrojs/sitemap` integration (Starlight bundles/recommends it) — config-only, replaces the hand-written `write_sitemap()`. |
| `docs/src/schemas/*.json` | Move to `docs/public/schemas/*.json` — Astro's `public/` dir is verbatim static passthrough, same semantics as mdBook copying non-`.md` files in `src/`. URL `/schemas/...` unchanged if `public/schemas/` mirrors the current path. |
| `demo.cast` + asciinema player | Since the embed lives only on the non-collection landing page (`index.astro`), it ports as a plain `<script>`/`<link>` pair identical to today — no `.md`/`.mdx` component-import question at all. |
| `install.sh` / `install.ps1`, `robots.txt`, `og-card.png`, favicons | All move to `docs/public/` verbatim (Astro static passthrough), same served paths. |
| `404.html` | Starlight/Astro convention: `src/content/docs/404.md` (or `src/pages/404.astro`) → build emits `404.html` at the output root, same GH Pages custom-domain behavior as today. |
| `CNAME`/custom domain | Not a repo file — no change; `actions/configure-pages` + `upload-pages-artifact` + `deploy-pages` steps are generator-agnostic, only the build command inside the job changes. |
| Old `*.html` URLs → clean routes | Astro `redirects` config in `astro.config.mjs` (e.g. `{'/introduction.html': '/introduction/'}`) — for a fully static (non-adapter) build these render as static HTML stub pages with `<meta http-equiv="refresh">`, not true server 301s. |
| Pagefind search | Starlight ships Pagefind built in, zero config — replaces mdBook's built-in `elasticlunr` index (`searchindex-*.js`). |
| Dark/light theme | Starlight has a built-in toggle backed by its own `--sl-color-*` custom properties — philosophically identical to the index site's `data-theme` + `localStorage` mechanism (`Base.astro:60-65,215-220`), values need porting, mechanism doesn't. |
| i18n | Not needed — explicitly out of scope, Starlight's i18n features simply go unused. |

**`.md` vs `.mdx`, frontmatter**: Starlight's content-collection schema (`docsSchema()`) makes **`title` the only mandatory frontmatter field** on every page in `src/content/docs/`; `description` is optional (falls back to site default) but drives the meta-description tag Starlight otherwise leaves empty. Plain `.md` pages render through Starlight's standard remark/rehype pipeline with no MDX/component-import capability, and are fully supported — every existing `docs/src/*.md` file works once a `title:` frontmatter block is added (the pages currently open with `# Heading` as line 1, not frontmatter — see the B-item correction above, so this is new work, not a preserved contract). **`.mdx` is the one that would break on the docs-quality rule set's planned `<!-- doc_type: ... -->` line-1 HTML comment**: MDX parses `<` as JSX-tag syntax, not HTML-comment syntax, so a literal `<!-- -->` at the top of an `.mdx` file is a parse error — MDX comments must be written `{/* ... */}`. Since none of grimoire's current pages carry that comment yet (per the B correction), this is a forward-looking constraint on the docs-quality rollout, not an active blocker on today's content — but it does mean **`.md`, not `.mdx`, is the correct target format** if that convention is ever adopted.

## D. Risks and open questions

- **URL contract break.** All 21 SUMMARY pages plus `start.html`/`privacy.html` currently resolve at `/*.html`; Starlight's default routing is clean paths (`/introduction/`, no `.html`). Every one of the 19 hardcoded `https://grimoire.rs/*.html` references enumerated in B (in `src/`, `catalog/`) would 404 without either (a) an `Astro.config` `redirects` map for all 21+2 old paths, or (b) rewriting every source/catalog string literal — the latter conflicts with Principle 9 (frozen surfaces, additive-only) since some of those strings (e.g. `REGISTRY_COMPAT_DOCS_URL`, the browse-filter remedy text) are user-facing CLI output, not just docs cross-links.
- **The schema URL must not move.** `SCHEMA_BASE_URL = "https://grimoire.rs/schemas"` (`src/command/schema.rs:27`) is baked into every `grimoire.toml` grim writes via the `#:schema` directive (`init.rs:205`, `add.rs:1664`) and is asserted in tests (`schema.rs:165,176,193,293`). This is an external, already-shipped contract independent of the docs site's internal page routing — the migration must keep `docs/public/schemas/*.json` mapping to exactly `/schemas/*.json`, with no base-path or trailing-slash change.
- **Installer URLs.** `install.sh`/`install.ps1` at `grimoire.rs/install.{sh,ps1}` (`docs/src/installation.md:62-63`) are documented "front doors" independent of `setup.grimoire.rs` — must land at the literal same paths post-migration (static passthrough via `public/`, not a content-collection route, so this is low-risk if placed correctly).
- **`seo.py`'s per-chapter OG description** (first real paragraph >80 chars, `seo.py:` `chapter_summary()`) has no Starlight equivalent; either accept a single site-wide description or write a small remark/rehype plugin — flagged as a capability gap in C, not a blocker.
- **`doc_examples.py` / docs-quality harness expectations** are not currently wired to `docs/` at all — the rule set lives only in `.claude/rules/docs-quality/` (and a `docs-plan` worktree scaffold) with its own fixtures, and no `docs/src/*.md` page currently satisfies its `doc_type`/`doc_tier`/fence-marker requirements. A Starlight migration and a docs-quality rollout are two independent, unstarted efforts that happen to touch the same files — sequencing matters (adding declarations before or after the format change) but neither currently blocks the other.
- **Node toolchain in CI.** The `grimoire` repo's own CI (`.github/workflows/docs.yml`) is Rust-only today — no `actions/setup-node` step anywhere in it. The precedent cited (Node `^22`/`^24` via `actions/setup-node@…` + `npm ci`) exists only in the **sibling** `grimoire-rs/indexer` repo's `ci.yml`/`release.yml`, not in `grimoire` itself. Adding Starlight is therefore the **first** Node toolchain introduction into grimoire's own CI, not a repeat of an existing in-repo pattern.
- **GitHub Pages + custom domain from an Astro build.** No `CNAME` file exists in `docs/`; the domain is a repo setting consumed by `actions/configure-pages`. The deploy job (`configure-pages` → build → `upload-pages-artifact` → `deploy-pages`) is generator-agnostic — only the `task docs:build`-equivalent build step changes, which lowers this risk to "swap one `run:` line," not a redesign of the deploy job.
- **Version pinning**: mdBook is currently pinned to an exact patch (`mdbook@0.5.3`) specifically because `theme/index.hbs`'s `{{else}}` branch is a byte-for-byte vendor copy that must be manually re-diffed on any upstream bump (comment at `theme/index.hbs` top, workflow comment). Astro/Starlight have no equivalent "diff against stock theme" constraint since the entire landing page is already hand-authored — but exact version pins (`astro`, `@astrojs/starlight`, `@astrojs/sitemap`) should still be recorded the same deliberate way, per `product-tech-strategy.md`'s "Latest Stable... unless pinned" rule.
