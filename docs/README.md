# Grimoire Documentation

The user-facing documentation site, built with [mdBook][mdbook].

- Source pages live in [`src/`](./src/); the table of contents is
  [`src/SUMMARY.md`](./src/SUMMARY.md).
- Site configuration is [`book.toml`](./book.toml).
- CI builds the book and publishes it to GitHub Pages on every push to `main`
  (see [`.github/workflows/docs.yml`](../.github/workflows/docs.yml)).

Build and preview locally:

```sh
cargo install mdbook
mdbook serve docs --open
```

Writing conventions live in `.claude/rules/docs-style.md`.

[mdbook]: https://rust-lang.github.io/mdBook/
