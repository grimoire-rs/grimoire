# ADR: opt-in git provenance annotations (`--git`)

- Status: accepted
- Date: 2026-06-29
- Issue: #17 "git tracing"

## Context

`grim release`/`publish` mirror authored metadata into OCI manifest
annotations (`src/oci/annotations.rs`). The mapping is **deterministic by
design**: `org.opencontainers.image.created` is deliberately omitted so a
re-release of identical content produces the same manifest digest, keeping
re-release an idempotent no-op (the exact-version overwrite guard in
`release.rs` depends on this). Issue #17 asks to embed git provenance
("ie. Git commit") so a published artifact can be traced back to the source
commit.

## Decision

Add an **opt-in `--git` flag** on `build` / `release` / `publish`. When set,
grim derives provenance from the artifact's git working tree and emits
standard OCI annotations:

- `org.opencontainers.image.revision` — HEAD commit SHA, with a `-dirty`
  suffix when tracked files differ from HEAD (`git describe --dirty`
  convention; untracked files are ignored).
- `org.opencontainers.image.created` — HEAD committer date (strict RFC3339,
  `%cI`). This is the per-commit date, **not** a wall-clock build time, so it
  is deterministic for a given commit.
- `org.opencontainers.image.source` — the `origin` remote normalized to an
  `https://` URL, used only as a **fallback** below an authored `repository`
  metadata value (which still wins). No duplication when `repository` is
  authored.

Provenance is derived once per invocation by shelling out to the `git`
binary (boring tech — no new crate dependency; grim is itself a
git-distributed tool). Derivation lives in `src/oci/git_provenance.rs`;
the remote-URL normalization is a pure, unit-tested function.

### Idempotency

`--git` is **off by default**, so every existing deterministic-annotation
guarantee and test is unchanged. With `--git` on a **clean tree**, the manifest
digest becomes a function of the commit: a re-release from the *same* commit is
still idempotent; a re-release of identical content from a *different* commit
produces a different digest and is refused by the existing overwrite guard
unless `--force`. This is the correct trade-off — the provenance genuinely
changed — and it is opt-in, so no caller is surprised.

On a **dirty tree** the guarantee is narrower. The digest is then a function of
*(commit, working-tree-content)*: the layer carries the uncommitted bytes, so
two different uncommitted states already produce different manifest digests
through the layer. The provenance annotations, however, are intentionally
**non-unique** — both `revision` (`<sha>-dirty`) and `created` (the commit
date) are identical across *any* uncommitted state on the same commit. The
`-dirty` marker says "this was built from working-tree changes on top of
`<sha>`", not "this exact set of changes"; pinning a dirty build to a single
reproducible source is out of scope (commit first for that). The idempotency
claim above is therefore scoped to a clean tree.

### Failure mode

`--git` in a non-git directory, or with no `git` on PATH, is a hard error
(DataError, exit 65, attributed to the artifact path) rather than a silent
skip: the user explicitly asked for provenance.

## Read / display side

The two new annotations round-trip through the catalog read path
(`CatalogEntry.revision` / `.created`, additive optional fields — no
`CatalogVersion` bump, same as the earlier `repository_url`/`deprecated`
additions) into `CatalogRow` → `TuiRow`, and are surfaced as `Revision:` /
`Created:` rows in the TUI detail pane and as `revision`/`created` fields in
`grim search --format json`.

## Alternatives considered

- **Automatic when in a git repo** — rejected: silently changes every
  publisher's manifest digests and breaks the idempotent-re-release contract
  without consent.
- **Generic `--label key=value` passthrough** — rejected for v1 (YAGNI):
  issue #17 is specifically git provenance; a generic mechanism is a larger
  surface that can be added later if asked.
- **A pure-Rust git crate (`gix`)** — rejected: a heavy new dependency and an
  innovation token spent for what three `git` subprocess calls do.
