# ADR: Move the docs site from mdBook to Astro Starlight

## Metadata

**Status:** Accepted
**Date:** 2026-09-06
**Deciders:** Repository owner
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:** [x] no deviation — the strategy names no docs-site
generator; its "latest stable unless pinned" rule is honoured by pinning below.
**Domain Tags:** frontend, devops

## Context

`grimoire.rs` is built by mdBook 0.5.3 and deployed to GitHub Pages. Its real
landing page is not Markdown: it is a 927-line `docs/theme/index.hbs` whose
`is_index` branch replaces mdBook's index entirely, while the `{{else}}` branch
vendors mdBook's stock template byte-for-byte — which is why the version is
pinned to an exact patch. Post-build SEO is a hand-written `docs/seo.py`.

Three pressures arrive together. The sibling index site (`index.grimoire.rs`) is
already Astro, so the two halves of the product look different. The discovery
pass wants four nav groups and twelve new pages, which a flat `SUMMARY.md` cannot
express. And the landing page's visual language is locked inside Handlebars,
where no docs page can reach it.

Principle 9 governs the move. The site is a released surface: 21 chapter pages
plus `start.html` and `privacy.html` resolve at `https://grimoire.rs/<page>.html`,
and those URLs are hardcoded in `src/` and `catalog/` — some of them inside
user-facing CLI output. `/schemas/*.json`, `/install.sh` and `/install.ps1` are
shipped contracts consumed by `grim` itself.

## Decision Drivers

- Principle 9: no breaking change to any already-published URL.
- One visual system across `grimoire.rs` and `index.grimoire.rs`.
- Grouped nav and room for the twelve planned use-case pages.
- Keep the `<!-- doc_type -->` declaration convention working.
- Minimum new machinery: this repo's CI has never run Node.

## Industry Context & Research

**Research artifact:** [`research_docs_starlight_migration.md`](../research/research_docs_starlight_migration.md)
**Key insight:** every mdBook capability in use maps 1:1 onto Starlight — Pagefind
search, sidebar groups, an `editLink` base URL, `customCss` — except `seo.py`'s
scraped per-page description, which becomes explicit frontmatter.

## Considered Options

### Option 1: `build.format: 'file'` — keep `<page>.html` byte-for-byte

**Description:** Astro emits `about.html`, not `about/index.html`. Old URLs stay
the live URLs; nothing redirects because nothing moved.

| Pros | Cons |
|------|------|
| Zero URL change, so Principle 9 holds by construction | `.html` URLs read as dated for a docs site |
| No redirect map to maintain as pages are added | `@astrojs/sitemap` omits the extension (astro#15526) |
| Starlight generates `.html` links natively (below) | Pagefind's URL derivation for non-index files is undocumented |

### Option 2: clean routes plus a redirect map

**Description:** Starlight uses `/introduction/`; `redirects` covers the 23 old
paths. On a static build these emit meta-refresh stubs, not 301s.

| Pros | Cons |
|------|------|
| Conventional modern docs URLs | 23 hand-maintained entries that rot silently |
| Search engines follow the canonical eventually | A meta-refresh stub drops the URL fragment, and 59 shipped deep links carry one |

### Option 3: rewrite the hardcoded strings

**Description:** Change all 19 page URLs in `src/` and `catalog/` to clean routes.

| Pros | Cons |
|------|------|
| No redirect layer at all | Several strings are CLI output text — a frozen surface under Principle 9 |
| | Every released binary keeps printing the old URL forever |

## Decision Outcome

**Chosen Option:** Option 1 — `build.format: 'file'` with `trailingSlash: 'never'`.

**Rationale:** it is the only option that leaves the contract untouched instead of
compensating for having broken it. Option 3 fails outright — the URLs in
`registry_catalog.rs` and `catalog_service.rs` are printed by binaries already in
the wild, and rewriting a string cannot un-print it. Option 2 trades a hard
guarantee for a stub that drops the URL fragment, and 289 headings across 20 pages
carry an explicit `{#custom-id}` feeding 59 hardcoded anchored deep links.

**Evidence.** Astro's configuration reference on `build.format`: `'file'` means
"Astro will generate an HTML file named for each page route. (e.g.
`src/pages/about.astro` and `src/pages/about/index.astro` both build the file
`/about.html`)", and the same page advises pairing "`file` with
`trailingSlash: 'never'`". Starlight implements the format as a first-class
strategy in `packages/starlight/src/utils/createPathFormatter.ts`:

```ts
const formatStrategies = {
	file: { addBase: fileWithBase, handleExtension: (href) => ensureHtmlExtension(href) },
	directory: defaultFormatStrategy, preserve: defaultFormatStrategy,
};
// Skip trailing slash handling for `build.format: 'file'`
if (format === 'file') return href;
```

Every Starlight-generated link — sidebar, previous/next, edit link — runs through
that formatter, so all emit `.html`. Canonical tags follow the same rule in
`utils/canonical.ts`: `if (opts.format === 'file') return href;`. `utils/path.ts`
documents `ensureHtmlExtension` as producing output "suitable for S3/CloudFront
deployments without folder index support" — the shape GitHub Pages serves today.
The one open Starlight bug here, `withastro/starlight#2383`, fires only when
`base` is a subpath; `grimoire.rs` serves from the root with no `base`.

### Settled sub-decisions

| Question | Decision | Rationale |
|---|---|---|
| Page format | `.md`, never `.mdx` | MDX parses `<` as JSX, so the `<!-- doc_type -->` declaration is a parse error there |
| Declaration placement | Directly below the frontmatter block | `doc_declaration.py` DOC-TYPE-29 rejects a declaration above frontmatter |
| Frontmatter | `title:` (mandatory) and `description:` on every page | Starlight's `docsSchema()` requires `title`; `description` replaces `seo.py`'s scraped paragraph, first draft from the discovery inventory |
| Sidebar | Four groups in `astro.config.mjs`: Getting started, Guides, Teams and automation, Reference | The discovery IA plan; replaces the flat 20-entry `SUMMARY.md` (quality-audit finding 4) |
| Landing page | Custom `src/pages/index.astro`, outside the content collection, gaining a use-case router section | Same bypass the `is_index` Handlebars branch performs today |
| Visual design | Port the `index.hbs` tokens (ground `#161826`, text `#e9e9ed`, accent `#9184d9` / `#d2cefd` / `#2b2741`, neutrals `#595d6c` / `#3f424d`, 8px radii, system-ui body, ui-monospace kickers) onto Starlight's `--sl-color-*` via `customCss` | The landing page already is the product's visual vocabulary; docs pages currently do not share it. Draft: <https://claude.ai/code/artifact/4c5ba779-da78-444b-b258-450926724743> |
| Custom heading IDs | A rehype plugin honouring `{#custom-id}` is a hard requirement of phase 1 | Astro has no native support; without it 289 headings lose their anchor and render the literal brace text |
| Node in CI | `actions/setup-node` pinned to Node LTS, `npm ci`, lockfile committed under `docs/` | First Node toolchain in this repo's CI; a lockfile plus `ci` makes the build reproducible |
| Dependency pins | Exact-pin `astro`, `@astrojs/starlight`, `@astrojs/sitemap` in `docs/package.json`; record the versions at implementation time | Tech strategy says latest stable unless pinned; inventing version numbers in an ADR dates it |
| `seo.py` | Deleted. Astro `site` gives canonical URLs, `@astrojs/sitemap` gives `sitemap.xml`, per-page `description:` gives the meta description | Removes 197 lines of post-build HTML rewriting |
| `data-grim-version` | A build-time environment variable read by the landing page | No post-build stamping step survives |
| Schemas | `task schema:generate` writes to `docs/public/schemas/` | Astro `public/` is verbatim passthrough, so `/schemas/*.json` is unchanged |
| asciinema player | Stays vendored; loaded by the landing page and, since the 2026-09-06 amendment, by every page through Starlight's `head` array for the per-guide casts | The two served paths are frozen, so vendoring keeps one copy; the per-guide decision was taken by the discovery plan (`screencasts:` block) |
| Link style | `docs-style.md` reference-style links win; relax MD054 in this project's `markdownlint.jsonc` when the docs checks are wired up (docs-instrument phase) | 22 of 23 pages already follow the project rule (quality-audit finding 1, 499 hits) |

### Consequences

**Positive:**
- Every published URL, fragment and static asset keeps its exact path.
- Docs and index site share one visual vocabulary from one token set.
- `seo.py`, the vendored mdBook template and its exact-patch pin all go away.

**Negative:**
- First Node toolchain in this repo's CI: a new supply chain, a lockfile to audit,
  and an `npm ci` step in a workflow that was Rust-only.
- Starlight ships breaking changes on a fast minor cadence, and the theme override
  surface is exactly what those releases move. mdBook's pin was frozen; this one
  needs tending.
- `.html` URLs read as dated, and that is now permanent.
- A docs deploy failure blocks nothing but still costs CI minutes and attention,
  on a repo where docs rebuild on every `src/**` push.

**Risks:**
- *Custom heading IDs.* 289 `{#custom-id}` headings, 59 anchored deep links in
  shipped strings. Mitigation: the plugin lands in phase 1; the URL check asserts
  a sample of anchors as `id="..."` in the built HTML.
- *Sitemap extensions.* `@astrojs/sitemap` omits `.html` under this format
  (`withastro/astro#15526`, closed not planned). Mitigation: its `serialize(item)`
  hook rewrites each entry before write; assert one entry in the check.
- *Pagefind result URLs.* Pagefind documents stripping `index.html`, not what it
  emits for a bare `about.html`. Mitigation: verify on a real build in phase 1,
  before any content moves.
- *Pre-existing gap surfaced here.* `grim schema --kind mcp` stamps
  `$id: .../schemas/grim-mcp.schema.json`, but `task schema:generate` emits only
  config, publish and lock, so that URL 404s today. Add the fourth kind in phase 1.

## Technical Details

### URL contract asserted by CI

A post-build script greps the built output for each path. Absent path or absent
anchor fails the job before deploy.

```
/  /404.html  /og-card.png  /robots.txt  /sitemap.xml  /install.sh  /install.ps1
/introduction.html  /installation.html  /quickstart.html  /concepts.html
/clients.html  /commands.html  /configuration.html  /authentication.html
/package-index.html  /hosting-an-index.html  /ratings.html  /publishing.html
/ci.html  /self-hosted-gitlab.html  /agents.html  /mcp-servers.html
/artifacts.html  /vendor-metadata.html  /json-interface.html  /stability.html
/upgrading.html  /start.html  /privacy.html
/schemas/grimoire-config.schema.json  /schemas/grim-publish.schema.json
/schemas/grimoire-lock.schema.json    /schemas/grim-mcp.schema.json
```

The schema filenames are literal `$id` values in `src/command/schema.rs` — they
are not `config.json` / `publish.json` / `lock.json`.

Plus a fragment sample asserted as `id="..."` in the built HTML:
`configuration.html#registry-compatibility`, `publishing.html#batch-publish`,
`artifacts.html#skill-example-full`, `commands.html#tui`,
`vendor-metadata.html#claude-md-excludes`.

## Implementation Plan

Three PRs, in order. Each lands independently. (Amended 2026-09-06 at
decomposition: pages before landing, because the landing's router cards link
to the pages; twelve pages, not seven; the browse page is `browse.md`.)

1. [ ] **Migration.** Content byte-identical except frontmatter, the declaration
       moved below it, the four sidebar groups, `build.format: 'file'`, the
       custom-heading-id plugin, `docs/public/` for statics and schemas, the
       ported landing page, the Node CI job, the URL check, `grim-mcp` added to
       `schema:generate`, and the deletion of `seo.py` and `theme/`.
2. [ ] **The twelve use-case pages and their casts**, in the discovery plan's
       `order_of_work`: quickstart expand, `tutorials/own-index.md`,
       `guides/team-ci.md`, `guides/registries.md`,
       `guides/scopes-and-clients.md`, `first-skill.md`,
       `guides/shared-skills.md`, `guides/lifecycle.md`, `browse.md`,
       `guides/inspect.md`, `guides/versioning.md`, `guides/mcp-everywhere.md`,
       `guides/catalog-best-practices.md`, `publishing.md` expand; the cast
       player loaded on every page.
3. [ ] **Landing router and page style.** The use-case router section; the token
       port onto `--sl-color-*` for every docs page.

## Validation

- [ ] URL-contract check passes on the built output, fragments included.
- [ ] `task docs:build` (or its Astro successor) builds clean from a fresh clone.
- [ ] `doc_declaration.py --root docs` still returns zero findings post-frontmatter.
- [ ] Pagefind returns a resolvable link for a known page, on a real build.
- [ ] Lighthouse: not required.

## Links

- [Research: docs site migration](../research/research_docs_starlight_migration.md)
- [Discovery IA plan](../discovery/use-cases.yaml)
- [Docs quality audit](../discovery/quality-audit.md)
- [Astro `build.format`](https://docs.astro.build/en/reference/configuration-reference/#buildformat) · [Starlight config](https://starlight.astro.build/reference/configuration/)
- [Design draft](https://claude.ai/code/artifact/4c5ba779-da78-444b-b258-450926724743)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-09-06 | Architect agent | Initial draft |
| 2026-09-06 | hex-plan (decomposition) | PR order pages-before-landing; twelve pages (discovery `ia_plan`), `browse.md`; asciinema player loaded on every page for the per-guide casts (C-024); `/sitemap.xml` produced by a post-build copy; the fourth schema file — see `design_docs_site_redesign.md` § 8 |
