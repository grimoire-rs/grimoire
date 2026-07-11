# ADR: Repository description companion v2 — publish.toml integration + fetch read surface

## Metadata

**Status:** Accepted
**Date:** 2026-07-12
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI substrate, existing tar/fetch machinery, no new crates)
**Domain Tags:** api, integration
**Supersedes:** the v1 `desc` surface shipped on `feat/vscode-extension-api`
(commits `706b307`, `bcee7ba` — unmerged PR #33, so this is a clean rework,
no compat shims per pre-1.0 policy)

## Context

The VS Code extension shows a details tab (README, logo, CHANGELOG,
metadata) for **every** artifact kind. The in-tree README channel only
reaches tar-backed kinds — mcp and bundle publish a single JSON layer with
no file tree. v1 added a repository-level description companion: a tar
layer at the reserved `__grimoire` tag, marked `com.grimoire.kind: desc`.

v1 review surfaced five problems:

1. **Magic tag leaks to callers** — the extension hardcodes
   `repo:__grimoire` refs; an internal convention became a consumer API.
2. **Command sprawl** — `grim desc publish` added a near-collision noun
   beside `grim describe`, and the read path had no command at all.
3. **Publish UX is under-designed** — "pack whatever is in `--dir
   description/`" forces a non-standard repo layout and gives no
   source→packed mapping. README.md was arbitrarily required.
4. **No cheap cache probe** — the extension re-downloads describe + content
   + companion on every details view; it needs a digest-only call to skip
   unchanged downloads.
5. **Read granularity wrong** — the companion is one bundle, but reading it
   took one fetch plus per-file `--path` follow-ups (up to 3 calls).

## Decision Drivers

- Extension latency: warm-cache details view should cost HEAD requests, not
  layer downloads
- Command surface pressure (20+ subcommands): no new top-level nouns
- Internal conventions (`__grimoire`) must not be typed by consumers
- Standard repo layout (`README.md`, `assets/logo.png`) over tool-specific
  directories
- Deterministic, CAS-idempotent publishing (byte-stable republish = no-op)
- GitLab compatibility: no custom `artifactType` on the wire
  (`adr_oci_empty_config_compat.md`)

## Industry Context & Research

**Research basis:** live exploration of `~/dev/ocx` (the `__ocx.desc`
precursor) and `~/dev/grimoire-vscode` (the consumer) — session 2026-07-12.

- **ocx `__ocx.desc`**: OCI manifest with a typed layer *per file*
  (`application/markdown` README layer + logo layer, custom
  `artifactType`, title annotations), merge-on-update publish, repo-level
  title/description/keywords annotations. Rejected pieces: custom
  `artifactType` (GitLab), layer-per-file (second read path, more round
  trips), companion metadata (duplicates grim's per-version manifest
  annotations), merge-on-update (conflicts with deterministic
  whole-assembly publish).
- **VS Code extensions / Cargo / npm**: manifest-declared docs assembly
  (`package.json` + `.vscodeignore`, `readme = "README.md"`, `files`
  allowlist) — the mainstream pattern for "tool assembles docs from
  standard repo layout, manifest overrides".
- **Extension consumption today** (`grimoire-vscode/src/grim.ts`,
  `details.ts`): hardcodes `DESC_TAG = '__grimoire'`, blind-probes the
  companion, then up to two `--path` follow-ups for logo/CHANGELOG; only
  structured error discriminators used are `error.reason` and exit 64.

**Key insight:** the wire format was right; the authoring and read
surfaces around it were ad hoc.

## Considered Options

### Option 1: Keep v1 surface (desc publish + raw tag fetch)

| Pros | Cons |
|------|------|
| Already implemented | Magic tag is the consumer API |
| — | Command sprawl, no read command, README required, no cache probe |

### Option 2: Dedicated noun command (`grim description fetch|publish`)

| Pros | Cons |
|------|------|
| Symmetric read/write, full-word naming | Still a new top-level command |
| Hides the tag | `describe`/`description` confusable in help listing |

### Option 3: Fold into existing commands (chosen)

Write rides `grim publish` (manifest-driven); read rides `grim fetch`
flags; `grim describe` reports presence.

| Pros | Cons |
|------|------|
| Zero new top-level commands | `fetch` gains modes (flag interplay to document) |
| Tag fully internal | Publish behavior change (auto-companion, opt-out) |
| Digest probe composes for artifact AND companion | — |

## Decision Outcome

**Chosen Option:** 3.

### 1. Wire format (unchanged from v1)

Single deterministic tar layer at the reserved `__grimoire` tag,
`com.grimoire.kind: desc` annotation as sole discriminator, `__grimoire.<x>`
family reserved for future companions, internal tags hidden from all
user-facing listings (`is_internal_tag`). Not an `ArtifactKind`.

**Content contract:**
- Well-known packed names: `README.md`, `logo.png` | `logo.svg`,
  `CHANGELOG.md`. Free-form assets (README-referenced images) allowed.
- **All members optional** (v1's README-required gate is dropped); an
  empty companion is a data error (65).
- **No metadata in the companion.** Summary/keywords/license/etc. stay on
  the per-version artifact manifest (read via `grim describe`). One home
  per datum — versioned metadata on the manifest, repo-level docs on the
  companion.

### 2. Write: `publish.toml` is the source of truth

```toml
[description]                # optional; paths relative to publish.toml
readme    = "README.md"
logo      = "assets/logo.png"
changelog = "CHANGELOG.md"
include   = ["docs/img/*.png"]   # extra assets
# publish = false            # top-level kill switch

[skills.grim-usage]
repository = "grimoire-rs/skills/grim-usage"
[skills.grim-usage.description]      # per-entry override (same schema)
readme = "skills/grim-usage/README.md"

[mcp.grim]
repository = "grimoire-rs/mcp/grim"
description = false                  # per-entry opt-out
```

- **Fan-out:** top-level `[description]` publishes to **every** entry's
  repository; a per-entry `description` table overrides; `description =
  false` opts an entry out.
- **Convention fallback:** with no `[description]`, grim probes the
  manifest directory for `README.md`, `CHANGELOG.md`,
  `assets/logo.png|svg`, `logo.png|svg`. Any hit → companion publishes by
  default (opt out with `publish = false`). Source names map to well-known
  packed names, decoupling repo layout from wire layout.
- Companion pushes ride the `grim publish` batch, after the entry's
  artifact pushes. The companion tag is mutable metadata: always re-point;
  unchanged content ⇒ identical digest ⇒ idempotent no-op (not gated by
  skip-existing).
- `grim desc` (the v1 command) is **removed**. Docs-only update = re-run
  `grim publish` (artifacts skip-existing, companion re-points).
  `grim release --desc` is deferred until demand shows (YAGNI).

### 3. Read: `grim fetch` flags — the tag never leaves grim

```
grim fetch <ref> --description             # companion bundle
grim fetch <ref> --description --out <dir> # plain: unpack tree to dir
grim fetch <ref> [--description] --digest-only   # cheap probe, no download
```

- `--description` retargets the reference's repository to the internal
  companion tag. JSON: **all files inline** —
  `{ref, digest, kind: "desc", files: [{path, size, content, encoding?}]}`
  (`encoding: "base64"` for binary, same convention as fetch `--path`).
  Bounded by the existing 8 MiB layer gate. Plain mode requires `--out`
  (a multi-file bundle has no single payload to print).
- `--digest-only` resolves the tag and reports `{ref, digest}` without
  downloading — one HEAD-equivalent. Composes with `--description` to
  probe the companion tag. This is the extension's cache key: one manifest
  digest covers both annotations and content, so a matching digest skips
  `describe` + `fetch` entirely.
- `--path` composes with `--description` through the shared fetch core
  (works, not the documented contract). `--out` is only valid with
  `--description`.
- Missing companion → not-found (79) parity with fetch; offline uncached →
  offline-blocked (81).
- `grim describe` gains `has_description: bool` — derived from the tag
  listing it already fetches, zero extra network. Consumers skip the blind
  probe. (Named `has_description` because `DescribeReport.description`
  already carries the description-text annotation — presence flag must not
  collide with it; existing metadata fields stay unchanged.)
- MCP parity: `grim_fetch` tool args gain `description` / `digest_only`
  (no `--out` — writes stay in `grim_render`); `grim_describe` report
  gains the `has_description` field.

### Consequences

**Positive:**
- Extension details view: 3+ downloads → 1 companion fetch; warm cache →
  2 digest probes, zero downloads.
- Net command surface: −1 top-level command vs v1 (describe stays, desc
  dies, nothing new).
- Standard repo layout publishes docs with zero config.

**Negative:**
- `grim publish` behavior change: conventional files now auto-publish a
  companion (opt-out documented, CHANGELOG entry; pre-1.0).
- `fetch` report becomes tri-shaped (content / description-bundle /
  digest-only) — each mode's JSON documented in `json-interface.md`.
- `publish.toml` JSON Schema changes (`grim schema --kind publish`).

**Risks:**
- Unintended fan-out: a multi-package manifest with a root README stamps
  the same docs on every repo. Mitigation: per-entry override/opt-out is
  first-class; dry-run preview lists planned companion pushes.
- Fetch flag interplay errors (`--out` without `--description`, `--vendor`
  with `--description`) must fail as usage errors (64) with clear wording.
- Replication: the companion is a separate manifest under the reserved
  `__grimoire` tag, so a single-tag mirror (skopeo/oras copying one version
  tag) drops it; only a full-repo sync carries it (documented in
  `publishing.md`). **Deferred idea:** additively emit an OCI Referrers-API
  record (`subject` → the artifact manifest) on capable registries so
  referrers-aware replication tooling carries the companion alongside its
  subject — additive to the tag, never a replacement (tag stays the
  universally-supported read path).

## Implementation Plan

1. [ ] Fetch core: typed fetched-kind (replace `kind: ArtifactKind` +
   `is_description` bool placeholder in `FetchedArtifact`) — independent
   refactor
2. [ ] `[description]` parsing (`PublishManifest`, `PublishEntrySpec` —
   untagged bool|table), conventional probe, in-batch companion publish,
   delete `src/command/desc.rs` + report + CLI wiring
3. [ ] `fetch --description` / `--digest-only` / `--out`, describe
   `has_description` field, MCP parity
4. [ ] Docs (`publishing.md`, `commands.md`, `json-interface.md`,
   `artifacts.md`), catalog skill drift review, acceptance tests
5. [ ] Extension handover: `.claude/artifacts/handover_vscode_description_api.md`

## Links

- [adr_oci_empty_config_compat.md](./adr_oci_empty_config_compat.md) — no
  custom artifactType on the wire
- [adr_fetch_service_extraction.md](./adr_fetch_service_extraction.md) —
  the shared fetch core these flags extend
- [adr_repository_annotation.md](./adr_repository_annotation.md) —
  precedent: metadata lives on manifest annotations
- [handover_vscode_description_api.md](./handover_vscode_description_api.md)
  — consumer migration guide
