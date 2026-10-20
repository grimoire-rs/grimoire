# Research: Compressed OCI layers — prior art and registry behaviour

## Metadata

**Date:** 2026-10-18
**Domain:** packaging
**Triggered by:** `.agents/discussions/compressed-artifact-layers.md` — backwards-compatible layer compression
**Expires:** 2027-04-18 (GitLab allowlist and registry referrers support move fastest)

## Direct Answer

No surveyed tool has introduced compression into a non-image artifact type while keeping old clients working. Docker/containerd introduced zstd **reader-first**: read support shipped years before push, and old engines still fail on zstd images. Helm, Flux and Falco started out compressed, each with its own `…tar+gzip` media type. Dual-publish has no clean precedent, and Fedora notes you cannot push one image under both compressions directly. Using referrers to carry an alternate representation is undocumented; every known use is signatures, SBOMs or attestations.

## Key Findings

1. **Unknown media types.** The OCI image-spec says implementations MUST NOT error on an unknown `mediaType`, and must ignore unknown annotations. So no annotation can block an old client, and no `org.opencontainers.image.*` key expresses a minimum client version. `tar+gzip` is MUST-support and `tar+zstd` is SHOULD. The `+suffix` is defined only for OCI-defined types. ([manifest.md](https://github.com/opencontainers/image-spec/blob/main/manifest.md), [layer.md](https://github.com/opencontainers/image-spec/blob/main/layer.md))
2. **GitLab allowlist.** GitLab's registry matches media types exactly. `application/vnd.oci.image.layer.v1.tar+gzip` and `+zstd` are listed. A custom `+gzip` variant is not, and Flux and WASM hit `MANIFEST_INVALID … unknown media type` on exactly this. ([supported-media-types.md](https://gitlab.com/gitlab-org/container-registry/-/raw/master/docs/supported-media-types.md), [#951](https://gitlab.com/gitlab-org/container-registry/-/issues/951), [#1869](https://gitlab.com/gitlab-org/container-registry/-/issues/1869))
   - Unresolved conflict: `adr_oci_empty_config_compat.md` records that GitLab *accepts* grimoire's custom layer type, which is not on that list. Either layer types are checked differently from config types, or the list is stale. **Test before relying on either reading.**
3. **Reader-first rollout.** Moby/containerd shipped zstd reads first ([moby#41759](https://github.com/moby/moby/pull/41759)), and `docker push` was still gzip-only in 2022 ([moby#28394](https://github.com/moby/moby/issues/28394)). Old engines and NAS Docker still fail on zstd images. One client broke on a manifest that mixed gzip and zstd layers ([enroot#250](https://github.com/NVIDIA/enroot/pull/250)). Nothing tells a publisher when the switch is safe.
4. **Dual-publish.** Fedora: "not possible to directly push container images with both compression formats" ([Fedora wiki](https://fedoraproject.org/wiki/Changes/zstd:chunked)). Homebrew carries custom selection metadata on an image index, but grim rejects image indexes.
5. **Referrers.** The spec has a referrers API with a tag-schema fallback that is race-prone and clutters tag listings ([distribution-spec](https://github.com/opencontainers/distribution-spec/blob/main/spec.md), [Chainguard](https://www.chainguard.dev/unchained/building-towards-oci-v1-1-support-in-cosign)). Old clients never query referrers. A new client pays an extra round trip per install. ghcr and Docker Hub support is not established.
6. **Transport-only compression.** The distribution spec says nothing on `Content-Encoding`. containerd 2.0 sends `Accept-Encoding` and verifies the digest on the decoded stream (secondary source: GitLab MR 1789). No registry documents compressing blob responses. Blobs are usually served by redirect to S3 or a CDN, verbatim.
7. **Size, measured on repo markdown.** gzip -9 is about 3.2–3.7× smaller than tar. zstd -19 is about 10% smaller than gzip; zstd -3 is 2–3% smaller. zip is 9–22% *larger* than tar.gz because it compresses per file. A few-KB artifact saves under 1 KB.

   | Sample | tar | gzip -9 | zstd -19 | zip -9 |
   |---|---|---|---|---|
   | `catalog/skills/grim-usage` | 174,080 | 58,650 | 52,518 | 63,808 |
   | `catalog/skills/grim-authoring` | 143,360 | 44,610 | 39,918 | 54,363 |
   | `.claude/rules` | 2,713,600 | 738,359 | 601,917 | — |

8. **Determinism.** gzip without `-n` embeds an mtime. With `-n` and a pinned tar, output was byte-identical across runs (md5-verified). gzip and zstd bytes still depend on implementation, version and level. Helm only became reproducible in 4.0.0 ([reproducible-builds.org](https://reproducible-builds.org/docs/helm/), [helm#3612](https://github.com/helm/helm/issues/3612)).
9. **Other registries.** Nexus failed validation on zstd layers ([nexus-public#384](https://github.com/sonatype/nexus-public/issues/384)). Docker Hub stores zstd layers but did not scan them. No documented layer allowlist was found for ghcr, ECR, Harbor or zot.

## Open leads

- Push the current custom layer type and a `+gzip` variant to gitlab.com to resolve finding 2's conflict.
- Run `curl -I -H 'Accept-Encoding: gzip'` against a real ghcr or Docker Hub blob and its redirect target. Only this settles transport-only compression.
- Check containerd `images/mediatypes.go` and go-containerregistry for how they dispatch on the media-type suffix, as a reference for a sniff-first reader.
- Read Helm 4.0's reproducible `helm package` as the closest precedent for a deterministic gzip policy.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| https://github.com/opencontainers/image-spec | Spec | 2026-10 | Unknown-type tolerance, layer suffixes |
| https://gitlab.com/gitlab-org/container-registry/-/raw/master/docs/supported-media-types.md | Docs | 2026-10 | Exact-match allowlist |
| https://fedoraproject.org/wiki/Changes/zstd:chunked | Wiki | 2026-10 | Dual-compression limits, rollout |
| https://github.com/moby/moby/pull/41759 | Repo | 2021 | Reader-first zstd |
| https://reproducible-builds.org/docs/helm/ | Docs | 2026-10 | Deterministic gzip packaging |
