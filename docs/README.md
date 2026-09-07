<!-- doc_type: readme -->
# Grimoire Documentation

The user-facing documentation site, built with [Astro Starlight][starlight].

- Pages live in [`src/content/docs/`](./src/content/docs/) — the `docs`
  collection declared in [`src/content.config.ts`](./src/content.config.ts).
  `title` and `description` frontmatter are both mandatory: a page without a
  `description` fails the build.
- The landing page is [`src/pages/index.astro`](./src/pages/index.astro).
- Site config, sidebar groups and the markdown plugin chain are
  [`astro.config.mjs`](./astro.config.mjs).
- [`public/`](./public/) is copied verbatim into the site root: the vendored
  asciinema player, `demo.cast`, `start.html`, `privacy.html`, the favicons,
  `og-card.png`, `robots.txt` and the install scripts.
- Built output lands in `dist/` (gitignored).
- CI builds the site and publishes it to GitHub Pages on a push to `main` that
  touches one of the paths it watches — `docs/`, `catalog/`, `src/`, the Cargo
  manifests and three taskfiles
  (see [`.github/workflows/docs.yml`](../.github/workflows/docs.yml)).

Build and preview locally:

```sh
task docs:serve    # dev server
task docs:build    # the CI Pages artifact, into dist/
task docs:check    # docs subsystem gate (alias: docs:verify)
```

These tasks need Node 24 ([`package.json`](./package.json) `engines`); `task
verify` does not.

Prefer the tasks over a bare `npm` call: each regenerates the JSON Schemas
under `public/schemas/` from grim's parse structs first, so a preview never
serves schemas that disagree with the binary. Those schemas are gitignored and
rebuilt on every run. `docs:check` builds, then runs the URL-contract check,
the docs-quality declaration check, `npm audit --audit-level=high`, and any
test script `package.json` declares.

The landing page's terminal cast is [`public/demo.cast`](./public/demo.cast).
It is committed, and re-recorded by `task test:demo`.

Writing conventions live in `.claude/rules/docs-style.md`.

[starlight]: https://starlight.astro.build/
