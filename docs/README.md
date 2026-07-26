# Grimoire Documentation

The user-facing documentation site, built with [mdBook][mdbook].

- Source pages live in [`src/`](./src/); the table of contents is
  [`src/SUMMARY.md`](./src/SUMMARY.md).
- Site configuration is [`book.toml`](./book.toml).
- CI builds the book and publishes it to GitHub Pages on every push to `main`
  (see [`.github/workflows/docs.yml`](../.github/workflows/docs.yml)).

Build and preview locally:

```sh
cargo install mdbook --version 0.5.3
task docs:serve
```

The version is pinned here for the same reason it is pinned in CI:
`theme/index.hbs` vendors mdBook 0.5.3's stock template in its non-landing
branch. Raising it means re-copying that template from `mdbook init --theme`
and re-diffing — see the comment on the pin in
[`.github/workflows/docs.yml`](../.github/workflows/docs.yml).

Prefer the task over a bare `mdbook` call: it regenerates the JSON Schemas
under `src/schemas/` from grim's parse structs first, so a preview never
serves schemas that disagree with the binary. `task docs:build` additionally
runs [`seo.py`](./seo.py) over the built site; `docs:serve` deliberately does
not, so a local preview carries no canonical or Open Graph tags.

Writing conventions live in `.claude/rules/docs-style.md`.

[mdbook]: https://rust-lang.github.io/mdBook/
