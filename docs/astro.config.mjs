// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';
import starlight from '@astrojs/starlight';
import { rehypeHeadingIds } from '@astrojs/markdown-remark';
import remarkHeadingId from 'remark-heading-id';
import rehypeAutolinkHeadings from 'rehype-autolink-headings';
import astroRehypeRelativeMarkdownLinks from 'astro-rehype-relative-markdown-links';
import starlightLinksValidator from 'starlight-links-validator';

/**
 * Append `.html` to the internal links the rewrite plugin leaves extensionless.
 *
 * `astro-rehype-relative-markdown-links` resolves `./page.md#frag` against
 * Astro's *route* manifest, and a route has no extension — it has no
 * `build.format` option and its source never mentions `.html`. Under
 * `format: 'file'` that leaves 526 body links pointing at `/page` while the
 * emitted file is `/page.html`, and while Starlight's own sidebar, every
 * `rel="canonical"`, and every shipped deep link in a released binary all say
 * `/page.html`. C-002 requires every internal href to end in `.html`; this
 * closes the gap without touching a page body.
 *
 * Runs LAST, after the rewrite plugin, so it only ever sees final hrefs —
 * C-004's declared order is unchanged, this is one appended stage.
 */
function rehypeHtmlSuffix() {
  // Any trailing `.ext` means the target is already a file — an `.html` page
  // the rewrite plugin got right, or a non-page asset (`/og-card.png`,
  // `/schemas/grimoire-config.schema.json`). Both are left untouched.
  const HAS_EXTENSION = /\.[A-Za-z0-9]+$/;
  // Build-output routes. Every file under them carries an extension today, so
  // HAS_EXTENSION already covers them — this is cheap insurance against a
  // future extensionless asset route, not a fix for anything observed.
  const BUILD_ROUTE = /^\/(?:_astro|pagefind)\//;
  const visit = (node) => {
    const href = node.tagName === 'a' ? node.properties?.href : undefined;
    // Root-relative only. A scheme (`https:`, `mailto:`) and a bare fragment
    // (`#frag`) both fail startsWith('/'); protocol-relative `//host/path`
    // passes it and must be excluded by hand.
    if (
      typeof href === 'string' &&
      href.startsWith('/') &&
      !href.startsWith('//') &&
      !BUILD_ROUTE.test(href)
    ) {
      // ponytail: parse against a dummy origin rather than splitting by hand —
      // URL separates path from query and hash correctly, and a hand-rolled
      // split is exactly how `.html` ends up inside the fragment.
      const url = new URL(href, 'https://grimoire.rs');
      const path = url.pathname;
      if (path !== '/' && !path.endsWith('/') && !HAS_EXTENSION.test(path)) {
        node.properties.href = `${path}.html${url.search}${url.hash}`;
      }
    }
    for (const child of node.children ?? []) visit(child);
  };
  return (tree) => visit(tree);
}

export default defineConfig({
  // C-002: no `base` key — withastro/starlight#2383 only fires when `base` is
  // a subpath. `format: 'file'` + `trailingSlash: 'never'` keep the shipped
  // `/page.html` URLs, and `outDir` stays at its default.
  site: 'https://grimoire.rs',
  build: { format: 'file' },
  trailingSlash: 'never',

  // C-004: the ORDER of this chain is the contract — it keeps 289 shipped
  // heading anchors alive. The remark stage runs first so `remark-heading-id`
  // strips the `{#custom-id}` brace suffix out of the heading text before any
  // rehype pass can slugify the braces into the id. Then `rehypeHeadingIds`
  // must come before `rehypeAutolinkHeadings`, because autolink only wraps a
  // heading that already carries an `id` — reorder these two and every anchor
  // silently disappears.
  markdown: {
    remarkPlugins: [remarkHeadingId],
    rehypePlugins: [
      rehypeHeadingIds,
      [rehypeAutolinkHeadings, { behavior: 'wrap' }],
      // `collectionBase: false` because Starlight serves the `docs`
      // collection at the site root: the default `'name'` prefixes every
      // rewritten link with `/docs/`, which resolves nowhere.
      [astroRehypeRelativeMarkdownLinks, { collectionBase: false }],
      rehypeHtmlSuffix,
    ],
  },

  integrations: [
    starlight({
      title: 'Grimoire',
      favicon: '/favicon.svg',
      customCss: ['./src/styles/theme.css'],
      editLink: {
        baseUrl: 'https://github.com/grimoire-rs/grimoire/edit/main/docs/',
      },
      // C-020: `TwoColumnContent` appends `SiteFooter.astro` — the five site
      // links seo.py injected into every chapter page — as a sibling of
      // `<main>`, which is the only override slot outside it and therefore the
      // only place a `contentinfo` landmark can go. That component's own
      // comment carries the reasoning. The other three make the site dark-only
      // and rebuild the nav bar: overriding `ThemeProvider` and `ThemeSelect`
      // is the only supported way to drop the theme picker — Starlight has no
      // dark-only config flag — and `Header` re-lays the bar as brand / search
      // / nav.
      components: {
        TwoColumnContent: './src/components/TwoColumnContent.astro',
        Header: './src/components/Header.astro',
        ThemeProvider: './src/components/ThemeProvider.astro',
        ThemeSelect: './src/components/ThemeSelect.astro',
      },
      // C-011
      plugins: [starlightLinksValidator()],
      head: [
        {
          tag: 'meta',
          attrs: {
            property: 'og:image',
            content: 'https://grimoire.rs/og-card.png',
          },
        },
        // The PNG favicon `index.hbs` linked alongside the SVG. Starlight's
        // `favicon` key takes one file, and `/favicon.png` is a shipped static
        // path — the fallback for a client that will not take an SVG icon.
        {
          tag: 'link',
          attrs: { rel: 'icon', href: '/favicon.png', type: 'image/png' },
        },
        // C-024: vendored asciinema player, served from docs/public/. The
        // stylesheet loads here so a mounted player is styled on its first
        // paint; the 181 KB player bundle does NOT — `casts.js` injects it on
        // the first `[data-cast]` intersection, so a page with no embed pays
        // nothing for it.
        {
          tag: 'link',
          attrs: { rel: 'stylesheet', href: '/asciinema-player.css' },
        },
        {
          tag: 'script',
          attrs: { src: '/casts.js', defer: true },
        },
      ],
      sidebar: [
        {
          label: 'Getting started',
          items: [
            { slug: 'introduction', label: 'Introduction' },
            { slug: 'installation', label: 'Installation' },
            { slug: 'quickstart', label: 'Quick Start' },
            { slug: 'browse', label: 'Browse the index' },
            { slug: 'first-skill', label: 'Your first skill' },
            { slug: 'concepts', label: 'Concepts' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { slug: 'guides/scopes-and-clients', label: 'Scopes and clients' },
            { slug: 'guides/shared-skills', label: 'Shared skills' },
            { slug: 'guides/lifecycle', label: 'Install, update, status' },
            { slug: 'guides/inspect', label: 'Inspect before you install' },
            { slug: 'guides/versioning', label: 'Versioning' },
            {
              slug: 'guides/mcp-everywhere',
              label: 'MCP servers everywhere',
            },
            { slug: 'clients', label: 'Client Compatibility' },
          ],
        },
        {
          label: 'Teams and automation',
          items: [
            { slug: 'tutorials/own-index', label: 'Publish to your own index' },
            { slug: 'guides/team-ci', label: 'Team and CI' },
            { slug: 'guides/registries', label: 'Company registries' },
            {
              slug: 'guides/catalog-best-practices',
              label: 'Catalog layout and naming',
            },
            { slug: 'publishing', label: 'Publishing Skills and Rules' },
            { slug: 'ci', label: 'Release automation' },
            { slug: 'hosting-an-index', label: 'Host Your Own Index' },
            { slug: 'self-hosted-gitlab', label: 'Self-Hosted GitLab Setup' },
            { slug: 'authentication', label: 'Authentication' },
            { slug: 'ratings', label: 'Artifact Ratings' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { slug: 'commands', label: 'Command Reference' },
            { slug: 'configuration', label: 'Configuration' },
            { slug: 'artifacts', label: 'Artifact formats' },
            { slug: 'agents', label: 'Agent Artifacts' },
            { slug: 'mcp-servers', label: 'MCP Server Artifacts' },
            { slug: 'vendor-metadata', label: 'Vendor-Specific Metadata' },
            { slug: 'package-index', label: 'The Package Index' },
            { slug: 'json-interface', label: 'The JSON Interface' },
            { slug: 'stability', label: 'Stability and Versioning' },
            { slug: 'upgrading', label: 'Upgrading' },
          ],
        },
      ],
    }),
    sitemap({
      // C-009: `seo.py` walked every built *.html, so its sitemap carried the
      // two hand-written pages too. This integration only sees Astro routes,
      // and both live in `docs/public/`. `check_urls.py::check_sitemap` gates
      // the loss.
      customPages: [
        'https://grimoire.rs/start.html',
        'https://grimoire.rs/privacy.html',
      ],
      // C-009: @astrojs/sitemap emits extensionless URLs under
      // `build.format: 'file'` — withastro/astro#15526 (closed, not planned).
      serialize(item) {
        const url = new URL(item.url);
        const last = url.pathname.split('/').pop();
        if (last && !last.includes('.')) {
          url.pathname += '.html';
          item.url = url.href;
        }
        return item;
      },
    }),
  ],
});
