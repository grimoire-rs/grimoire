# Research: Compressed artifact layers — codebase recon

## Metadata

**Date:** 2026-10-18
**Domain:** packaging
**Triggered by:** `.agents/discussions/compressed-artifact-layers.md` — how to compress published layers backwards-compatibly
**Expires:** 2027-04-18 (re-verify after any change to `src/oci/**`, `src/install/materializer.rs`, `src/command/release.rs`)

## Direct Answer

Nothing on the pull path reads the layer `media_type`. Every grim parses the single layer blob as a raw tar, so a compressed layer makes an older grim fail with exit 65 `failed to materialize artifact: cannot read tar entry: archive header checksum mismatch`. That failure is loud, with no partial write. The "already published" check compares manifest digests, so any change to the bytes, including compressor drift, refuses a re-release without `--force`.

## Key Findings

1. **Write site.** The layer media type literal `application/vnd.grimoire.artifact.layer.v1.tar` is set in `src/command/release.rs:272` and copied into the pushed manifest by `oci_descriptor` (`src/oci/access/registry_client.rs:283-285`, `:516`). `GRIMOIRE_LAYER_MEDIA_TYPE` (`registry_client.rs:43`) is used only as an Accept type in `put_tag` (`:576`). It is never a check.
2. **No media-type gate on pull.** `OciManifest::single_layer()` (`src/oci/manifest.rs:65`) checks only the layer count. Kind comes from `artifactType`, the config media type or the `com.grimoire.kind` annotation (`src/oci/annotations.rs:372`), never from the layer type.
3. **Tar consumers.** `DefaultMaterializer::materialize` (`src/install/materializer.rs:56`) and `unpack_tar_in_memory` (`:141`, called from `src/fetch.rs:455,493,686`) are the tar consumers. They serve install, update, add, the TUI, MCP render and fetch. `MaterializeFailed` maps to DataError 65 (`src/error.rs:456`).
4. **Size caps.** `INSTALL_LAYER_SIZE_LIMIT` (512 MiB), `FETCH_BLOB_SIZE_LIMIT` (8 MiB) and `CappedSink` bound the declared, compressed size. **Nothing bounds decompressed output**: a compressed layer opens a decompression-bomb surface (attacker T1).
5. **Digests.** Lock entries pin the manifest digest. The blob store keys on the layer digest as pushed (`src/store/blob_store.rs:30`, `src/oci/access/cached_access.rs:114`). The path-source and dev-install lock `hash` is the SHA-256 of the packed layer bytes (`src/lock/locked_source.rs:86`, `src/command/status.rs:1655`, `src/install/installer.rs:2313`). Changing pack output shows those as `Outdated`. The install-record content hash covers the materialized tree and does not depend on layer bytes.
6. **Idempotent release.** Release re-runs are idempotent only on byte-identical layers (`src/command/release.rs:836` test). The overwrite guard compares manifest digests (`release.rs:306`). `--skip-existing` is version-tag-only (`release.rs:284`).
7. **The tar is deterministic today.** `append_entry` (`src/skill/skill_package.rs:479-487`) sets mode 0644, mtime 0 and uid/gid 0, and sorts entries. Tests: `pack_is_deterministic` (`:1091`) and the rule and agent variants (`:871`, `:940`). `adr_local_path_sources.md:56` calls this a byte-for-byte contract.
8. **Dependencies.**
   - `flate2 = "1"` resolves to 1.1.10 in the lock, with both miniz_oxide and zlib-rs present. Today it is used only to gunzip `all.json`.
   - `tar = "0.4"`.
   - `zip = "9"` has default features off, so it has no deflate reader. `src/export/archive.rs` writes Stored, reproducible zips.
   - There is no `zstd` crate.
9. **Frozen-format text.**
   - `docs/src/stability.md:40` freezes the "OCI wire format", and `:146-149` says a published layer is never rewritten.
   - `docs/.../artifacts.md:129` says "single tar layer".
   - ADRs: `adr_multifile_rules.md:29,36,54`, `adr_local_path_sources.md:56`, `adr_description_companion.md:109` and `adr_oci_artifact_type.md:102`.
   - Test pins: `test/src/registry.py:42` and `test/data/golden/pre_hooks_03e59b0/tools/generate.py:30`.
10. **No compatibility hook.** There is no min-grim-version or layer-format annotation. `com.grimoire.compatibility` (`annotations.rs:231`) is a free-text editor hint for skills only. Bundles and MCP already use distinct layer media types (`…bundle.v1+json`, `…mcp.v1+json`).
11. **Size reality.** Uncompressed tar vs `gzip -9`, run on `catalog/`:

    | Artifact | Tar | Gzip |
    |---|---|---|
    | `grim-usage` | 174 KB | 59 KB |
    | `grim-authoring` | 143 KB | 45 KB |
    | `ai-config-authoring` | 82 KB | 26 KB |
    | hooks | ~10 KB | ~1.7 KB |

    Rules and agents are 1-2 KB of text, and their tars are dominated by 512-byte headers. Gzip gives about 3x on skills and about 6x on tiny tars.

## Open leads

- Does GitLab, GHCR or `oci-client` accept a `+gzip`/`+zstd`-suffixed custom layer type? GitLab's allowlist is known to reject a custom config type and `artifactType`.
- Is flate2 output byte-identical across the miniz_oxide and zlib-rs backends and across crate versions?
- Should the path-source lock `hash` stay on uncompressed tar bytes, given that it feeds `status` drift?
- Does any surface (`describe`, `fetch --digest-only`, the TUI) display layer `size` as if it were content size?

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| repo tree at `main` (`92b9e0c9`) | Code | 2026-10-18 | All file:line citations above |
| `tar-0.4.46` `archive.rs:322` | Code | 2026-10-18 | Header-checksum failure an old grim hits on compressed bytes |
