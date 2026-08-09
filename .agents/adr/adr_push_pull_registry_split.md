# ADR: Push/pull registry split for publish and release

## Metadata

**Status:** Accepted
**Date:** 2026-07-16
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI distribution substrate unchanged; one new `Identifier` rewrite
      helper — no new crate, no new infrastructure)
**Domain Tags:** api, integration
**Supersedes:** N/A
**Issue:** #39

## Context

`grim publish` knew exactly one registry name — the resolved push value
(`--registry` flag > manifest `registry`) — and used it BOTH as the network
push target AND as the name baked into every piece of descriptive metadata
riding the publish: the `org.opencontainers.image.source` fallback
annotation, pinned bundle member ids (`registry/repo@sha256:…`), announce
pointer references, description-companion targets, and the report `ref`.

Pipelines that push through one endpoint while consumers pull from another
name (a staging registry synced to the public one, an internal push URL
fronted by a read-only mirror) could redirect the push with `--registry`,
but then every baked name carried the push endpoint — wrong for consumers,
and digest-unstable across endpoints.

## Decision

The manifest `registry` stays the **canonical PULL name** baked into every
reference, annotation, and report. A new OPTIONAL knob names the deviating
**network push endpoint** only:

- `publish.toml` gains an optional `push_registry = "host[/prefix]"` field.
- Both `grim publish` and `grim release` gain `--push-registry
  <host[/prefix]>` (flag > manifest — symmetric, documented).
- The `host[/prefix]` shape mirrors the `--registry` flag: the first `/`
  splits the host from an optional repository prefix prepended to every
  pushed repository (`Identifier::with_registry(host, prefix)` is the one
  rewrite primitive; tag and digest are preserved).
- Every network operation targets the push-rewritten identifier: push_blob
  / push_manifest, the skip-existing lookup, the overwrite guard, tag
  cascade moves, pin digest resolution, the description-companion push,
  and the announce metadata read-back — on the skill/rule/agent, bundle,
  and mcp release paths alike.
- Every baked/reported value keeps the pull name: the source-annotation
  fallback, pinned member ids, announce pointer references, the report
  `ref`, and the companion report item.
- Reports gain an additive **always-present** `pushed_to` field (release
  single object + publish per-entry item): the push-side reference
  actually used, `null` when the split is inactive (no
  `skip_serializing_if`, per the src/api additive-field policy).
- Unset knob ⇒ byte-identical behavior (locked by tests).
- A malformed value (empty host, invalid prefix charset) is a DataError 65
  — NOT 64 — matching the existing `--registry` value gates
  (`validate_registry_value` / `validate_repository_path`).

### Pin semantics under the split

Pinned bundle members bake **pull-named** absolute digest-pinned refs
whose digests were resolved via the **push endpoint** — but only for
members on the release target's own (pull) registry; a member on a foreign
registry resolves where it lives, unchanged. This trades the old
resolves-where-pushed guarantee for mirror-correctness: OCI content
addressing makes the pin sound for a true mirror (identical bytes ⇒
identical digests on both names), but if the pull name does not serve
identical content the pin fails at install. Documented in
`docs/src/publishing.md#batch-publish-push-registry`.

### `--registry` flag interplay

`--registry` keeps its shipped meaning: it overrides the manifest
`registry` — i.e. the PULL name (and, with a path, the enforced
namespace). The push endpoint is exclusively the new knob's job. Anyone
who today uses `--registry` to redirect pushes and adopts `push_registry`
sees a one-time digest change (the source fallback flips from push to pull
name); release's overwrite guard refuses it without `--force`.

## Consequences

**Positive:**
- Mirror/staging pipelines publish artifacts whose baked identity is the
  name consumers actually resolve.
- Digest stability across push endpoints: identical content publishes the
  same digest regardless of where it is pushed.
- Additive only: no existing manifest, flag, or report consumer changes.

**Negative / Risks:**
- Pins verified on the push endpoint trust the mirror to be
  content-identical (documented trade-off).
- The description companion lands only on the push registry; a single-tag
  mirror copy will not carry `__grimoire` (pre-existing replication
  caveat, cross-linked in the docs).

## Links

- Issue #39
- `docs/src/publishing.md` — "Push vs pull registries"
- `adr_unified_publish_version_cascade.md` — the publish flag surface this
  extends
- `adr_repository_annotation.md` — the source-annotation precedence the
  pull name feeds into
