# ADR: Compressed artifact layers (reader-first, opt-in gzip)

## Metadata

**Status:** Accepted
**Date:** 2026-10-22
**Deciders:** Michael Herwig (maintainer)
**Beads Issue:** N/A
**Related PRD:** N/A — input dossier `.agents/discussions/compressed-artifact-layers.md` (ratified 2026-10-21)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI substrate unchanged; gzip via the existing `flate2 = "1"` dependency; no new crate)
**Domain Tags:** api, integration, security
**Supersedes:** N/A
**Superseded By:** N/A

## Context

Every published skill, rule, agent and hook is one uncompressed tar layer, `application/vnd.grimoire.artifact.layer.v1.tar`.
Skills compress about 3×, tiny rule and agent tars about 6×.
No grim reads the layer `media_type`: every version parses the blob as raw tar.
So a gzip layer makes an old grim fail with exit 65 and a misleading tar checksum message.

The overwrite guard compares manifest digests only.
Any byte change refuses a re-release with `TagExists`, including compressor drift or a compression flip.
Current size caps (`INSTALL_LAYER_SIZE_LIMIT` 512 MiB, `FETCH_BLOB_SIZE_LIMIT` 8 MiB, `CappedSink`) bound stored bytes only.
Nothing bounds decompressed output.

Compression support is an **owner constraint**, ratified in the dossier, not an outcome of the matrix below.
No quota or egress figure drives it; the measured gain is 174 KB → 58 KB (`grim-usage`) and 2.5 KB → 422 B (`hello-world`).
The owner also requires that compression is visible on the JSON surface, readable by the VS Code extension (`grimoire-vscode`) and the indexer (`@grimoire-rs/indexer`).

## Decision Drivers

- **Principle 9.** The OCI wire format is frozen (`docs/src/content/docs/stability.md:40`); evolution is additive only. Default output stays byte-identical, and nothing that installs today may stop installing.
- **Old-client behaviour.** grim ≤ 0.14.3 must keep working on every artifact published without opt-in. On an opted-in artifact an honest publisher's layer makes it fail loudly and write nothing.
- **T1, malicious publisher** (`arch-threat-model.md`). A compressed layer opens a decompression-bomb surface the stored-byte caps do not cover.
- **Registry evidence.** The 2026-10-21 live probe pushed `custom-gzip` and `oci-gzip` tags to GHCR and GitLab. Both accepted all three with 201, stored manifests byte-for-byte and kept the layer annotation.
- **Artifactory is unprobed.** It is the company target and needs credentials not available in that session.
- **No new dependency.** `flate2` is already linked for the index.

## Industry Context & Research

**Research artifacts:**
[`research_compressed_layers_prior_art.md`](../research/research_compressed_layers_prior_art.md),
[`research_compressed_layers_recon.md`](../research/research_compressed_layers_recon.md),
[`research_artifactory_oci_permissions.md`](../research/research_artifactory_oci_permissions.md) (Artifactory overwrite context only).

**Trending approaches:** Docker and containerd shipped zstd **reader-first**; push followed years later.
Helm, Flux and Falco ship their own `…tar+gzip` types from day one.
OCI's own home for uncompressed digests is the config `rootfs.diff_ids`. grim keeps the empty config for GitLab (`adr_oci_empty_config_compat.md`), so the fact rides on a layer annotation instead (eStargz does the same; not re-verified).

**Key insight:** no surveyed tool added compression to a non-image artifact with old-client compatibility.
The only proven path is to ship the reader first and make the writer opt-in.
The OCI image-spec forbids erroring on unknown annotations, so no annotation can gate an old client.

## Considered Options

### Option 1: Reader-first, opt-in custom `+gzip`, per-layer uncompressed-digest annotation (chosen)

New grim reads tar and gzip. `--compression gzip` publishes `application/vnd.grimoire.artifact.layer.v1.tar+gzip`.
Compressed layers carry `com.grimoire.uncompressed.digest`; the guard compares on it.

| Pros | Cons |
|------|------|
| Default output byte-identical; purely additive | Old grim exits 65 on opted-in artifacts |
| Proven on GHCR and GitLab | Artifactory unprobed |
| No new crate; guard needs no blob download | Custom `+gzip` is not on GitLab's documented allowlist (accepted in practice) |

### Option 2: zstd
2–10% smaller than gzip; needs a new crate with a C build. Nexus fails validation on zstd layers.

### Option 3: zip layers
9–22% larger than tar.gz. Per-entry timestamps hurt determinism. Not an OCI layer format. `grim export --zip` already covers zip.

### Option 4: Dual publish (tar and tar+gzip side by side)
Needs an image index or referrers. grim rejects image indexes. Fedora reports no direct dual-compression push. Doubles storage.

### Option 5: Manifest-level uncompressed-digest annotation
One key cannot describe several layers. Stamping it on every manifest changes the digest of every unchanged re-release after upgrade, yielding a false `TagExists`.

### Option 6: Download-and-compare guard
The guard pulls and decodes the existing blob to compare. One capped blob GET on a mismatch; no annotation. See Review decisions.

### Option 7: Standard OCI layer types for the push
`application/vnd.oci.image.layer.v1.tar(+gzip)` sit on GitLab's allowlist.
Switching the *uncompressed* push changes every manifest digest for unchanged content. Readers accept both types now, so a later switch stays possible.

### Trade-off matrix

Scores 1 (worst) to 5 (best). Weighted sum. The status quo would score 4.60 under these weights; it is excluded by the owner constraint, not by the matrix.

| Criterion (weight) | 1 Chosen | 2 zstd | 3 zip | 4 Dual | 5 Manifest annot. | 6 Download guard | 7 OCI types |
|---|---|---|---|---|---|---|---|
| Old-client safety (0.30) | 4 | 4 | 4 | 5 | 4 | 4 | 4 |
| Additive wire / Principle 9 (0.25) | 5 | 5 | 4 | 2 | 3 | 5 | 3 |
| Registry compatibility (0.15) | 4 | 3 | 3 | 2 | 4 | 4 | 5 |
| Size win (0.10) | 4 | 5 | 3 | 3 | 4 | 4 | 4 |
| Complexity / deps (0.20) | 4 | 2 | 2 | 1 | 4 | 3 | 4 |
| **Weighted** | **4.25** | 3.80 | 3.35 | 2.80 | 3.75 | 4.05 | 3.90 |

## Decision Outcome

**Chosen Option:** Option 1.

**Rationale:** it keeps default output byte-identical, needs no new crate, and makes the release guard compression-agnostic without downloading blobs.

### D1. Readers first

Classification is by exact media-type string (no parameters, no case folding, no content sniffing), and only on the **tar read path** (skill, rule, agent, hook, description companion):

| Layer media type | Encoding |
|---|---|
| `application/vnd.grimoire.artifact.layer.v1.tar+gzip` | gzip tar |
| `application/vnd.oci.image.layer.v1.tar+gzip` | gzip tar |
| `application/vnd.docker.image.rootfs.diff.tar.gzip` | gzip tar |
| any type ending `+zstd` | `UnsupportedMediaType`, exit 65 |
| anything else | raw tar, **as today** |

Unknown types keep today's raw-tar path: refusing them would break artifacts that install today (Principle 9).
The bundle and mcp layers keep their own read paths and are never decoded; their caps and parsers apply unchanged.
Kind stays on `com.grimoire.kind`; the layer type never carries it. The push still sets no `artifactType`.

Gzip framing is strict. Exactly one member; trailing bytes after it are `Corrupt`, so grim and multistream readers (Go `gzip.Reader`, Docker) see the same bytes.
A header with FEXTRA, FNAME or FCOMMENT is `Corrupt`; grim's writer never sets them, and refusing them closes the easy tar/gzip polyglot for new grim.

### D2. Opt-in writer

`grim release` gains `--compression <none|gzip>`, a clap `ValueEnum`, default `none`.
`grim publish` gains the same flag as `Option<Compression>` with **no clap default**, plus an optional top-level `publish.toml` `compression` key, resolved `flag.or(manifest).unwrap_or_default()`.
No short flag: `-c` would collide by convention with the global `--config`.
A future algorithm is a new enum literal: additive.

The setting is **ignored for bundle and mcp** entries, like `--pin` is for skills and rules (`release.rs:68-72`): the top-level key applies batch-wide, so it cannot be an error.
`--compression gzip` prints one stderr line: consumers need grim ≥ `<R>`.

The writer gzips the already-deterministic packed tar with `flate2`: level 9, header mtime 0, no name, no extra, no comment.
Byte drift across flate2 backends or versions is tolerated by D5, not prevented.

### D3. Scope by kind

| Layer | In scope | Why |
|---|---|---|
| skill, rule, agent, hook (tar via `release.rs::run`) | **yes** | One shared push path; hooks deliberately take it (`release.rs:233-245`) |
| bundle, mcp (JSON layers) | no; setting ignored | 64–512 KiB caps; tiny JSON; separate push and read paths |
| description companion (`__grimoire`) | no (push) | Separate path in `oci/description.rs`; old grim's `describe` and TUI keep reading it |

Readers still decode a gzip companion, because the decode sits in the shared `fetch_artifact`. Pushing one is a later, additive change.

### D4. Uncompressed-digest annotation

Compressed layer descriptors carry `com.grimoire.uncompressed.digest: sha256:<hex>`, the digest of the uncompressed tar.
It is written **only** on compressed layers; stamping it everywhere would change every unchanged re-release's manifest digest.

The annotation is a publisher claim. The lock-pinned manifest and blob digests remain the integrity check.
Readers check it on gzip layers only. A `sha256:` value must be 64 lowercase hex, else exit 65; a mismatch with the decoded bytes exits 65.
An unknown algorithm is skipped with a debug log, so a future grim may write another one.
No uncompressed-size annotation (YAGNI): the decode cap below bounds output.

### D5. Release guard

Order, matching `release.rs` today: `--skip-existing` (returns before packing or pushing, fetches nothing) → `--dry-run` → push → `if !force { guard }` → move tags.
`--force` skips the guard, as today, so it moves the tag to the new manifest. It never adopts; it is the only way to convert an existing version.

Guard:

1. Same manifest digest at the version tag: no-op, as `publishing.md#git-idempotent` documents.
2. Different digest: fetch the **existing manifest only**, by digest, never a blob. A fetch error propagates with its own exit code (69, 80 or 81); a missing manifest is `TagExists`. Neither ever adopts or moves a tag.
3. Compare the normalized **content key**:
   - config media type, with `None` read as `application/vnd.oci.empty.v1+json` (built manifests carry `None`, fetched ones `Some(empty)`);
   - `artifact_type`, exactly (legacy readers infer kind from it first);
   - manifest annotations, exactly;
   - layer count, then per layer in order, the layer's **content key**, which depends on its encoding:

     | Layer encoding | Content key |
     |---|---|
     | raw tar | the descriptor digest |
     | gzip with a well-formed annotation | the annotation (the uncompressed tar digest) |
     | gzip without an annotation | the blob digest, so it equals only the same blob |
     | a layer grim cannot read (`+zstd`) | never equal, to anything |

     An annotation on a raw layer that differs from its descriptor digest makes the manifests unequal.
   - Ignored: the manifest `media_type`, each layer's `media_type` and `size`.
4. Equal: adopt, **for tar kinds only** (skill, rule, agent, hook). The version tag stays on the existing manifest; cascade tags point at it; the report shows its digest with `pushed: true` (publish status `pushed`). One stderr line names the adopted digest.
5. Different, or equal on a bundle or mcp release: `TagExists` (exit 65), as today. Bundle and mcp readers never verify the annotation, so adopting a manifest whose bytes they could not check would trade the digest guarantee for convenience. Their layers are never compressed, so a flip cannot occur there and nothing is lost.

This covers a `--compression` flip, compressor drift, a later switch to OCI layer types, and multi-layer manifests.
The orphaned new manifest on a refused or adopted re-release is existing behaviour (the guard already runs after the push).
`--dry-run` makes no guard read. Its preview digest is the new manifest's, now computed over the exact bytes a real push sends (it was a non-authoritative preview hash before), so a real push of the same content reports the same digest. A real run that adopts reports the existing one. `publishing.md` states this.

### D6. Decompression cap (T1)

No new constant. `decode_layer` takes the **caller's existing cap** and applies it to decoded bytes, so every cap means content bytes:

| Caller | Cap on decoded bytes |
|---|---|
| install, update, add, export, TUI, MCP `grim_render` | the kind cap or `INSTALL_LAYER_SIZE_LIMIT` (512 MiB), as passed today |
| `grim fetch`, MCP `grim_fetch`, description companion | `FETCH_BLOB_SIZE_LIMIT` (8 MiB) |

The effective bound is `min(caller cap, max(64 MiB, 100 x stored bytes))`. The expansion-ratio term (PKG-05, review round 1) exists because the caller cap alone is loose for a small layer: a 512 MiB install cap lets a few-hundred-byte gzip stream decode to half a gigabyte, which is a compression bomb against memory and disk (T1). The 64 MiB floor keeps ordinary small artifacts clear of the ratio, and real tar content compresses far below 100x.

A gzip artifact therefore carries no more content than an uncompressed one, and CLI `fetch` truncation stays unreachable (`fetch.rs:402-406`, `:683-686`).
Decoding streams through `Read::take(limit + 1)`; output past the limit exits 65.
Decode and the annotation hash run in `spawn_blocking`.

**Writer symmetry.** `grim release` / `grim publish` refuse (exit 65, `LayerError::UnreadableAsGzip`) a gzip layer the reader above would refuse: a tar larger than `min(INSTALL_LAYER_SIZE_LIMIT, max(64 MiB, 100 x gzip size))`, e.g. hundreds of MiB of near-identical bytes. The check calls `layer::check_gzip_readable`, built on the reader's `expansion_limit`, before any push and in `--dry-run`; the message suggests `--compression none`. Publishing a version this grim cannot install was the alternative, and was rejected.

### D7. Digests that do not move

- Pull-path blob and manifest digest checks are unchanged; they cover stored bytes.
- The blob store keeps keying on the stored (compressed) digest. Decode happens on every read.
- The path-source and dev-install lock `hash` stays the SHA-256 of the uncompressed packed tar. `grim status` drift does not flip with compression.
- `grim build`'s `layer_digest` stays the uncompressed tar digest. Under gzip it equals the annotation, not the pushed layer digest.

### D8. CLI visibility

`grim describe` (CLI JSON, plain, MCP `grim_describe`) gains an always-present `compression`: `"none"`, `"gzip"` or `null`.
Both consumers already call `describe`, which already fetches the manifest, so the field costs zero requests.
The plain output gains one `compression` row (lowercase, like the other rows); release and publish output is unchanged.

Semantics: `"gzip"` for a gzip-classified single layer. `"none"` for any other single layer the read path accepts, including bundle and mcp.
`null` means this grim cannot read it: a `+zstd` layer, or not exactly one layer.

Not on `grim release`/`grim publish` reports (no consumer; additive later), `grim fetch --digest-only` (its contract is no manifest fetch, `commands.md:1714`) or `grim search` (add on `OciMeta` when the extension wants a badge).

### Quantified Impact

| Metric | Before | After (opted in) | Notes |
|---|---|---|---|
| `grim-usage` layer | 174,080 B | ≈ 58,650 B | gzip -9, prior-art finding 7 |
| `hello-world` layer | 2,560 B | 422 B | 2026-10-21 probe |
| Extra requests on install | 0 | 0 | Decode is local |
| Extra requests on guard mismatch | 0 | 1 manifest GET | Never a blob |
| Peak read memory | blob + unpacked | blob + decoded (≤ caller cap) + unpacked | Install: ≤ 512 MiB stored plus ≤ 512 MiB decoded plus entries; fetch: ≤ 8 MiB each |

### Consequences

**Positive:**
- Opt-in publishers ship 3–6× smaller layers with no impact on anyone else.
- The guard stops refusing equal content after a compressor or type change.
- Compressed layers grim cannot read get a true error instead of a tar checksum message.

**Negative:**
- Consumers on grim ≤ 0.14.3 cannot install opted-in artifacts (exit 65).
- Converting an existing version needs `--force`, which changes its digest and triggers a stale-lock refusal for consumers pinned to it. Docs say: compress new versions only.
- New frozen contracts: three read types plus the `+zstd` refusal, one written type, the annotation key, two enum literals, the `unsupported-layer-media-type` slug, the `publish.toml` key, the flag, and one `describe` field (the `release`/`publish` report field is added, see Review decisions).

**Risks:**
- *Artifactory rejects the type or strips the annotation.* Mitigation: Open question 1; docs keep compression unrecommended there until probed.
- *Adopted manifest with a lying annotation.* Adoption keys on the annotation only for gzip layers. Old grim cannot read gzip and new grim verifies the annotation, so an adopted lie is never installed silently.
- *T1 polyglot on old clients.* A gzip header whose FEXTRA hides a tar can make grim ≤ 0.14.3 install different files than new grim, Docker or the indexer see. "Fails loudly" holds for honest publishers only; the residual closes on upgrade.

## Technical Details

### Architecture (C4)

```
Context:   publisher --grim release/publish--> OCI registry (GHCR | GitLab | Artifactory)
           consumers (grim CLI, VS Code ext, indexer) --grim describe/fetch/install--> registry

Container: grim binary
             ├─ command::release / command::publish   (writer: --compression)
             ├─ oci::access (registry_client)         (descriptor annotations on the wire)
             ├─ oci::layer  [NEW]                     (classify + decode + verify + same_content)
             ├─ install::installer / fetch            (call oci::layer after digest check)
             └─ fetch::DescribeReport                 (compression field; api::describe_report wraps it)

Component (oci::layer):
   classify(media_type) ─► LayerEncoding{Tar, TarGzip} | UnsupportedMediaType
   decode(desc, blob, limit) ─► gunzip (≤ limit, one member) ─► verify annotation ─► tar bytes
```

### API Contract (Rust)

```rust
// src/oci/manifest.rs — additive field
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub digest: Digest,
    pub media_type: String,
    pub size: u64,
    /// `None` serialises to no key (byte-identical default manifests).
    pub annotations: Option<BTreeMap<String, String>>,
}

// src/oci/layer.rs — new module
pub const TAR_GZIP_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.artifact.layer.v1.tar+gzip";
pub const UNCOMPRESSED_DIGEST_ANNOTATION: &str = "com.grimoire.uncompressed.digest";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq,
         clap::ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Compression { #[default] None, Gzip }

pub enum LayerEncoding { Tar, TarGzip }

pub fn classify(media_type: &str) -> Result<LayerEncoding, LayerError>;
/// Describe-side: `None` for a `+zstd` layer or a non-single-layer manifest.
pub fn compression_of(manifest: &OciManifest) -> Option<Compression>;
/// Tar read path only, after the stored-digest check; runs inside `spawn_blocking`.
pub fn decode_layer(desc: &Descriptor, blob: Vec<u8>, limit: u64) -> Result<Vec<u8>, LayerError>;
/// Write path: gzip (level 9, mtime 0, no name/extra/comment) plus the annotated descriptor.
pub fn encode_layer(tar: &[u8], c: Compression) -> (Vec<u8>, Descriptor);
/// Guard: the normalized content key of D5 step 3.
pub fn same_content(existing: &OciManifest, new: &OciManifest) -> bool;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LayerError {
    #[error("unsupported layer media type '{0}'; this grim reads tar and gzip layers")]
    UnsupportedMediaType(String),
    #[error("layer decompresses past the {limit}-byte limit")]
    DecompressedTooLarge { limit: u64 },
    #[error("layer is not valid single-member gzip")]
    Corrupt(#[source] std::io::Error),
    #[error("decompressed layer digest {actual} does not match com.grimoire.uncompressed.digest {expected}")]
    UncompressedDigestMismatch { expected: String, actual: String },
    #[error("malformed com.grimoire.uncompressed.digest '{0}'")]
    MalformedAnnotation(String),
}
```

Registry-sourced strings in `UnsupportedMediaType` and `MalformedAnnotation` are control-character-escaped (precedent `MarketplaceExportReport::file_lines`) and truncated to 128 characters.

Wiring:

- `crate::error::Error` gains `Layer(#[from] LayerError)`. `classify` maps every variant to 65; only `UnsupportedMediaType` carries the new `ErrorReason::UnsupportedLayerMediaType` (`Display` slug `unsupported-layer-media-type`).
  `fetch_verified_layer` (`installer.rs:2344`) returns it through `Error`; `fetch_artifact` (`fetch.rs:303`, `anyhow`) converts through `Error`, never a bare `LayerError`, or it would exit 1 (the trap `fetch.rs:363-374` documents).
- `fetch_verified_layer` and `fetch_artifact` call `decode_layer` after the stored-digest check, on the tar kinds and the companion only, passing the D6 cap.
- `oci_descriptor` (`registry_client.rs:283`) passes `annotations` through; the read conversion (`manifest.rs:117`) keeps them.
- `MemoryRegistry`'s `ManifestKey` (`memory_registry.rs:128`) and `preview_manifest_digest` (`release.rs:671`) include annotations. Missing either silently drops them.
- `guard_existing_version` (`release.rs:644`) takes the new `&OciManifest`; on a digest mismatch it calls `fetch_manifest(repo@existing)` and `same_content`. Bundle and mcp call sites (`:423`, `:501`) pass their manifest unchanged.
- `ReleaseArgs.compression` is forwarded at `publish.rs:1170`. `PublishManifest.compression: Option<Compression>` (`publish.rs:500`).
- `DescribeReport` (`src/fetch.rs`) gains `compression: Option<Compression>`.

### Exit codes

| Condition | Exit | Error reason |
|---|---|---|
| `--compression zstd` (invalid value) | 64 (clap) | — |
| Invalid `compression` in `publish.toml` | 65 (manifest parse, as today) | — |
| `+zstd` layer on the tar read path | 65 | `unsupported-layer-media-type` (new, additive) |
| Decoded output past the caller cap | 65 | — |
| Corrupt gzip, trailing data, or FEXTRA/FNAME/FCOMMENT header | 65 | — |
| Uncompressed digest mismatch or malformed `sha256:` annotation | 65 | — |
| Guard: existing-manifest fetch fails | 69 / 80 / 81 (access error, propagated) | — |
| Guard: different content, or existing manifest missing | 65 (`TagExists`, unchanged) | unchanged |

Every row holds on the install path and on `grim fetch` / MCP.

## Interface impact overview

### Pseudo-code

```
release(entry, c):                               # publish: c = flag ?? manifest ?? none; bundle/mcp ignore c
    if skip_existing and resolve(version): return skipped    # unchanged, nothing packed or pushed
    tar  = validate_and_pack(entry)              # deterministic, unchanged
    hash = sha256(tar)                           # lock/status hash, unchanged
    (blob, layer) = encode_layer(tar, c)         # c=none → tar, no annotation
    manifest = build(layer)
    if dry_run: return preview(manifest)         # no guard read
    push_blob(blob); push_manifest(manifest)
    if !force: target = guard(version, manifest) # may adopt
    move_tags(target)

guard(version, new):
    old = resolve(version) or return new         # resolve's fail-open is unchanged
    if old == digest(new): return new
    existing = fetch_manifest(repo@old)?         # by digest; error → 69/80/81; None → TagExists
    if same_content(existing, new): warn adopt; return old
    fail TagExists (65)

read(ref):                                       # install, fetch, export, TUI, MCP
    manifest = fetch_manifest(pinned)            # digest-verified
    blob     = fetch_blob(layer)                 # stored-digest verified, stored caps
    bytes    = spawn_blocking(decode_layer(layer, blob, caller_cap))  # tar kinds only
    materialize(bytes)                           # unchanged tar reader
```

### JSON

`grim describe --format json`, before and after (abridged):

```json
{ "ref": "ghcr.io/acme/skills/hello-world:1.2.0", "digest": "sha256:…", "kind": "skill", "annotations": {} }
{ "ref": "ghcr.io/acme/skills/hello-world:1.2.0", "digest": "sha256:…", "kind": "skill", "annotations": {}, "compression": "gzip" }
```

### OCI

Manifest of an opted-in skill. Only the layer descriptor differs from today:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "config": { "mediaType": "application/vnd.oci.empty.v1+json", "digest": "sha256:44136fa…", "size": 2 },
  "layers": [{
    "mediaType": "application/vnd.grimoire.artifact.layer.v1.tar+gzip",
    "digest": "sha256:<gzip bytes>",
    "size": 422,
    "annotations": { "com.grimoire.uncompressed.digest": "sha256:<tar bytes>" }
  }],
  "annotations": { "com.grimoire.kind": "skill", "org.opencontainers.image.version": "1.2.0" }
}
```

With `--compression none`, the layer is today's descriptor, byte-for-byte, with no `annotations` key.

### CLI

`grim release`:

```
--compression <COMPRESSION>
        Layer compression for skill, rule, agent and hook releases; ignored for bundles and mcp.
        Consumers need grim >= <R> to install gzip layers.
        [default: none] [possible values: none, gzip]
```

`grim publish`:

```
--compression <COMPRESSION>
        Layer compression for skill, rule, agent and hook entries; ignored for bundles and mcp.
        Overrides publish.toml `compression`; none when neither is set.
        Consumers need grim >= <R> to install gzip layers.
        [possible values: none, gzip]
```

```sh
grim release ./skills/hello-world ghcr.io/acme/skills/hello-world:1.2.0 --compression gzip
grim publish --compression none            # overrides publish.toml compression = "gzip"
grim describe ghcr.io/acme/skills/hello-world:1.2.0 --format json | jq -r .compression
```

## Consumer impact and requirements

`<R>` below is the first grim release containing the D1 reader.

### Indexer (`@grimoire-rs/indexer`)

**What changes:** `describe` gains `compression`. The indexer only shells out to grim and parses no blobs or media types.
**If it does nothing:** with grim ≥ `<R>` everything works; the field is just not copied.
With grim < `<R>`, `fetch` on a gzip package exits 65; the package goes stale, the old sidecar is kept and a `warn:` line printed.

### VS Code extension (`grimoire-vscode`)

**What changes:** `describe` gains `compression`; failures carry a new reason slug on grim ≥ `<R>`.
**If it does nothing:** on grim ≥ `<R>` gzip packages install normally and the field is invisible.
On grim < `<R>`, install and fetch of a gzip package show grim's raw tar error.
The extension cannot pre-detect compression on that grim: an old `describe` has no `compression` field.

### Requirements

| Requirement | Owner repo | Priority | grim dependency |
|---|---|---|---|
| Copy `compression` from `describe` into the sidecar (`META_KEYS`); absent means unknown (old grim or `null`), only `"none"` means uncompressed | grimoire-indexer | P1 | ≥ `<R>` (absent-tolerant) |
| Warn in `checkGrim` when grim < `<R>` (`MIN_GRIM_FOR_GZIP`, `MARKETPLACE_GRIM_VERSION` pattern); keep the floor | grimoire-indexer | P1 | `<R>` |
| Optional `compression?: string` on `CatalogPackage` if the site shows it | grimoire-indexer | P3 | none |
| Add optional `compression` to `DescribeResult`, model, `DetailsVM`; render a "Compression" row whenever the field is present, `"none"` included | grimoire-vscode | P2 | ≥ `<R>` (read-site guard, no gate) |
| `COMPRESSED_LAYER_GRIM_VERSION = <R>` + `supportsCompressedLayers()`; on grim < `<R>` and an exit-65 fetch/install failure, show "may need grim ≥ `<R>`" | grimoire-vscode | P1 | `<R>` |
| On grim ≥ `<R>`, map reason `unsupported-layer-media-type` to "update grim" copy | grimoire-vscode | P3 | ≥ `<R>` |
| Do not raise either repo's `MINIMUM_GRIM_VERSION`, and do not bump `CACHE_VERSION`: entries key on digest, a stale one only hides the row, and a bump is a fleet-wide purge (`adr_cache_probe_truth.md`) | both | P1 | none |

Release ordering: grim `<R>` ships first. No index or catalog package opts in until both consumers run it.

## NFRs

- **Scalability:** stored bytes and egress drop 3–6× for opted-in artifacts. No new requests on the read path.
- **Latency:** gunzip runs in `spawn_blocking`, bounded by the caller cap. The guard adds one manifest GET, only on a digest mismatch.
- **Security:** T1 bomb bounded by the caller's existing cap on decoded bytes. Integrity stays on lock-pinned digests; the annotation is verified on read and is a guard key only on gzip layers. No change to the trust boundary (I1–I6).
- **Cost:** registry storage and egress fall for adopters. No new crate, CI job or infrastructure.
- **Operability:** every failure names its cause. Stopping the flag stops new gzip versions; versions already pushed stay gzip until `--force` re-released.

## Compatibility & migration

- **Old grim** on a gzip artifact: `fetch` exits 65 `cannot read tar entry`, nothing written (probe-verified on 0.14.3). `install` was probed on 0.14.3 (2026-10-21, local `registry:2`, skill released by the new grim with `--compression gzip`): after `grim lock`, `grim install --client claude` exits 65 `failed to materialize artifact: cannot read tar entry: failed to read entire block`; no client output, `grimoire.lock` unchanged, and `.grimoire/state.json` holds no record (only the empty workspace scaffold is created). Old grim on an OCI `…layer.v1.tar` artifact keeps working.
- **Unknown layer types** keep today's raw-tar read: no artifact that installs today stops installing.
- **Default output:** byte-identical manifests and layers. Locked by the existing wire-shape tests.
- **Docs, additive edits** (`docs/src/content/docs/`):
  - `stability.md:40`, `:59-65`, `:146` — gzip layer type and annotation join the frozen format; published layers are still never rewritten.
  - `artifacts.md:127-129` — "single tar or tar+gzip layer"; the read types.
  - `json-interface.md:105` (describe), `:487` (`layer_digest` stays the uncompressed digest), `:797-830` (the new reason slug).
  - `publishing.md:643-645`, `:717-722`, `:1224-1248`, schema refs — flag, manifest key, bundle/mcp ignore, guard and adopt semantics, dry-run preview vs adopted digest, "compress new versions only, never `--force` an existing one", consumer-version warning.
  - `commands.md:1742` (describe), `:2753`, `:2803` (release, publish flag), and the `fetch` row: the 8 MiB cap now bounds decoded content.
  - `configuration.md#registry-compatibility` — probe results; Artifactory pending.
  - Fix `AGENTS.md`'s stale `docs/src/stability.md` path in the same change.
- **Catalog drift review:** `grim-usage`, `grim-authoring`.
- **CHANGELOG:** `feat:` for the reader, the flag and the describe field. Generated by `task release:prepare`.

## Test plan

**Unit (Rust):**
- `oci::layer`: classify every row of the D1 table, unknown type → raw tar, `+zstd` → `UnsupportedMediaType`. Gzip round-trip. Cap boundary with a small injected limit (limit passes, limit + 1 fails). Corrupt gzip, trailing bytes, second member, FEXTRA/FNAME/FCOMMENT header. Digest mismatch, malformed `sha256:` value, unknown algorithm skipped. Control characters escaped in error text. `encode_layer` deterministic across two runs.
- `same_content`: equal across a compression flip; a pulled manifest (`config_media_type: Some(empty)`) against a freshly built one (`None`); differing config, `artifact_type`, manifest annotations, layer count; gzip annotation lie vs raw-tar layer with an annotation.
- `Descriptor` annotations survive `oci_descriptor` and `descriptor_from_oci`. `None` writes no key.
- Extend `memory_registry_release_pushes_cascade_idempotent_and_guards` (`release.rs:805`): `none` → `gzip` flip on unchanged content adopts with `pushed: true`; content change is `TagExists`; `--force --compression gzip` moves the tag to the new digest in both directions; existing-manifest fetch error propagates its exit code.
- `release`/`publish` args: `publish.toml` gzip with no flag → gzip; with `--compression none` → none; gzip on a bundle or mcp entry is ignored.
- Exit classification: each `LayerError` variant reaches 65 through `classify`, via `fetch_verified_layer` and via `fetch_artifact`; only `UnsupportedMediaType` carries the reason.
- `materializer.rs`, `installer.rs:3726`, `fetch.rs` `fetch_skill_canonical_content_and_files`, `fetch_with_limit_controls_truncation`: gzip blobs decode; a gzip layer under 8 MiB stored whose decoded size passes 8 MiB fails `grim fetch` and the companion with 65, never truncates.
- Fixtures using `application/vnd.grimoire.content.v1.tar` (`mcp/render.rs:225`, `command/add.rs:2510`, `fetch.rs:1136`, `:1641`) keep passing as unknown → raw tar.
- `describe_report.rs`: `compression` always serialised, `null` included; plain `compression` row.

**Acceptance (pytest):**
- `test/src/registry.py`: gzip push helper (custom and OCI types, with annotation).
- `test_release.py`: `--compression gzip` wire shape; idempotent re-run; `none` → `gzip` flip on unchanged content adopts; `--skip-existing` unchanged; live run on `registry:2` is the gate for the normalized comparison.
- `test_publish.py`: manifest key, flag override both ways, bundle/mcp ignore.
- `test_compression.py`: `grim schema --kind publish` gains the optional `compression` key.
- `test_describe.py`, `test_json_interface.py`: `compression` present as `"none"`, `"gzip"` or `null`.
- `test_fetch.py`: gzip fetch; `--digest-only` shape unchanged.
- Install of a gzip artifact; `+zstd` exits 65 with the reason slug; tampered annotation exits 65. One streamed bomb only if it never materialises the full output.
- `grim status` on a path source is unchanged by compression (dossier verification).
- Manual: old-grim (0.14.3) install of a gzip artifact exits 65 with no partial write; push, pull and install with new grim on GHCR, GitLab and Artifactory.

## Open questions

- [NEEDS CLARIFICATION: Does the company Artifactory accept `application/vnd.grimoire.artifact.layer.v1.tar+gzip` and keep the descriptor annotation byte-for-byte?]
  Recommended: rerun the three-tag probe with Artifactory credentials before docs recommend compression there. The reader ships regardless.
- [NEEDS CLARIFICATION: Should the description companion also be published gzip?]
  Recommended: no for now. Readers already decode it; revisit when a companion with a large logo is measured.
- [NEEDS CLARIFICATION: When does the default flip to `gzip`?]
  Recommended: a separate future ADR, gated on `<R>` adoption by the indexer, the extension and the company rollout.

## Review decisions (2026-10-22)

Findings deferred at review, resolved by the maintainer.

- **Hybrid push type** (OCI `tar+gzip` for compressed layers): rejected, keep the custom `+gzip` type.
- **Option 6** download-and-compare instead of D4: rejected, keep the annotation (D4).
- **Writer timing:** reader and writer both ship in 0.15.0. The writer is opt-in; first-party packages stay uncompressed until the indexer and the VS Code extension read gzip.
- **`resolve_existing_version` failed open:** fixed in this change. Only a definite not-found means "no existing version"; any other tag-lookup error aborts the release before a push or tag move, with the error's own exit code. Applies to `release`, `--skip-existing` and `publish`.
- **Streaming decode:** deferred until install memory is measured as a problem.
- **`compression` on `release`/`publish` reports:** added. Always present, `none`/`gzip` for tar kinds, `null` for bundle, mcp and failed entries; an adopted manifest reports its own, a `--skip-existing` no-op the requested value.

## Links

- `.agents/discussions/compressed-artifact-layers.md` — ratified dossier
- `.agents/discussions/registry-tag-overwrite.md` — split-out Artifactory tag-overwrite thread
- [adr_oci_empty_config_compat.md](./adr_oci_empty_config_compat.md) — GitLab accepts the custom layer type; why the config stays empty
- [adr_description_companion.md](./adr_description_companion.md) — the companion left out of scope
- [adr_local_path_sources.md](./adr_local_path_sources.md) — the lock `hash` contract D7 keeps
- [adr_unified_publish_version_cascade.md](./adr_unified_publish_version_cascade.md) — the guard and cascade D5 extends
- `.claude/rules/arch-threat-model.md` — T1

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-10-21 | hex architect (for Michael Herwig) | Initial draft, Proposed |
| 2026-10-21 | hex architect (for Michael Herwig) | Review round 1: unknown layer types keep raw-tar read (deviates from dossier line 23 under Principle 9); caller caps on decoded bytes replace the 1 GiB constant; normalized guard comparison, `--force` before adoption, annotation keyed on gzip only; publish flag `Option`; bundle/mcp ignore; release/publish report fields dropped; strict gzip framing; error wiring; deferred findings recorded |
| 2026-10-21 | hex architect (for Michael Herwig) | Review round 1 (implementation): adoption limited to tar kinds, bundle and mcp keep `TagExists` (D5); per-encoding content key, so an unreadable or unannotated gzip layer is never equal to a raw tar (D5); expansion-ratio bound `min(caller cap, max(64 MiB, 100 x stored))` (D6, PKG-05); the dry-run preview digest now hashes the exact pushed bytes (D5) |
| 2026-10-22 | Michael Herwig | Accepted; deferred findings resolved into Review decisions (fail-closed guard, report `compression` field added) |
