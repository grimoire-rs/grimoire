# Research: Astro Starlight stack for the docs migration

**Date:** 2026-09-06
**Run:** hex-plan xhigh, docs redesign (`.agents/plans/plan_docs_site_redesign.md`)
**Phase:** Research, technology axis
**Consumers:** `.agents/specs/design_docs_site_redesign.md`, the plan.

# Research: Astro Starlight technology axis

Grounded against `adr_docs_site_starlight.md` (Astro Starlight, `build.format: 'file'` +
`trailingSlash: 'never'`, `.md` pages with title frontmatter, custom landing page outside
the collection, four sidebar groups, GitHub Pages at grimoire.rs).

## 1. Versions (as of 2026-09-06)

| Package | Version | Source |
|---|---|---|
| astro | 7.3.1 (7.3.0 released 2026-09-03) | [npm](https://www.npmjs.com/package/astro), [astro.build blog](https://astro.build/blog/) |
| @astrojs/starlight | 0.41.11 | [npm](https://www.npmjs.com/package/@astrojs/starlight), [releases](https://github.com/withastro/starlight/releases) |
| @astrojs/sitemap | not directly queried — pin to latest `@astrojs/sitemap@^3` compatible with Astro 7; verify with `npm view @astrojs/sitemap version` at implementation time (**not independently confirmed this pass**) |
| asciinema-player | 3.17.0 | [npm](https://www.npmjs.com/package/asciinema-player), [libraries.io](https://libraries.io/npm/asciinema-player) |
| Node.js LTS | 24.x is the established LTS; Node 26 enters LTS in October 2026 | [endoflife.date](https://endoflife.date/nodejs), [InfoQ](https://www.infoq.com/news/2026/06/nodejs-release-changes/) |

**Starlight/Astro compatibility**: Starlight 0.41.x requires Astro **v7** (peer dep
`astro@^7`) and dropped Astro v6 support in 0.41.0 — [npm](https://www.npmjs.com/package/@astrojs/starlight),
[releases](https://github.com/withastro/starlight/releases). Pin `astro@^7.3` +
`@astrojs/starlight@^0.41`.

**Recommendation**: use astro 7.3.x + @astrojs/starlight 0.41.x + asciinema-player 3.17.x + Node 24 LTS in CI.

## 2. `build.format: 'file'` known issues

- General Astro issue: `build.format`/`trailingSlash`/`Astro.url` interactions are
  inconsistent across the matrix of options —
  [withastro/astro#12833](https://github.com/withastro/astro/issues/12833). No fix
  targeted; treat as a standing rough edge, not a blocker, since our combo
  (`file` + `never`) is one of the more common ones and is exercised in Starlight's
  own test suite.
- Starlight-specific: setting `base` together with `build.format: 'file'` makes the
  header logo/link point at the wrong URL —
  [withastro/starlight#2383](https://github.com/withastro/starlight/issues/2383).
  **Not applicable** to this project: the ADR uses no `base` (site is served at
  `grimoire.rs` root), so this bug's precondition doesn't occur.
- **Pagefind search results under `trailingSlash: 'never'`**: Pagefind's static index
  encodes URLs with a trailing slash (standard SSG `index.html`-under-a-directory
  behavior), which would 404 under `never` if unhandled —
  [withastro/starlight#1358 discussion](https://github.com/withastro/starlight/discussions/1358).
  **Already mitigated in Starlight itself**: `Search.astro` sets a
  `data-strip-trailing-slash` attribute when `trailingSlash === 'never'` in the
  project config, and strips the slash client-side before navigating —
  [Search.astro source](https://github.com/withastro/starlight/blob/main/packages/starlight/components/Search.astro).
  Verify this still holds at implementation time by grepping that file at the pinned
  version; it has been in Starlight for several releases, so risk is low but not zero.
- **`ensureHtmlExtension`**: no evidence found of a Starlight/Astro flag with that
  exact name; `build.format: 'file'` is what makes Astro itself emit `page.html`
  instead of `page/index.html` — no dedicated Starlight-side option, no plugin
  needed.

**Recommendation**: proceed with `build.format: 'file'` + `trailingSlash: 'never'`
without a `base`. Add a smoke check post-build (link + search round-trip) rather
than pre-emptively patching around #12833/#2383 — neither applies to this
project's exact config.

## 3. Custom heading ids (`## Title {#custom-id}`)

Confirmed via Starlight's own discussion thread on exactly this feature request —
[withastro/starlight#2050](https://github.com/withastro/starlight/discussions/2050).

**Decision: `remark-heading-id`, not a rehype-stage plugin.** Correction at plan
review (2026-09-06): Astro's docs state it injects `id` attributes *after* user
plugins have run and preserves an id already set, so a rehype-stage plugin would
not lose a race. The remark stage is chosen because it consumes the `{#id}`
suffix before the heading text reaches rehype, and `rehypeHeadingIds` is listed
first among the rehype plugins so `rehype-autolink-headings` sees every id.

Working `astro.config.mjs` `markdown` block (source: the discussion's accepted
solution, cross-referenced with Astro's own markdown docs on `getHeadings()`
ordering — [docs.astro.build/guides/markdown-content](https://docs.astro.build/en/guides/markdown-content/)):

```js
import remarkHeadingId from 'remark-heading-id'; // or remark-custom-heading-id — same mechanism
import rehypeHeadingIds from '@astrojs/markdown-remark/rehype-heading-ids'; // Astro's own pass, chained explicitly
import rehypeAutolinkHeadings from 'rehype-autolink-headings';

markdown: {
  remarkPlugins: [remarkHeadingId],
  rehypePlugins: [
    rehypeHeadingIds,           // Astro's id-assignment pass, re-run explicitly so it sees the custom id and doesn't overwrite it
    [rehypeAutolinkHeadings, { behavior: 'wrap' }],
  ],
},
```

Starlight's TOC reads the id off the hast node's `properties.id` after all
markdown plugins run (same node Astro's `getHeadings()` walks), so once the custom
id is stamped before Astro's own pass, both the rendered `<h2 id="custom-id">` and
the TOC/anchor links pick it up automatically — no separate Starlight-side config.
Starlight's own `headingLinks: false` config option (Starlight docs, via context7)
only *disables* the clickable anchor icon; it's unrelated to id assignment.

**Two rejected candidates**: `rehype-custom-heading-id` and `rehype-slug-custom-id`
operate too late in the pipeline (same problem as bare `rehypeHeadingIds` alone) —
the discussion explicitly names this as the failure mode users hit first.

## 4. Relative `.md` links

Astro does not rewrite `./page.md#anchor` links itself — confirmed:
[docs.astro.build/guides/markdown-content](https://docs.astro.build/en/guides/markdown-content/)
covers Astro's own heading-id injection but link rewriting is a separate concern
handled by a rehype plugin.

Two real options surfaced:

- **`astro-rehype-relative-markdown-links`** — rehype plugin, `markdown.rehypePlugins`,
  purpose-built for this, supports query strings and hashes
  (`./other.md?query=test#hash`) —
  [npm](https://www.npmjs.com/package/astro-rehype-relative-markdown-links),
  [GitHub](https://github.com/vernak2539/astro-rehype-relative-markdown-links). Marked
  experimental by its own README.
- **One-time source rewrite** to `/page/#anchor` (matching `trailingSlash: 'never'`
  → actually `/page#anchor` with no trailing slash) done once across the docs tree.

**Recommendation**: use `astro-rehype-relative-markdown-links` over a one-time
rewrite. It is safer under `build.format: 'file'` specifically because it resolves
against Astro's actual route manifest at build time — a hand-rewrite baked to
`.html` or a bare slash would silently drift the day `build.format` or
`trailingSlash` changes (Principle 9 compatibility risk cuts the other way here:
the plugin is the additive-safe choice, not the hand edit). Gate correctness with
**`starlight-links-validator`** in CI — it validates the *final* resolved links
(post-plugin) against the real page graph, not the raw markdown source —
[GitHub](https://github.com/HiDeoo/starlight-links-validator),
[demo](https://starlight-links-validator.vercel.app/). No evidence either tool has a
documented incompatibility with `build.format: 'file'` specifically; both operate
on the content/route graph, not on emitted file paths.

## 5. Landing page outside the collection

Confirmed, Starlight docs via context7
([guides/customization.mdx](https://github.com/withastro/starlight/blob/main/docs/src/content/docs/guides/customization.mdx)):

- A file at `src/pages/index.astro` **does** override Starlight's default splash —
  Astro's file-based router in `src/pages/` takes precedence, and Starlight only
  auto-generates a landing page when no matching page/route exists in `src/pages/`
  or `src/content/docs/index.{md,mdx}`.
- To keep Starlight's chrome (header, sidebar-off, styles) while writing raw
  `.astro`, wrap the page in the `StarlightPage` component — confirmed useful
  distinction found for actions/hero base-url handling
  ([Starlight discussion #3660](https://github.com/withastro/starlight/discussions/3660)):
  `index.astro` + `StarlightPage` resolves `base` in `hero.actions` correctly, where
  an equivalent `index.mdx` under the content collection does **not** (a real bug
  to avoid by using the `.astro` route, matching the ADR's decision).
- `build.format: 'file'` does emit `index.html` at the site root for the `/` route
  regardless of source type — this is Astro's general `file` behavior (one `.html`
  file per route), not something Starlight overrides — [Astro build config](https://docs.astro.build/en/reference/configuration-reference/#buildformat).
- Custom `404.astro` at `src/pages/404.astro` + `disable404Route: true` disables
  Starlight's built-in 404 route entirely if a fully custom layout is wanted;
  otherwise `src/content/docs/404.md` with `template: splash` is the low-effort
  path (Starlight docs, via context7).

## 6. Dark-only theme, custom tokens, Header override

- **Disabling the theme toggle / forcing dark**: no built-in config flag exists.
  Confirmed via Starlight's own component schema — the *only* way is to override
  the `ThemeProvider` and `ThemeSelect` components
  ([`components.ts` schema](https://github.com/withastro/starlight/blob/main/packages/starlight/schemas/components.ts)),
  wired via `starlight({ components: { ThemeProvider: './src/components/ThemeProvider.astro', ThemeSelect: './src/components/ThemeSelect.astro' } })`
  per the [Overrides Reference](https://starlight.astro.build/reference/overrides/).
  A forced-dark `ThemeProvider` override is a small file (skip the toggle UI
  entirely by overriding `ThemeSelect` with a no-op/empty component).
- **Custom tokens**: override `--sl-color-*`, `--sl-content-width`,
  `--sl-sidebar-width`, font vars via `customCss` array pointing at a project CSS
  file, or via `@astrojs/starlight-tailwind` `@theme` tokens if Tailwind is in the
  stack — both patterns confirmed with exact variable names in the Starlight docs
  (via context7, `guides/css-and-tailwind.mdx`).
- **Header override** (brand, centred search, nav links): override the `Header`
  component the same way as `ThemeProvider` above — component-override mechanism is
  generic across all of Starlight's named slots (confirmed via the schema file);
  no separate research needed, same reference doc governs it.
- **Expressive Code styling**: match the landing's `pre` style via
  `starlight({ expressiveCode: { styleOverrides: { ... } } })` or a `customCss` file
  targeting Expressive Code's own CSS custom properties — mechanism confirmed in
  the same config surface as `expressiveCode.shiki.langs` shown in Starlight's own
  production `astro.config.mjs` (via context7). Did not pull the full Expressive
  Code variable list this pass — low risk, defer to implementation-time docs read.

## 7. asciinema-player in Astro

`.md` files cannot import Astro components, so the player must be initialized by a
**global client-side script**, not per-embed component import. Two viable patterns,
neither fully spec'd by an off-the-shelf integration (the one third-party
`astro-terminal-player` component exists but is unmaintained-scale and adds a
dependency for what is a few lines — [code.juliancataldo.com](https://code.juliancataldo.com/component/astro-terminal-player/)):

- **Pattern A (recommended): Starlight `head` script + `data-cast` markers.**
  Author markdown as a plain `<div data-cast="/casts/demo.cast" data-cast-poster="npt:0:03"></div>`
  (raw HTML is allowed inline in `.md`). Ship one global script (via Starlight's
  `head` config array, `{ tag: 'script', attrs: { src: '/scripts/casts.js', type: 'module', defer: true } }`)
  that on `DOMContentLoaded` queries `[data-cast]` and calls
  `AsciinemaPlayer.create(el.dataset.cast, el, { idleTimeLimit: 2, fit: 'width', theme: 'asciinema', poster: el.dataset.castPoster })`
  for each — confirmed the `AsciinemaPlayer.create(src, el, opts)` API shape and
  `idleTimeLimit`/`fit`/`theme`/`poster` options exist —
  [asciinema player docs](https://docs.asciinema.org/manual/player/quick-start/),
  [options reference](https://docs.asciinema.org/manual/player/options/).
- **Pattern B: Starlight component override for a custom remark directive** — more
  machinery for no real gain over Pattern A; skip unless authoring ergonomics
  become a real pain point.
- Load `asciinema-player`'s CSS the same global way (`{ tag: 'link', attrs: { rel: 'stylesheet', href: '/scripts/asciinema-player.css' } }`
  or bundle it into `customCss`).
- **Lazy loading**: don't gate on Astro islands (no framework component exists to
  hydrate) — use a plain `IntersectionObserver` in the same global script to defer
  `AsciinemaPlayer.create` until each `[data-cast]` div scrolls near-viewport. This
  is bespoke code, not a library feature — flag as implementation-time work, not a
  research gap.

## 8. GitHub Pages deploy

- **`withastro/action` vs manual `upload-pages-artifact`**: `withastro/action`
  internally wraps `actions/upload-pages-artifact` and handles the Astro build for
  you — [action.yml](https://github.com/withastro/action/blob/main/action.yml),
  [repo](https://github.com/withastro/action). Given the repo's stated convention
  of pinning actions by SHA and avoiding an extra layer of indirection over a
  plain `npm ci && npm run build`, the **manual path
  (`npm ci` → `npm run build` → `actions/upload-pages-artifact` → `actions/deploy-pages`)
  is the better fit** — it's 3 pinnable, auditable actions instead of one
  wrapper action whose internal action-pin behavior you don't control. This is a
  recommendation on **fit with this repo's existing CI conventions**
  ([subsystem-ci.md](../../../.claude/rules/subsystem-ci.md) — SHA-pinned actions),
  not a technical requirement — both work.
- **`lastUpdated: true`** needs full git history: `actions/checkout` must set
  `fetch-depth: 0`, else every page's "last updated" reads the shallow-clone date —
  general `actions/checkout` behavior, not Starlight-specific
  ([actions/checkout](https://github.com/actions/checkout)); Starlight's own docs
  reference this same checkout requirement for the `lastUpdated` feature.
- `site: 'https://grimoire.rs'`, no `base` — confirmed as the correct combination
  for a root-domain GitHub Pages / custom-domain deployment (no path prefix needed
  when the custom domain serves from `/`).
- `editLink.baseUrl` — a plain Starlight config string pointing at the repo's edit
  URL (e.g. `https://github.com/<org>/grimoire/edit/main/docs/`), confirmed in the
  production Starlight `astro.config.mjs` sample pulled via context7.

## 9. OG / meta tags

Confirmed via Starlight's `frontmatter.md` reference (context7): a page's `title`
and `description` frontmatter fields are used by Starlight to emit standard
`<title>`, `<meta name="description">`, and their `og:title`/`og:description`/
`twitter:*` equivalents **automatically** — no manual `head` entries needed for
those. **Not independently found**: an explicit statement that Starlight also
auto-emits `<link rel="canonical">` — the config surface only directly documents
`head` overrides and the `site` value being used to build absolute URLs; treat
canonical-tag presence as **needs a one-line implementation-time check** (view
source on a built page) rather than an assumed guarantee.

For **default `og:image`**, use the global `head` array with a static
`{ tag: 'meta', attrs: { property: 'og:image', content: `${site}/og-default.png` } }`
entry — this exact pattern is shown in Starlight's own production config pulled
via context7. Per-page overrides use the same `head` frontmatter field on
individual pages to replace it.

## Recommendations summary

1. Pin `astro@^7.3`, `@astrojs/starlight@^0.41`, `asciinema-player@^3.17`, Node 24 LTS.
2. `build.format: 'file'` + `trailingSlash: 'never'`, no `base` — the one Starlight
   bug requiring `base` doesn't apply; Pagefind's trailing-slash handling is
   already built into `Search.astro`. Confirm at implementation time by reading
   that file at the pinned version.
3. Heading ids: `remark-heading-id` at the remark stage, chained before Astro's
   own `rehypeHeadingIds` pass, then `rehype-autolink-headings`. Do not reach for
   a rehype-only custom-id plugin — it loses the ordering race.
4. Relative links: `astro-rehype-relative-markdown-links` (rehype plugin) +
   `starlight-links-validator` in CI as the safety net, over a one-time hand
   rewrite of source links.
5. Landing page: `src/pages/index.astro` wrapped in `StarlightPage` (not
   `index.mdx` in the content collection) — avoids a real `base`-in-hero-actions
   bug and matches the ADR.
6. Dark-only: no config flag exists; override `ThemeProvider`/`ThemeSelect`
   components — this is the only supported mechanism, not a workaround.
7. asciinema-player: one global `head`-injected script scanning `[data-cast]`
   elements + `IntersectionObserver` lazy-init. Skip third-party wrapper
   components.
8. GitHub Pages: plain `npm ci && npm run build` + `actions/upload-pages-artifact`
   + `actions/deploy-pages`, all SHA-pinned, over `withastro/action`, to match this
   repo's existing CI pinning convention. `fetch-depth: 0` required for
   `lastUpdated`.

## Riskiest items (unresolved / needs implementation-time verification)

1. **Pagefind + `trailingSlash: 'never'` mitigation is inferred from source, not
   independently tested against build.format `'file'`** — `Search.astro`'s
   `data-strip-trailing-slash` handling was written against the general
   `trailingSlash` config, and I did not find a test or changelog entry pinning it
   specifically to the `file`+`never` combination. Verify with an actual local
   build + search-result click-through before treating this as closed.
2. **Canonical `<link>` tag emission by Starlight is unconfirmed** — description/
   title/OG meta are documented as automatic, but canonical-tag auto-emission
   was not found in the docs pulled this pass. If missing, add it manually via
   the global `head` array using `Astro.url` per page (small, but currently
   unbudgeted work if the assumption is wrong).
