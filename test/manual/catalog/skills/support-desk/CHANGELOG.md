# Changelog

All notable changes to `support-desk`, the manual rig's annotation showcase.
It exists so the TUI's **Changelog** tab has something to render — a rig with
only a README leaves that tab greyed out and untestable.

## [1.1.0] - 2026-08-27

### Added

- A `CHANGELOG.md`, published through `[description].changelog` in
  `catalog/publish.toml`.
- Support channels (`issues`, `chat`, `contact`, `security`) on the companion
  manifest.

### Changed

- `authors` now comes from the skill's own frontmatter rather than the
  catalog-wide `[metadata]` fallback — the precedence rung made visible.

## [1.0.0] - 2026-08-26

### Added

- Initial release. Every optional metadata field grim can publish, authored in
  one artifact:

  ```sh
  grim describe localhost:5050/grimoire/skills/support-desk --format json
  ```

- A `README.md` on the description companion.

---

Nothing here is real history — it is a fixture, shaped to exercise the
renderer: headings at two levels, nested bullets, a fenced block, a thematic
break, and a [link](https://grimoire.rs/publishing.html#description-companion).
