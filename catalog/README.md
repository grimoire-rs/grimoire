# Catalog — First-Party Grimoire Packages

Grim-publishable AI-config packages, dogfooding grim's own packaging:
authored here, validated by `grim build` in CI, published to
`ghcr.io/grimoire-rs` and announced to the public package index
(`https://index.grimoire.rs`) via `grim publish --announce`.

## Layout

```
catalog/
├── publish.toml        # grim publish manifest: registry + catalog-wide version + package tables
├── taskfile.yml        # catalog: subsystem tasks (verify, release)
├── skills/<name>/      # one dir per skill package (SKILL.md + references/)
├── bundles/<name>.toml # one file per bundle package
├── mcp/<name>.toml     # one file per MCP server descriptor package
├── rules/<name>.md     # (when the first rule package lands)
└── agents/<name>.md    # (when the first agent package lands)
```

Skill internals follow the [agentskills.io specification] best practices:
supporting docs in `references/`, executable helpers in `scripts/`, static
files in `assets/`. The root `SKILL.md` is a short index/bootstrap; deep
knowledge lives in `references/` files loaded on demand. Every skill
carries `references/updating.md` — the maintainer re-research protocol
(procedure, durable search terms, canonical links).

## Content drift tiers

Declared per file; applied at authoring and review time:

| Tier | Content | Policy |
|------|---------|--------|
| 1 — inline | ADR-backed invariants (artifact kinds, name rules, metadata-location asymmetry, projection classes, exit-code classes) | State freely; survives minor releases |
| 2 — summarize + verify | Command flags, lifecycle behaviors | Narrative only + "confirm with `grim <cmd> --help`"; never reproduce flag tables |
| 3 — link only | Vendor key registries, exact limits, full command reference | Link the [docs site] anchors; never inline |

The grim-* skills open with a verify-before-acting protocol: on conflict
between skill content and live `--help` output, trust `--help`.

## Versioning

Every catalog package publishes at **grim's own release version**. Entries
in `publish.toml` omit their `version` and inherit the top-level one; the
release CI passes `--version <git tag>` (the `v` prefix is stripped by
grim), so the published catalog always matches the binary that shipped it.
The top-level `version` in `publish.toml` is the fallback for local/manual
runs — keep it at the next planned release. No catalog-specific git tags
(`cliff.toml`'s unanchored `tag_pattern` would pick them up and corrupt
`--bumped-version`).

Content changes therefore need **no per-package version bump** — they ride
out with the next grim release automatically (skip-existing pushes every
package whose version is new).

Registry refs are kind-segmented: `ghcr.io/grimoire-rs/skills/<name>:<version>`,
`ghcr.io/grimoire-rs/bundles/<name>:<version>` (per-entry `repository`
overrides in `publish.toml` set the `skills/`/`bundles/` segment — see
[Batch publishing with a manifest][batch-publish]). Semver releases
cascade (`1.2.3` also moves `1.2`, `1`, `latest`). Bundle members
reference the floating major tag (`:0` while on the 0.x line, relative to
the bundle's own deployment — `../skills/<name>:0`) and bundles publish
without `--pin`, so skill patches reach bundle consumers via plain
`grim update`.

## Local loop

```sh
task catalog:verify                       # grim build every package (builds grim if stale)
grim login ghcr.io -u <user>              # once, interactive
task catalog:release -- --dry-run         # preview full publish plan, zero writes
task catalog:release                      # publish everything per publish.toml
task catalog:release -- --only grim-usage    # publish one package by hand
task catalog:release -- --version canary      # ad-hoc channel tag (no cascade), manifest untouched
task catalog:release -- --announce            # publish, then announce to the package index
```

Semver comes from the release tag in CI (`--version`) or the top-level
`version` in `publish.toml` locally — the repo records exactly what was
published. `--version` is the single version source: a semver value
cascades, a non-semver value (`canary`) is a movable channel tag applied to
every entry. A channel obeys the same skip-existing / `--force` rule as a
semver release.

CI publishes two ways, both via the `publish-catalog.yml` workflow (pushes
to GHCR, then announces to the public package index): the manually
dispatched `Publish Catalog` workflow, and a cargo-dist post-announce job
on every grim release — each release publishes the whole catalog at the
release's version (skip-existing makes re-runs idempotent). Skills publish
before bundles so bundle members always resolve. Never auto-publish on
plain pushes to main.

## Keeping content honest

- `task catalog:verify` runs in CI on every PR — the real parser is the
  schema gate.
- When `docs/src/{artifacts,publishing,vendor-metadata,commands,package-index}.md`
  or `src/command/**` or `src/mcp/**` change, review `catalog/skills/grim-usage`
  and `catalog/skills/grim-authoring` for drift (each package's
  `references/updating.md` describes the re-research procedure).
- Hard numbers (vendor limits, activation rates) drift fastest — re-verify
  against the sources in `references/updating.md` before trusting.

[agentskills.io specification]: https://agentskills.io/specification
[docs site]: https://grimoire.rs/
[batch-publish]: https://grimoire.rs/publishing.html#batch-publish
