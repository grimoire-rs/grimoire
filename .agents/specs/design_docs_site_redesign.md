# Design note — docs site redesign (mdBook → Astro Starlight)

**Status:** proposed, not implemented. Written 2026-09-06 by the architect for
`/hex-execute`. **Governed by:** [`adr_docs_site_starlight.md`](../adr/adr_docs_site_starlight.md)
— that ADR decides; this record makes it executable.
**Constitution:** `AGENTS.md` › Core Principles. Principle 9 is the binding one.

Path convention used throughout: the discovery artifact names mdBook paths
(`docs/src/commands.md`). Under Astro the same page is
`docs/src/content/docs/commands.md` and still serves at `/commands.html`.
Where a contract says *discovery path P*, read *`docs/src/content/docs/<P minus
`docs/src/`>`*.

## 1. Why this exists, and scope

The site is 21 mdBook pages plus a 927-line Handlebars landing page whose visual
language no docs page can reach. The discovery pass wants four nav groups, twelve
new pages and eleven screencasts; a flat `SUMMARY.md` cannot express the first
and the Handlebars branch cannot host the rest. The ADR settles the generator and
the URL strategy. What is left is the seam: which files exist, what each must do,
and what a test asserts about it.

Three waves, three PRs, each landing independently.

| Wave | Content | Gate |
|---|---|---|
| **W1 Migration** | Astro+Starlight replaces mdBook. Page content byte-identical except added frontmatter and the declaration comment moved below it. Every URL, fragment and static path preserved. Landing ported as-is. | URL-contract check green on the built output |
| **W2 Landing and theme** | Landing rebuilt around entry paths and the pain router; `index.hbs` tokens ported onto `--sl-color-*` so docs pages inherit them | Landing renders the three entry tiles and eight router cards from one data module |
| **W3 Pages and casts** | Twelve new pages, two expands, `commands.md` additions, five drift fixes, eleven screencasts | `page_type.py` clean on every new page; every `[data-cast]` resolves |

**Landing order (decomposition amendment D-1).** The PRs land W1 → W3 →
W2: the landing's router cards link to the eight guide pages, so the pages
must exist before the landing that routes to them ships. C-024 (cast
embedding) lands with W3 for the same reason. The section labels below keep
the original grouping.

**Out of scope**, named so nobody widens the waves: the index site
(`index.grimoire.rs`, a separate repo and pipeline), VS Code extension reference
docs (they live in `grimoire-vscode`; only the *entry path* from VS Code is in
scope, T32), internationalisation, a light theme, and `setup.grimoire.rs`
(a separate redirector, zero references in this repo).

## 2. Component contracts

Each contract names a public surface, its behaviour, and one criterion a tester
can assert without reading the implementation.

### W1 — migration

**C-001 `docs/package.json`.** Declares `astro`, `@astrojs/starlight`,
`@astrojs/sitemap`, `remark-heading-id`, `rehype-autolink-headings`,
`astro-rehype-relative-markdown-links`, `starlight-links-validator`. Every
version an **exact pin**, no `^` or `~` in either dependency map — the tech
strategy's "latest stable unless pinned" resolves to pinned here, because
Starlight moves its override surface on a fast minor cadence. `engines.node` is
`">=24"` (the established LTS; the sibling indexer's `>=22.14.0` is the older
floor). Scripts `build`, `dev`, `preview`; `package-lock.json` committed. The
exact versions are resolved to the latest releases when W1 builds; the
peer-dependency ranges jointly set the floor and are re-read at that time. On
2026-09-06 that is Starlight 0.42.0 (peer `astro@^7.2.10`, so Astro 7.3.1),
`starlight-links-validator` 0.26.0 (peer `@astrojs/starlight >=0.42.0` — a
0.41.x pin fails `npm ci`) and `astro-rehype-relative-markdown-links` 0.19.2
(peer `astro >=2 <8`). *Acceptance:* no dependency version string in
`docs/package.json` matches `[\^~*x><]` or a range; `cd docs && npm ci` exits 0
from a clean `node_modules`.

**C-002 `docs/astro.config.mjs` — routing core.** `site: 'https://grimoire.rs'`,
**no `base`**, `build.format: 'file'`, `trailingSlash: 'never'`, default `outDir`
(`docs/dist`). No `base` is load-bearing: `withastro/starlight#2383` (header logo
points at the wrong URL under `file`) fires only when `base` is a subpath. Also
`editLink.baseUrl: 'https://github.com/grimoire-rs/grimoire/edit/main/docs/'`
(replacing `book.toml`'s `edit-url-template`), no `lastUpdated` (the mdBook
site shows no date; adding one is scope, D-11), and a global
`head` array carrying the default `og:image` (`${site}/og-card.png`) plus the
vendored player's CSS and script tags. Per-page `title`/`description` drive
`<title>`, the meta description and the OG/Twitter pairs with no `head` entry.
*Acceptance:* after `task docs:build`, `docs/dist/commands.html` exists and
`docs/dist/commands/index.html` does not; every internal `<a href>` in
`docs/dist/quickstart.html` ends in `.html`; `docs/dist/commands.html` carries
an `og:image` of `https://grimoire.rs/og-card.png` and an edit link into
`/edit/main/docs/`.

**C-003 sidebar.** Four groups, declared in order, from `ia_plan.groups`:
Getting started (introduction, installation, quickstart, browse, first-skill,
concepts) · Guides (guides/scopes-and-clients, guides/shared-skills,
guides/lifecycle, guides/inspect, guides/versioning, guides/mcp-everywhere,
clients) · Teams and automation (tutorials/own-index, guides/team-ci,
guides/registries, guides/catalog-best-practices, publishing, ci,
hosting-an-index, self-hosted-gitlab, authentication, ratings) · Reference
(commands, configuration, artifacts, agents, mcp-servers, vendor-metadata,
package-index, json-interface, stability, upgrading). Entries are explicit
(`slug` + `label`), never `autogenerate` — the order is editorial. `label` comes
from `ia_plan.nav_labels` where the map has a row, otherwise the page title.
Pages that do not exist yet are added to the config in W3, not stubbed in W1.
*Acceptance:* `docs/dist/commands.html` contains the four group labels in that
order, and contains `Artifact formats` (the `nav_label`) rather than
`Artifact Reference` (the title) in its sidebar markup.

**C-004 markdown plugin chain.** Exactly this order, and the order is the
contract:

```js
markdown: {
  remarkPlugins: [remarkHeadingId],
  rehypePlugins: [
    rehypeHeadingIds,
    [rehypeAutolinkHeadings, { behavior: 'wrap' }],
    astroRehypeRelativeMarkdownLinks,
  ],
}
```

`remark-heading-id` runs at the **remark** stage so the `{#custom-id}` suffix
is consumed before any rehype pass sees the heading text. Astro's own id pass
runs after user plugins and preserves an id already set (Astro docs, "Astro
injects `id` attributes after your custom plugins have run"); `rehypeHeadingIds`
is listed first among the rehype plugins so `rehype-autolink-headings` sees
every id, custom or generated. A rehype-stage custom-id plugin would also
keep its id — the remark choice is about stripping the brace suffix from the
rendered text, not a race.
`astro-rehype-relative-markdown-links` rewrites the 394 relative `./page.md#frag`
links against Astro's route manifest, which is why it beats a one-time source
rewrite: a hand-baked `.html` suffix silently drifts the day `build.format`
changes. *Acceptance:* `docs/dist/configuration.html` contains
`id="registry-compatibility"`; `docs/dist/concepts.html` contains no `href`
ending in `.md`.

**C-005 content collection schema.** `docs/src/content.config.ts` extends
Starlight's `docsSchema()` with `description` made **mandatory** (Starlight
already requires `title`). `description` replaces `seo.py`'s scraped first
paragraph; the 21 drafts are in `research_docs_migration_content_survey.md` § 2.
The collection accepts `.md` only. *Acceptance:* adding a page with no
`description:` makes `task docs:build` exit non-zero. `find
docs/src/content/docs -name '*.mdx' | wc -l` prints 0, asserted by C-010.

**C-006 declaration placement.** Every page keeps its
`<!-- doc_type: ... -->` (and `doc_tier` where present) comment, moved to sit
**directly below** the closing `---` of the frontmatter. Never inside the
frontmatter (DOC-TYPE-28: mdBook renders it as a fake `<h2>` that enters the
search index) and never above it (DOC-TYPE-29: frontmatter must start on line 1).
*Acceptance:* `python3 .claude/rules/docs-quality/checks/doc_declaration.py
--root docs/src/content/docs` exits 0.

**C-007 static passthrough.** `docs/public/` is copied verbatim to the site root
and holds every non-markdown asset served today: `install.sh`, `install.ps1`,
`robots.txt`, `og-card.png`, `favicon.png`, `favicon.svg`, `start.html`,
`privacy.html`, `demo.cast`, `asciinema-player.css`, `asciinema-player.min.js`,
and the generated `schemas/`. `og-card.png` stays Git-LFS tracked
(`.gitattributes` routes `*.png`), so CI keeps `lfs: true` or Pages ships a
pointer file. `.gitignore`'s `docs/book/` and `docs/src/schemas/` entries become
`docs/dist/` and `docs/public/schemas/`. *Acceptance:* every path in C-010's list
resolves under `docs/dist/`; `file docs/dist/og-card.png` reports PNG image data,
not ASCII text.

**C-008 `task schema:generate` gains the fourth kind.** Writes into
`docs/public/schemas/` and adds `grim schema --kind mcp >
grim-mcp.schema.json`. `src/command/schema.rs:42` already stamps
`$id: https://grimoire.rs/schemas/grim-mcp.schema.json`, and that URL **404s
today** — the generator only emits config, publish and lock. This is additive
(a new file at a URL the binary already advertises), so Principle 9 permits it
and the URL contract requires it. *Acceptance:* after `task schema:generate`,
all four files exist under `docs/public/schemas/` and each file's `$id` equals
`https://grimoire.rs/schemas/<its own filename>`.

**C-009 sitemap.** `@astrojs/sitemap` omits the `.html` extension under
`build.format: 'file'` (`withastro/astro#15526`, closed not planned). Its
`serialize(item)` hook rewrites each entry: append `.html` to every URL whose
path is neither `/` nor already suffixed. The integration has no filename
option and emits `sitemap-index.xml` plus `sitemap-0.xml`; `/sitemap.xml` is
a shipped path (`seo.py` writes it, `robots.txt` names it), so `task
docs:build` copies `sitemap-0.xml` to `docs/dist/sitemap.xml` after `npm run
build` (the site is far below the 50 000-URL chunk limit, so there is always
exactly one chunk). *Acceptance:* `docs/dist/sitemap.xml` exists, contains
`https://grimoire.rs/commands.html` and contains no
`https://grimoire.rs/commands<` without a suffix.

**C-010 URL-contract check — `docs/check_urls.py`.** Python 3.11+, standard
library only, matching the docs-quality check contract: `--root DIR`, `--format
text|json`, exit 0 clean / 1 finding / 2 usage. `--root` is the **repository
root** (items 3–5 read the source tree, `catalog/` and `README.md`); the built
tree defaults to `<root>/docs/dist` and `--dist DIR` overrides it. It asserts,
and fails on the first miss with `path: reason`:

1. Every path from the ADR's contract list exists: `/404.html`, `/og-card.png`,
   `/robots.txt`, `/sitemap.xml`, `/install.sh`, `/install.ps1`, `/favicon.png`,
   `/favicon.svg`, `/demo.cast`, `/start.html`, `/privacy.html`, plus the 21
   chapter pages as `<name>.html` (`introduction installation quickstart
   concepts clients commands configuration authentication package-index
   hosting-an-index ratings publishing ci self-hosted-gitlab agents mcp-servers
   artifacts vendor-metadata json-interface stability upgrading`).
2. All four schema files under `/schemas/`, each `$id` equal to its own
   canonical URL. The filenames are literal `$id` values, not derived from
   `--kind`: `grimoire-config`, `grim-mcp`, `grim-publish`, `grimoire-lock`.
3. **Every `{#custom-id}` resolves.** Re-read the source tree, extract each
   custom id, assert `id="<it>"` in the matching built page. All 289, not the
   ADR's five-anchor sample — a sample passes while a whole family is dropped.
4. **Catalog and README link lint.** Scan `catalog/**/*.md`, `catalog/README.md`
   and `README.md` for `https://grimoire.rs/<page>.html[#frag]` and assert each
   resolves in the built tree. Twenty files under `catalog/` plus `README.md`
   carry `https://grimoire.rs/*.html` links, 74 of them with a fragment, and no
   check reads them today; `task catalog:verify` validates schemas only.
5. No `*.mdx` under `docs/src/content/docs/`.

*Acceptance:* `python3 docs/check_urls.py --root .` (repo root; `--dist`
defaults to `docs/dist`) exits 0 on a clean build; deleting `docs/dist/quickstart.html` makes it exit 1 naming that path;
changing `{#registry-compatibility}` to `{#registry-compat}` in
`configuration.md` makes it exit 1 naming the fragment.

**C-011 link validation inside the build.** `starlight-links-validator` runs as a
Starlight plugin so a broken internal link fails `task docs:build`. It validates
*resolved* links after C-004's rewrite plugin, which is the only stage where the
394 rewritten links can be checked. This is the safety net for the rewrite
plugin's own README-declared experimental status. *Acceptance:* adding
`[x](./nope.md)` to any page makes `task docs:build` exit non-zero.

**C-012 taskfile.** `taskfiles/docs.taskfile.yml` replaces its two mdBook tasks:
`docs:build` (deps `:schema:generate`, then `npm ci && npm run build` in `docs/`),
`docs:serve` (deps `:schema:generate`, then `npm run dev`), `docs:check` (deps
`docs:build`, then `python3 docs/check_urls.py --root .`, `python3
.claude/rules/docs-quality/checks/doc_declaration.py --root
docs/src/content/docs`, `npm audit --audit-level=high` and `npm test
--if-present` in `docs/`; D-5, D-12). CI calls `task`, never raw `npm run build` — subsystem
taskfile rule 1. Unlike today, `docs:serve` needs no separate SEO caveat: the
canonical and OG tags come from `site` + frontmatter, so a local preview and the
deployed page carry the same head. *Acceptance:* `task --list` shows all three;
`task docs:check` exits 0 on a clean tree.

**C-013 CI — `.github/workflows/docs.yml`.** Four mechanical changes: the
`taiki-e/install-action` mdBook step becomes `actions/setup-node` (SHA-pinned
with a `# vN` comment, `node-version: 24`, `cache: npm`,
`cache-dependency-path: docs/package-lock.json`); `task docs:check` runs after
`task docs:build`; the artifact path becomes `docs/dist`; the `paths:` filter
gains `catalog/**` and `README.md` so C-010's catalog link lint has a trigger.
No `fetch-depth: 0` (D-11). Everything else must stay: `submodules:
recursive` (the `[patch.crates-io]` source the grim build needs), `lfs: true`,
the `contents: read / pages: write / id-token: write` set, the `group: pages,
cancel-in-progress: false` concurrency, the two-job split with its one-shot
deploy retry, and the `src/**` + `Cargo.*` path filter (schemas come from grim's
parse structs). *Acceptance:* every `uses:` line carries a 40-hex SHA; the
workflow has exactly one `concurrency:` block, `group: pages`.

**C-014 dependabot.** A third `package-ecosystem: npm` block, `directory:
/docs`, weekly, one `groups:` entry matching `"*"`, `commit-message.prefix:
"chore(deps)"` — matching the existing cargo block's shape. Without it the first
Node dependency tree in this repo goes unpatched indefinitely.
*Acceptance:* `.github/dependabot.yml` parses and contains three
`package-ecosystem` keys.

**C-015 migration script — `docs/migrate_frontmatter.py`.** One-shot, idempotent.
For each `docs/src/*.md` except `SUMMARY.md`: read the leading
`<!-- doc_type -->` / `<!-- doc_tier -->` comment lines, prepend a frontmatter
block carrying `title` and `description` from a table embedded in the script
(the 21 drafts from the content survey), re-emit the comment lines directly below
the closing `---`, leave every other byte untouched. Running it on an
already-migrated tree changes nothing. *Acceptance:* run it, `git add -A`, run it
again — `git diff --quiet` exits 0. `git diff --stat` after the first run shows
only added lines at the top of each file, zero deletions in page bodies.

**C-016 landing page ported (W1 shape).** `docs/src/pages/index.astro` wrapped in
`<StarlightPage>`, carrying the `index.hbs` `is_index` branch's markup and inline
styles verbatim. `.astro` in `src/pages/`, never `index.mdx` in the collection —
Starlight resolves `base` in hero actions correctly only on the `.astro` route
(`withastro/starlight#3660`), and it is the same bypass the Handlebars branch
performs today. The `data-grim-version` span, stamped post-build by `seo.py`
today, is filled at build time from an environment variable the taskfile derives
from `Cargo.toml`. W1 changes the landing's host, not its design.
*Acceptance:* `docs/dist/index.html` exists, mounts the `demo.cast` player, and
its `data-grim-version` span holds the crate version, not the literal `dev`.

**C-017 `introduction.html` keeps working as a page.** The new landing lives at
`/`; `introduction.md` stays a real page at `/introduction.html`, first entry of
Getting started. No redirect, no stub — Principle 9 is satisfied by the page
continuing to exist, which is strictly stronger than a meta-refresh that would
drop fragments. It keeps `doc_type: landing` and `doc_tier: first-steps`
unchanged, and W2 closes its three standing findings: DOC-TYPE-10 (lead-in is 4
sentences against a 1-sentence shape), DOC-TYPE-11 (first reachable action at
word 222, budget 150), DOC-DISC-16 (a first-steps page with no runnable command
block). Its "Where to next" list is rerouted to the four nav groups.
*Acceptance:* both `landing_check.py` (DOC-TYPE-10, DOC-TYPE-11) and
`page_type.py` (DOC-DISC-16 lives there, not in `landing_check.py`) exit 0 on
`docs/src/content/docs/introduction.md` after W2; `docs/dist/introduction.html`
exists after W1.

**C-018 deletions.** W1 removes `docs/seo.py` (197 lines: canonical/OG injection,
footer injection, sitemap, version stamp — all four now come from Astro),
`docs/theme/` (`index.hbs`, `clients-matrix.css` moves per C-021, favicons move
per C-007), `docs/book.toml`, `docs/src/SUMMARY.md`, and the mdBook install step
and its exact-patch pin rationale from CI and `docs/README.md`. `docs/docs.toml`
**stays** — it is the docs-quality tooling config, not an mdBook file.
*Acceptance:* `test ! -e docs/book.toml -a ! -e docs/seo.py -a ! -e docs/theme
-a ! -e docs/src/SUMMARY.md`; `grep -rli mdbook docs/docs.toml docs/README.md
.github/workflows/docs.yml taskfiles/ .claude/rules/docs-style.md` prints
nothing. `start.html`, `privacy.html` and `clients-matrix.css` still carry the
word in prose or comments; `privacy.html`'s localStorage claims are corrected
in W2 (C-020), the other two mentions are inert.

### W2 — landing and theme

**C-019 theme tokens.** One `customCss` file maps the landing's palette onto
Starlight's variables: ground `#161826` → `--sl-color-bg`, text `#e9e9ed` →
`--sl-color-white`/`--sl-color-text`, accents `#9184d9` / `#d2cefd` / `#2b2741`
→ `--sl-color-accent{,-high,-low}`, neutrals `#595d6c` / `#3f424d` →
`--sl-color-gray-*`, radii 4px and 8px, system-ui body at 15px/1.55,
ui-monospace kickers at 11.5px uppercase with 0.08em tracking. Author the
overrides **unlayered** so they beat Starlight's own `@layer` without a
specificity fight — the trick the sibling indexer's `Base.astro` already uses.
*Acceptance:* the stylesheet `docs/dist/commands.html` links contains
`--sl-color-bg` set to `#161826` (static; a browser's computed value is a
reviewer note, not the gate).

**C-020 component overrides.** Starlight overrides under `docs/src/components/`.
`ThemeProvider.astro` forces dark and drops the before-paint `localStorage` read;
`ThemeSelect.astro` renders nothing — no config flag for dark-only exists, and
overriding these two is the only supported mechanism. `Header.astro` is brand
left, search centred, nav right (index · docs · vscode · source).
`Footer.astro` carries the four-group nav plus the
home/documentation/stability/privacy/license links `seo.py` used to inject. The
guide-page frame is Starlight's own three-column layout retuned by C-019, not a
new component: 250px sidebar (four groups, `nav_labels`, current item in
accent-high), content column (kicker `type · tier · time`, H1, goal paragraph,
"before you start" box, numbered `##` steps each with a command block and its
shown output), 220px aside (on this page, the "see it run" cast slot, reference
links, edit link), prev/next below. The aside cannot carry a cast slot from a
`.md` page (raw HTML lands in the content column only), so the cast sits in
the content column under a "See it run" heading and `PageFrame` is not
overridden (amended at decomposition, D-4). *Acceptance:*
`docs/dist/commands.html` has no theme-toggle control, carries the dark
attribute with no script able to change it, and renders sidebar, content and
aside as three columns above 1200px.

**C-021 clients matrix.** `clients-matrix.css` (moved to
`docs/src/styles/` in W1 and registered in `customCss`, D-6) folds its 69
lines into the C-019 stylesheet, scoped to `.matrix-table table`, and is
deleted. `clients.md`'s
`<div class="matrix-table">` wrapper stays as raw HTML in the markdown — it is
the only raw HTML block in the corpus. Verify the div is balanced before porting:
mdBook tolerates an unclosed HTML block, Astro's parser may not.
*Acceptance:* `docs/dist/clients.html` contains `class="matrix-table"` and its
table cells are left-aligned.

**C-022 landing data module.** `docs/src/data/landing.ts` exports two typed
arrays: `entryPaths` (3 items: label, href, blurb) and `routerCards` (8 items:
pain, href, task ids). Both mirror `use-cases.yaml`'s `entry_paths` and `router`
blocks. **One module, one source** — the cards must not be hardcoded in the
component, the footer nav and the sidebar independently. *Acceptance:*
`routerCards.length === 8` and `entryPaths.length === 3`; grepping the component
files finds each pain string exactly zero times.

**C-023 landing components.** `WaysIn.astro` (three tiles: quick start, browse
the index, your first skill), `PainRouter.astro` (eight cards, four per row,
equal height, whole card a link, arrow affordance bottom-right),
`FooterNav.astro` (four-group nav). Cards are nav links, not a numbered step
list — the eight guides have no fixed order, which is exactly GOV.UK's stated
criterion for links over a task list. Card wording names the pain, never the
feature, matching Stripe's persona-framed "common use cases" rather than Fly.io's
feature-named grid. *Acceptance:* `docs/dist/index.html` contains eight anchors
whose `href` set equals the eight `routerCards` hrefs, and each card's entire
bounding element is inside its anchor.

**C-024 cast embedding.** Per-page markup is
`<div data-cast="/casts/<name>.cast" data-cast-poster="npt:0:03"></div>` — raw
HTML in `.md`, because `.md` cannot import Astro components. One global script,
injected through Starlight's `head` array, queries `[data-cast]` and calls
`AsciinemaPlayer.create(el.dataset.cast, el, { idleTimeLimit: 2, fit: 'width',
poster: el.dataset.castPoster })`, each `create` deferred behind an
`IntersectionObserver`. Never autoplay. Each embed is followed by a `<noscript>`
link to the raw `.cast`: asciinema has no native transcript, and that link is
what sites that care about it ship. Player source is the **vendored** 3.17.0
pair already in the tree (C-007), loaded by the same `head` array.
*Acceptance:* `docs/dist/quickstart.html` has one `[data-cast]` div and one
`<noscript>` link to the same `.cast`, and constructs no player before the
element intersects.

### W3 — pages and casts

**C-025 new pages.** Twelve pages, each written to its declared type. Every one
carries frontmatter (C-005), a declaration below it (C-006), a goal sentence in
prose before the first `##` (DOC-TYPE-22, MUST — zero stripped words there is a
failure), a hard prerequisite before step 1 where one exists (DOC-TYPE-23),
numbered reader actions with one fenced command each (DOC-TYPE-24), and a closing
"Next steps" or "See also" heading (DOC-TYPE-25).

| Discovery path | Type · tier | Goal sentence ends when the reader has… | Serves |
|---|---|---|---|
| `docs/src/browse.md` | how-to · first-steps | installed one artifact from the website, the TUI and VS Code | T05, T32, T06 |
| `docs/src/first-skill.md` | how-to · first-steps | written one `SKILL.md`, installed it from its path, seen the agent load it | T30 |
| `guides/scopes-and-clients.md` | how-to · everyday | moved one artifact between project and global scope, and added then dropped a client | T03, T07 |
| `guides/shared-skills.md` | how-to · everyday | one directory every harness on the repo reads, plus the machine-wide set from a registry | T31, T04 |
| `guides/lifecycle.md` | how-to · everyday | installed, updated, inspected and uninstalled, keeping a hand edit | T08, T13, T17 |
| `guides/inspect.md` | how-to · everyday | read contents, provenance, rating and deprecation, and pinned a digest, before install | T14, T15 |
| `guides/versioning.md` | how-to · everyday | chosen a rung on the ladder and knows what `grim update` moves | T28 |
| `guides/mcp-everywhere.md` | how-to · everyday | one descriptor registered into every client, and can read each client's entry | T19 |
| `guides/team-ci.md` | how-to · integration | a committed lock, a frozen install and a CI job that fails on divergence | T09 |
| `guides/registries.md` | how-to · integration | a company index beside the public one, filtered, with short-id expansion | T06, T11 |
| `guides/catalog-best-practices.md` | how-to · integration | a flat repository layout and namespace the index picks up | T29 |
| `tutorials/own-index.md` | tutorial · integration | published to their own index and watched a colleague install it | T10, T21, T27 |

`guides/versioning.md` follows ocx.sh's named ladder — floating, major-rolling,
minor-rolling, exact, digest — the only surveyed model naming every rung T28
needs *and* stating what a resolver-level lock adds on top of tags, which is
grim's lockfile-to-tag relationship exactly.

`tutorials/own-index.md` is the one typed tutorial and adds DOC-DISC-17 (MUST:
**no branching**, prose branches included — "or, with", "alternatively", "if you
prefer"), DOC-DISC-18 (every step shows a visible result) and DOC-DISC-19. Its
riskiest step, the manifest-registry versus `repository_prefix` trap that broke
the T10 persona, is restated inline as one rule rather than deferred to
`configuration.md`: a tutorial minimises explanation but never defers a fact the
path depends on. Only DOC-DISC-17 is implemented in `page_type.py`; DOC-DISC-18
and DOC-DISC-19 are reviewer duties with the rule text as the checklist.
DOC-DISC-17 has never run against a real tutorial, so its first green run is
evidence about the check as much as about the page. Likewise for every page
here: DOC-TYPE-22 and DOC-TYPE-25 are machine-checked, DOC-TYPE-23 and
DOC-TYPE-24 are marked "reading heuristic" in `page-types.md` and are the
reviewer's.
*Acceptance:* `python3 .claude/rules/docs-quality/checks/page_type.py --root
docs/src/content/docs` exits 0; each page is in C-003's sidebar and in
`docs/dist/` at its `.html` path.

**C-026 the two expands.** `quickstart.md` gains the answer to its own drift
(C-028 row 2): which files to commit, whether the agent needs a reload, and what
happens in a project with no client marker. `publishing.md` (T27) gains the
path from a skill on disk to a published artifact that the tutorial links into
rather than restates. Both keep every existing `{#custom-id}` — 44 of the 289
live in `publishing.md`, and 59 shipped deep links carry a fragment.
*Acceptance:* C-010 still exits 0 (no fragment was renamed or dropped).

**C-027 `commands.md` additions.** Three edits, all additive. Under
`grim install`: the **stale-lock refusal** — exit 65, message `grimoire.lock is
stale (declaration_hash {locked} does not match current {current}); run
grim lock before installing` (`src/command/command_error.rs:24`). This is the
real CI gate, and it is documented nowhere; `grim status --check` always exits 0
and cannot serve as one. Under `grim status`: **one status vocabulary**, stated
once under `## Artifact states {#artifact-states}` (D-7) and linked from
`guides/lifecycle.md` and `json-interface.md` rather than restated — installed, outdated, locally modified, integrity-missing, not
installed, and the `stale` value `json-interface.md` lists but never defines.
Under `grim search`: the source-column contradiction removed (C-028 row 1).
*Acceptance:* `commands.md` contains the literal string `declaration_hash`; the
words defining the state vocabulary appear in exactly one page.

**C-028 the five drift fixes.** Each is a documented claim the binary
contradicts, from `use-cases.yaml` `drift:`.

| # | Page | Fix |
|---|---|---|
| 1 | `commands.md` › `grim search` | One paragraph promises a source column in the plain table, a later one says source is JSON-only; the binary prints neither. Delete the claim. |
| 2 | `quickstart.md` › steps 2–3 | "into the clients it detects" is false in a fresh project with no client marker — the artifact lands in the agents pool, which Claude Code does not read, silently. State what actually happens and how to get the other outcome. |
| 3 | `commands.md` › `grim install` | The stale-lock refusal, per C-027. |
| 4 | `json-interface.md` › status report | Define `stale`; state that the outputs array drops a dropped client's still-on-disk output. |
| 5 | `configuration.md` › Qualified references | `alias/repo` syntax is shown only for oci-type aliases and fails with a parse error against an index-type alias. Say whether that is intended. |

Rows 2, 4 and 5 describe *product* behaviour the personas were surprised by.
Documenting it is this design's job; changing it is not, and the three
`product_observations` in the discovery artifact stay the owner's to triage.
*Acceptance:* one commit per row, each citing its `use-cases.yaml` drift entry.

**C-029 recording harness generalisation.** Today one hardcoded test
(`test/recordings/test_record_demo.py`) drives a fixed sequence. Eleven casts
need a per-cast **declarative script**: one data file per cast naming its output
path, command list and registry, consumed by a single runner reusing
`CastRecorder` and the `GrimRunner` sandbox (isolated `GRIM_HOME`, `HOME`,
`XDG_CONFIG_HOME`) unchanged. Every cast except `own-index.cast` records against
the acceptance suite's session-scoped local `registry:2` fixture, not the public
GHCR ref the demo uses today — otherwise eleven casts become eleven network
flake sources. Recordings stay outside `testpaths` (explicit `task test:demo`, never CI); casts stay committed and diff-reviewed; `assert_tables_column_aligned` keeps
guarding column drift. *Acceptance:* `task test:demo` produces every declared `.cast`
under `docs/public/casts/`, each valid asciicast v2 (first line parses as JSON
with `"version": 2`); only `own-index.cast` reaches the public registry.

**C-030 TUI capture — `risk`.** `browse-tui.cast` needs something the harness
cannot do: `cast_recorder.py` spawns a bash shell and captures line-buffered
output against a sentinel prompt; it never injects raw keystrokes mid-screen nor
captures a redrawing full-screen frame. Contract for the new piece (amended at
decomposition, D-3): a recorder that sends raw key sequences on a schedule and
records the **raw PTY output stream** with timestamps as asciicast v2 frames —
asciicast stores the stream and the player emulates the terminal, so no
screen capture is needed. The assertion runs on the escape-stripped raw
stream (`re.sub(r'\x1b\[[0-9;?]*[A-Za-z]|\x1b[()][A-Z0-9]|\x1b[=>]', '', s)`)
— the expected artifact name and column headers must appear in it — never on
an emulated screen: `pyte`, the usual Python emulator, has had no release
since 2023-11 and is documented to mishandle the alternate screen buffer
that ratatui enters. The fallback still is a PNG rendered by asciinema's own
`agg` (its `avt` emulator handles the alternate screen) from whatever the
recorder captured, committed under `docs/public/img/` (Git-LFS by
`.gitattributes`). This is new code against a PTY and a timing loop, and it is
the single largest unknown in W3.
**Fallback, so W3 is never blocked on it:** `browse.md` ships with a CLI-half
cast (`grim search`, `grim describe`, `grim add`) and the `agg` PNG of the TUI,
and `browse-tui.cast` becomes a follow-up. The VS Code half is a short screen
recording either way — out of scope for this harness.
*Acceptance (of the fallback, which is the one W3 depends on):* `browse.md`
renders one working `[data-cast]` and one TUI still, and C-010 exits 0.

## 3. User-experience scenarios

Three entry paths (S-001..003, the "ways in" strip), eight router cards
(S-004..011, one per `use-cases.yaml` `router` row), three migration scenarios
(S-012..014).

| ID | Tasks | Action → outcome | Error case the page must cover |
|---|---|---|---|
| S-001 | T02 | Clicks the quick-start tile, follows `quickstart.html` verbatim in a fresh project → a skill the agent loads, and knows which files to commit and whether the agent needs a reload | No client marker: the artifact lands in the shared agents pool, which Claude Code does not read. State it and state how to get the vendor directory instead |
| S-002 | T05, T32 | Clicks "browse the index" → installs one artifact from the website, one from the TUI, one from VS Code; one key-map table serves all three | `grim` not on `PATH` in VS Code — point at the extension's checksum-verified download offer, do not restate it |
| S-003 | T30 | Clicks "your first skill" → writes one `SKILL.md`, installs it from its path, sees the agent load it, no registry involved | Malformed frontmatter fails `grim build` with exit 65; the page names the code |
| S-004 | T31, T04, T19 | "the same skill, rule or MCP server has to live in `.claude`, `.cursor` and `.agents` and drifts" → one directory every harness reads, plus the machine-wide set | A client that declines the kind — link the compatibility matrix, never summarise it (it is code-mirrored) |
| S-005 | T03, T07 | "project or global, which agents, and a repo skill leaking everywhere" → one artifact moved between scopes, one client added and one dropped | A dropped client's output stays on disk and `grim status` does not list it (C-028 row 4) |
| S-006 | T14, T15 | "installing a stranger's skill without knowing what it does" → contents, provenance, rating and deprecation read, digest pinned, before anything materialises | Say plainly what grim does **not** check. This is a trust boundary, not a promise |
| S-007 | T06 | "the catalog shows everything from every registry; my team should see only ours" → a company index beside the public one, filtered, short ids expanding | `alias/repo` against an index-type alias fails with a parse error (C-028 row 5) |
| S-008 | T08, T13, T17 | "a sync tool rewrote the file I edited" → keeps a hand edit across an update, or discards it deliberately | `grim install` refuses on a modified artifact; one vocabulary names that state everywhere it appears |
| S-009 | T28 | "which reference moves on update and which never does" → a chosen rung on the ladder, correct expectation of `grim update` | A floating tag moved under a pinned lock: what the lock freezes versus what re-resolution changes |
| S-010 | T09 | "CI cannot prove the reviewed skills are the ones that shipped" → a job that fails when the tree diverges from its lock | The gate is `grim install`'s stale-lock exit 65, **not** `grim status --check`, which always exits 0 |
| S-011 | T10, T21, T27, T29 | "private skills have nowhere to live but per-repo copies" → skill on disk, dev-install, publish, announce, colleague installs; one unbranching path | The manifest-registry versus `repository_prefix` mismatch, stated inline as one rule where it bites |
| S-012 | — | Opens `https://grimoire.rs/configuration.html#registry-compatibility`, the URL `registry_catalog.rs:61` prints → page loads, scrolls to the heading, nothing redirects because nothing moved | Heading-id plugin regressed: C-010 fails the build, deploy never runs |
| S-013 | — | Editor fetches `https://grimoire.rs/schemas/grimoire-config.schema.json` from the `#:schema` header `grim init` writes → resolves, `$id` byte-identical to the request. `grim-mcp.schema.json` now resolves too | A schema file absent or its `$id` altered: C-010 exit 1 |
| S-014 | — | Someone renames a page or drops a fragment → `task docs:check` exits 1 naming the path or fragment, deploy never runs, the live site keeps the previous build | — |

## 4. Error taxonomy

| Failure | Surfaces as | Remediation |
|---|---|---|
| Broken internal link | `starlight-links-validator` fails `task docs:build` (C-011) | Fix the link; the validator names source page and target |
| Missing page, static path or schema | `docs/check_urls.py` exit 1, `path: not found` (C-010) | Restore the page, or the path was renamed — Principle 9 says restore |
| Missing heading anchor | `docs/check_urls.py` exit 1, `page.md:N: fragment #x has no id in dist` | The `{#custom-id}` was dropped, or C-004's plugin order regressed |
| Catalog link rot | `docs/check_urls.py` exit 1 naming the `catalog/**` file | Fix the catalog reference file; drift duty is `catalog/README.md` |
| Declaration misplaced | `doc_declaration.py` DOC-TYPE-28/29 | Move the comment below the frontmatter |
| Missing `description` | Astro collection schema error, build exit non-zero | Add it; the survey has a draft for all 21 original pages |
| `.mdx` added | `docs/check_urls.py` exit 1 | Rename to `.md` — MDX parses `<` as JSX and the declaration comment becomes a parse error |
| Page fails its type | `page_type.py` exit 1 with a DOC-TYPE/DOC-DISC id | The rule's own text says what the page shape must be |
| Recording: network | `task test:demo` fails on the GHCR pull | Only `own-index.cast` may reach the public registry; move the cast to the local `registry:2` fixture |
| Recording: TUI | No capture exists (C-030) | Ship the fallback: CLI-half cast plus a still |
| Deploy flake | `deploy-pages` terminal "try again later" | Already handled — the one-shot retry step stays |

Note the deliberate split: link and URL failures fail the **build** job, so the
deploy job never runs and the previous build keeps serving. That is stricter than
subsystem-ci.md's "never let lint block test results", and correct here: a docs
site with a dead shipped URL is a Principle 9 breach, not a lint finding.

## 5. Edge cases

| Case | Handling |
|---|---|
| `404.html` | Starlight's built-in 404 route emits it under `build.format: 'file'` with no work. No custom `404.astro`, no `disable404Route`. C-010 asserts it |
| `start.html`, `privacy.html` | Raw standalone HTML with their own `<head>`, never in `SUMMARY.md`. They move to `docs/public/` and are copied verbatim — a simpler contract than today's, and it deletes the third place (`seo.py`'s `SKIP` list) that had to know they are special. Their in-file `<title>`-in-a-comment warning was a `seo.py` constraint and is now moot; leave it, they are shipped pages |
| `clients.md` matrix | The one raw HTML block in the corpus, and it is code-mirrored: a table-parity test in `src/install/client_target.rs` fails the build on any cell drift. The migration touches no cell. Verify the `<div>` is balanced before porting — mdBook tolerates an unclosed block, Astro may not |
| `# doc:` fence markers | Zero pages use them today. A new convention on new pages only in W3 — not retrofitted onto the 222 untagged fences |
| Pagefind under `.html` | `Search.astro` sets `data-strip-trailing-slash` under `trailingSlash: 'never'` and strips client-side — confirmed in source at 0.41.11 (§ 9, closed). W1 clicks one result as confirmation |
| `lastUpdated` | Not enabled (D-11): the mdBook site shows no date, and feeding it needs a full-history clone on every docs build |
| Dark only | No light palette ships. C-020's `ThemeProvider` forces it; `prefers-color-scheme` is not consulted |
| `/print.html` | mdBook generated it; Astro emits no such page; the ADR's contract list omits it. The `Disallow` line was dropped from `robots.txt` in review round 1 — a rule against a URL the site cannot serve reads as a page someone is hiding |
| Canonical tags | Emitted by Starlight and `trailingSlash`-aware since 0.33.0 (§ 9, closed). W1 asserts `rel="canonical"` on `commands.html` |

## 6. Trade-offs

The ADR chose `build.format: 'file'` over a redirect map and over rewriting the
hardcoded strings; that is not reopened. A meta-refresh stub drops the URL
fragment and 59 shipped deep links carry one, and a string a released binary
already printed cannot be un-printed. Four further trade-offs arise here.

**Vendored player versus npm.** npm would put `asciinema-player` under
dependabot; vendored keeps one copy. The two vendored files are served at
`/asciinema-player.css` and `/asciinema-player.min.js`, frozen paths, so adding
the package means shipping *both* copies. **Keep vendored**, matching the ADR.
Revisit on a player CVE, where the bump is a one-file diff.

**Plugin versus hand-rewrite for relative links.** A one-time rewrite bakes
today's `build.format` into 394 places and drifts silently the day it changes.
**Take the plugin**, experimental README and all, because
`starlight-links-validator` (C-011) turns a plugin regression into a build
failure rather than a dead link on the live site. That inverts the risk: the
experimental dependency cannot ship a broken page.

**One browse page versus two.** Every comparable — GitHub CLI/Desktop/web,
Docker CLI/Desktop — splits by surface, because a GUI walkthrough is screenshots
and a CLI walkthrough is commands. **Keep one page** (the owner's decision), per
-surface sections, TUI first, because the reader's task is one task: browse,
inspect, install. Split it the moment either surface outgrows a screen — the
research's own escape hatch, not a concession.

**Harness extension versus VHS.** VHS gives deterministic timing and CI
re-rendering of a `.tape`, proving the tape is *reproducible*, not that the
output was *correct*. The pytest harness produces a cast only when the commands
succeeded, because it records inside an assertion. **Keep it**; take VHS's good
ideas (idle trim, poster, lazy load) as player options in C-024 instead.

## 7. Constitution check

| Principle | How this design honours it |
|---|---|
| 1 Understand first | Every contract cites the file it changes. The 289-anchor sweep and the 394-link count come from reading the corpus, not estimating it. |
| 2 Prove it works | Every contract carries a command and an expected exit code. `task docs:check` is the gate; C-010 fails the build before deploy. |
| 3 Keep it safe | No secrets. Every CI action stays SHA-pinned; the permission set is unchanged and not broadened for Node. Dependabot covers the new npm tree (C-014) or it rots. |
| 4 Keep it simple | Net deletion in W1: `seo.py` (197 lines), `index.hbs` (927), `book.toml`, `SUMMARY.md`, the mdBook pin. One new check script, standard library only. |
| 5 DRY | Router cards, entry paths and their labels live in one typed module (C-022). The status vocabulary is stated once (C-027). The nav is the sidebar config, not a second `SUMMARY.md`. |
| 6 Ship it | Three PRs, each independently landable, on `docs/use-case-discovery`. No push. |
| 7 Leave a trail | This record, the ADR, the five research artifacts, and the discovery YAML the contracts cite by key. |
| 8 Learn and adapt | The five drift fixes (C-028) are persona findings turned into content. The three product observations stay the owner's to triage, not silently patched. |
| 9 Preserve compatibility | Line by line below. |

**Principle 9, line by line.**

| Frozen surface | Why it holds | Asserted by |
|---|---|---|
| 21 page URLs | `build.format: 'file'` + `trailingSlash: 'never'` + no `base`; nothing redirects because nothing moved | C-002, C-010 |
| 289 fragments | remark-stage custom-id plugin; a **full sweep** of all 289, not the ADR's five-anchor sample | C-004, C-010 |
| 4 schema `$id` values | literals in `src/command/schema.rs`, untouched. C-008 makes the fourth resolve instead of 404 — additive, which Principle 9 permits | C-008, C-010 |
| Static paths | installers, `robots.txt`, `og-card.png`, both favicons, `demo.cast`, both vendored player files, `start.html`, `privacy.html`, `sitemap.xml`, `404.html` | C-007, C-010 |
| CLI output strings | no *shipped* string in `src/` is touched — the whole reason the ADR's Option 1 won. Fifteen `#[cfg(test)]` path literals that read `docs/src/<page>.md` (`client_target.rs`, six `vendor_*.rs`, `catalog_service.rs`) follow the files they read (D-13); test fixtures are not a released surface | `task rust:test:unit` green at WP-D |
| 74 catalog fragment links | twenty `catalog/**` files plus `README.md` keep resolving unedited; now a checked fact with a CI trigger (`catalog/**` in the docs workflow's `paths:`) | C-010, C-013 |
| `introduction.html` | stays a page, not a redirect — strictly stronger, since a meta-refresh drops fragments | C-017 |
| Removals | `seo.py` and `theme/` are build machinery, never served; deleting them removes no URL | C-018 |

## 8. Decomposition amendments

Recorded by `/hex-plan` on 2026-09-06; the plan's "Decisions taken at
decomposition" table (D-1…D-13) is the authority, this list is the pointer.
D-1 PR order W1 → W3 → W2 and D-2 C-024 with W3 (§ 1). D-3 raw-stream TUI
recorder (C-030). D-4 cast in the content column (C-020). D-5 documented
verification is `task verify && task docs:check`. D-6 `clients-matrix.css`
interim home `docs/src/styles/` in W1 (C-018, C-021). D-7 status vocabulary
anchor `commands.md#artifact-states` (C-027). D-8 one YAML per cast (C-029).
D-9 `nav_depth.py` gains a Starlight branch (no contract listed it; the check
would otherwise return clean on a generator it cannot read). D-10 `/sitemap.xml`
by post-build copy (C-009). D-11 no `lastUpdated`, no `fetch-depth: 0` (C-002,
C-013). D-12 `npm audit --audit-level=high` runs in `task docs:check` (the
lockfile pins the transitive tree and nothing else reads it). D-13 readers of
the moved pages move with them in the same WP: fifteen `src/**` test reads
across eight files plus fourteen comment-only files, `test/tests/test_docs.py` (retargeted, `rglob`, `../` links, a
non-empty assertion, an allowlist of the twelve planned slugs so page WPs can
self-check links without a build), `catalog/README.md`'s trigger path list and
`.claude/hooks/post_tool_use_tracker.py`'s paths. Also amended: the interim
stylesheet is `docs/src/styles/theme.css` from W1 (D-6); `guides/` and
`tutorials/` pages link root pages as `../<page>.md` (D-7); the TUI raw-stream
extension of the recorder lives in the entry-pages WP, not the harness WP
(C-030).

**Execution amendments (E-1…E-5).** Recorded by `/hex-execute` on 2026-09-07 as the
review seats raised them against these contracts; the plan's "Amendments taken during
execution" table is the authority, this list is the pointer. E-1 the catalog link lint
covers `catalog/publish.toml`, not only markdown (C-010 item 4 contradicted itself).
E-2 the binary static assets are content-checked, so a Git-LFS pointer cannot pass
(C-010 item 1). E-3 findings render `path:line: [rule] message` with a real source line
(C-010 output). E-4 the sidebar must cover the page set, asserted by `check_urls.py` —
deleting `SUMMARY.md` removed the only disk→nav check and C-003 asserted nav→disk only.
E-5 the `src/` migration property is reverse-substitution identity, not a literal grep
for `docs/src/`, because rustfmt rewraps four call sites (C-015, D-13).

## 9. Open questions

1. ~~Does Starlight emit `<link rel="canonical">` automatically?~~ **Closed at
   plan review**: yes — Starlight generates it and
   [withastro/starlight#2927](https://github.com/withastro/starlight/pull/2927)
   (0.33.0) made it honour `trailingSlash: 'never'`. W1 asserts
   `rel="canonical"` on `commands.html` as confirmation, nothing is added.
2. ~~Does Pagefind return a clickable result under `file` + `never`?~~ **Closed
   at plan review**: `Search.astro` at 0.41.11 sets `data-strip-trailing-slash`
   when `trailingSlash === 'never'` and applies
   `path.replace(/(.)\/(#.*)?$/, '$1$2')` to every result href. W1 clicks one
   result as confirmation, before any new page is written.
3. **Does `browse-tui.cast` ship in W3 or later?** C-030's raw-stream
   recorder is new code against a PTY and a timing loop and is the largest
   unknown in the wave. *Recommended:* ship W3 on the fallback (CLI-half cast plus a TUI
   still), and raise the recorder as its own issue. `browse.md` is an entry path
   and must not wait on a recording tool.
