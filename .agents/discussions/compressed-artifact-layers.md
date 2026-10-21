State: handed-off → architect · Updated: 2026-10-21
Ratified: 2026-10-21 → architect
Confidence: ratified by the owner through `/goal` (autonomous run: architect, execute, one bounded review). Research vintage 2026-10-18 to 2026-10-21, including the live GHCR and GitLab probe.

## Intent

Support compressed ("zipped") published artifact layers without breaking existing grim clients or the frozen OCI wire format (Principle 9, `docs/src/stability.md:40`).

The owner added a requirement at handoff on 2026-10-21. Whether a package is compressed must be visible on the CLI's JSON surface, so the VS Code extension (`grimoire-vscode`) and the indexer (`@grimoire-rs/indexer`) can read it from an appropriate command. The ADR must carry an impact and requirements analysis for both consumers. It must also carry a concise interface section covering pseudo-code, JSON, OCI and CLI.

Out of scope:
- zstd, or any second algorithm;
- making compression the default;
- switching the push to the standard OCI layer types;
- an uncompressed-size annotation;
- department tag isolation, which is `.agents/discussions/registry-tag-overwrite.md`.

## Requirements

- Readers come first. New grim installs both the existing uncompressed tar layer and a gzip layer. Publishing stays uncompressed by default.
- Publishing compressed output is opt-in, through a flattened clap `ValueEnum` flag `--compression <none|gzip>` on the publishing commands, the same flag shape as `src/command/schema.rs:48`. No short flag: `-c` conventionally means `--config`, which grim has as a global long flag (`src/cli/options.rs:85`). This took the recommended default at handoff. Default is `none`. A future algorithm such as zstd is an added enum literal, which is additive under Principle 9. The flag pair `--gzip`/`--no-gzip` was considered and rejected because it would need a second flag per algorithm. Flipping the default to opt-out later is a separate future decision, gated on reader adoption.
- The compressed layer media type grim publishes is `application/vnd.grimoire.artifact.layer.v1.tar+gzip`, matching the existing custom `application/vnd.grimoire.artifact.layer.v1.tar`. Every registry that grim works with today already accepts a custom layer type, so it does not check layer types against an allowlist. GitLab's live test of 2026-06-19 showed this (`adr_oci_empty_config_compat.md:34`).
- Readers accept four layer types: the two grimoire types and the standard `application/vnd.oci.image.layer.v1.tar` and `application/vnd.oci.image.layer.v1.tar+gzip`. Any other layer type fails with a clear exit-65 error naming the type, instead of today's misleading tar checksum error. The standard types also cover artifacts pushed by generic tools (oras), and leave a fallback if a registry ever rejects the custom type. Accepting them now means a later release can switch the push to the standard types. Old grim parses the layer as raw tar without checking its type, so it can still read a pushed `application/vnd.oci.image.layer.v1.tar`; only gzip needs new grim. A switch changes manifest digests for unchanged content, which the re-release guard must treat as same content.
- Artifact kind rides on the `com.grimoire.kind` annotation, never on the layer type. The push sets no `artifactType` because GitLab rejects a custom one.
- Decompressed output must have a generous size limit, on the order of 1 GiB, because current caps bound only the stored (compressed) bytes. This closes a T1 decompression-bomb surface without constraining real artifacts.
- The path-source and dev-install lock `hash` stays on the uncompressed packed tar, so `grim status` drift does not flip with compression. Compression happens at push time only.
- The pull-path integrity checks are unchanged. Blob and manifest digests cover the stored bytes, so verification works as it does today.

## Decisions

- Gzip over zstd: zstd is only 2–10% smaller and would add a new crate.
- Zip is rejected for layers: it is larger than tar.gz, its per-entry timestamps hurt determinism, and OCI does not bless it. `grim export --zip` already covers the zip use case.
- Old grim on a compressed layer fails loudly with exit 65 and writes nothing. That is acceptable for opt-in publishers.

## Threads

- Settled 2026-10-21 (owner agreed): compressed layers carry a layer-descriptor annotation `com.grimoire.uncompressed.digest` holding the digest of the uncompressed tar. It is written only on compressed layers. An uncompressed layer's descriptor digest already is the uncompressed digest, and stamping it on every layer would change the manifest digest of every unchanged re-release after a grim upgrade, giving a false `TagExists` everywhere.
  - Guard (`src/command/release.rs:306`): a same manifest digest stays a no-op, as `publishing.md#git-idempotent` documents. On a mismatch it fetches the existing manifest only, never a blob. It compares per layer (annotation, or descriptor digest when uncompressed) plus config and manifest annotations. Equal means a no-op success; different means `TagExists`, as today. `--skip-existing` (`release.rs:284`) is unchanged and fetches nothing.
  - This covers a `--compression` flip, compressor drift, the later switch to OCI layer types, and multiple layers. It needs no fallback for existing artifacts, because they are all uncompressed.
  - The annotation is a publisher claim. Install never trusts it for integrity: the lock-pinned manifest and blob digests stay the integrity check. Install may additionally check the decompressed bytes against the annotation and exit 65 on a mismatch.
  - The change is in this repo only, and the `external/rust-oci-client` submodule is untouched (its `OciDescriptor` already has `annotations`). The pieces: grim's `Descriptor` (`src/oci/manifest.rs:25`) gains an optional `annotations` field; the write mapping `oci_descriptor` (`src/oci/access/registry_client.rs:283`, today `annotations: None`) passes it through; the read conversion (`src/oci/manifest.rs:120`) keeps it.
  - Precedent: eStargz carries uncompressed-layer facts as layer annotations (for example `io.containers.estargz.uncompressed-size`). Not re-verified in this session.
  - The download-and-compare alternative is superseded.
  - No uncompressed-size annotation (YAGNI, 2026-10-21). A size the publisher claims cannot replace the streaming 1 GiB decompression limit, because an attacker would lie, and nothing consumes it today. Adding it later is additive, if `describe` or the TUI wants to display a size.
- Split out, 2026-10-20: several departments publish to one Artifactory through separate OIDC integrations. Moving the cascade tags (`X.Y`, `X`, `latest`; `src/oci/release.rs` `publish_tags`) needs overwrite rights, so departments can overwrite each other, and exact versions are mutable at the registry. The guard is client-side only. `--no-cascade` already exists (`src/command/release.rs:81`). Separate plan; parked at `.agents/discussions/registry-tag-overwrite.md` with its own research.

## Research

- Live probe on 2026-10-21, GHCR. Source: `grim release` of `test/manual/catalog/skills/hello-world` (tar 2,560 B, gzip 422 B), re-pushed by a stdlib script as three tags on `ghcr.io/michael-herwig/grim-compression-test/hello-world`:
  - `uncompressed`, the baseline;
  - `custom-gzip`, layer type `application/vnd.grimoire.artifact.layer.v1.tar+gzip`;
  - `oci-gzip`, layer type `application/vnd.oci.image.layer.v1.tar+gzip`.

  Both gzip tags carry the `com.grimoire.uncompressed.digest` layer annotation. Results:
  - GHCR accepted all three with 201 and stored each manifest byte-for-byte (fetched digest equals pushed digest), with the layer annotation kept.
  - grim 0.14.3 `fetch` exits 0 on the baseline and 65 on both gzip tags with `failed to materialize artifact: cannot read tar entry: failed to read entire block`, writing nothing. `describe` still reads `kind: skill`.
  - Docker and dive 0.13.1 pull and decompress even the custom `+gzip` type (one layer, 525 B).
  - GitLab, same day: the same three tags went to `registry.gitlab.com/michael-herwig/grimoire-registry-test/hello-world`.
    - All three were accepted with 201, stored byte-for-byte, and kept the layer annotation.
    - The custom `+gzip` layer type is accepted, which settles the allowlist conflict: GitLab does not check layer media types against its allowlist.
    - grim 0.14.3 behaves exactly as on GHCR: exit 0 on the baseline, exit 65 on both gzip tags.
    - The test project lives under `michael-herwig/`, not the `grimoire-rs/` path that `adr_oci_empty_config_compat.md` names.
  - Not yet probed: Artifactory, the company target.
- `.agents/research/research_compressed_layers_recon.md` — codebase recon: no layer media-type check on pull (old grim → exit 65 on compressed bytes); overwrite guard keys on manifest digest; no bound on decompressed size; gzip ≈3× on skills.
- `.agents/research/research_compressed_layers_prior_art.md` — prior art: no non-image artifact has introduced compression with old-client compat; Docker went reader-first; GitLab allowlists the standard `application/vnd.oci.image.layer.v1.tar+gzip` but not custom `+gzip` variants; zstd only 2–10% better than gzip; zip worse than tar.gz.

## Related

- `.agents/discussions/registry-tag-overwrite.md` — split-out discussion on department tag overwrites in Artifactory; picked up after this one drains.

## Open questions

- [NEEDS CLARIFICATION: Does the company Artifactory accept the custom `+gzip` layer type and the descriptor annotation?] Recommended: run the same three-tag probe against Artifactory before compression is recommended there. GHCR and GitLab passed. Artifactory needs credentials that were not available in this session.
- [NEEDS CLARIFICATION: Which command exposes compression to the VS Code extension and the indexer?] Recommended: `grim describe --format json`, plus `grim fetch --digest-only` if cheap. The indexer and the extension both already read `describe`-shaped manifest metadata, and the field is a manifest fact, readable without a blob download.

## Verification

- Registry compatibility: GHCR and GitLab were proven by the 2026-10-21 probe (see Research). Remaining: the same push to the company Artifactory, then a pull and an install with new grim on all three registries. GitLab checks media types against an allowlist; no source says either registry checks descriptor annotations, which is why the push is needed.
- An old-grim install of a gzip artifact exits 65 with no partial write.
- A layer that decompresses past the 1 GiB limit exits 65.
- An unknown layer media type exits 65 with an error naming the type.
- A re-release of unchanged content after a `--compression` flip is a no-op, not `TagExists`.
- `grim status` on path sources is unchanged by compression.
