# Research: OCI artifact signing and cross-project code sharing — prior art

## Metadata

**Date:** 2026-11-16
**Domain:** security
**Triggered by:** Decision on a signing/verification design for an OCI-artifact package manager in Rust, and on how two sibling Rust CLIs share security-critical code.
**Expires:** 2027-05-16 (registry behaviour and Sigstore client support move fast; re-verify sooner)

Evidence quality: web search with page fetches. Many registry claims rest on one third-party matrix (stella-ops) or on undated issues. Every such claim is flagged "secondary" or "unverified". Findings only; no recommendation.

## Direct Answer

Cosign 3.x defaults to storing a Sigstore bundle as an OCI 1.1 referrer manifest (layer = serialized bundle, `subject` = signed digest). A registry that answers 404 on the referrers endpoint forces the client to use the `sha256-<hex>` fallback index tag; the legacy `sha256-<hex>.sig` tag is a third, separate scheme. Registry referrers support is uneven: GHCR, GitLab and Docker Hub are reported (secondary sources) as fallback-only or unclear, while Harbor, Quay, ECR, ACR, zot and distribution have documented or reported support, with version caveats. Four verification families exist, with different online needs: Sigstore keyless (Fulcio, Rekor, TUF root, OIDC identity), Notation (X.509 trust store plus a trust policy with strict/permissive/audit/skip levels), GitHub attestations (Sigstore bundles via the GitHub API, `gh attestation verify`, offline path with downloaded trusted root), and key-based cosign (no online dependency). In Rust, the `sigstore` crate (sigstore-rs) is self-described as experimental, 0.15.0 on 2026-11-05, lists "attestation verification not yet supported" in its README, and no source found confirms Rekor v2 support; a separate `sigstore/sigstore-rust` workspace (`sigstore-verify` 0.16.0, 2026-11-09) advertises bundle v0.1-v0.3, TUF, RFC 3161 and identity policies. Comparable tools mostly split into "verify at install, declared identities" (Flux, mise/aqua) and "attest at publish, verify opt-in or manual" (npm, PyPI, Homebrew, cargo trusted publishing). For code sharing, the Rust project's own write-ups report concrete costs for submodules and git subtree; evidence on crates.io, git deps and vendoring is mostly general guidance, not maintainer post-mortems.

## Technology Landscape

### Trending (gaining momentum)

| Tool/Pattern | Adoption Signal | Key Benefit | Relevance to Grimoire |
|---|---|---|---|
| Sigstore bundle v0.3 as referrer | Cosign 3.x default; Rekor v2 in cosign 2.6.0+/3.0.1+, Go, Python, Java clients | One portable bundle holds cert, log proof, timestamp | Gives offline-verifiable single blob per signature |
| Identity-pinned keyless verification | Flux `matchOIDCIdentity`; mise v2026.10.0 tightened keyless to check signer, not only chain | Trust names a workflow identity, no long-lived keys | Maps to GitHub OIDC / GitLab CI publishing |
| Trusted publishing (OIDC, no static token) | crates.io launched Jul 2025, enforceable per crate Jan 2026 | Publisher identity without secrets | Same OIDC base as keyless signing |
| Pure-Rust Sigstore verifier crates | `sigstore-verify` 0.16.0 (2026-11-09), under `sigstore` GitHub org; `sigstore-verification` used by mise | Smaller verify-only surface | Alternative to the `sigstore` crate |

### Established (proven, widely accepted)

| Tool/Pattern | Status | Notes |
|---|---|---|
| OCI 1.1 referrers API + fallback tag schema | Standard (spec 1.1 released 2024-03) | Client must maintain fallback index when registry lacks API |
| Key-based cosign | Mature | No online dependency, key management on the publisher |
| Notation + trust policy | Specs v1.0.0, audited 2023 | X.509 PKI model; policy names registries, trust stores, identities, level |
| GitHub artifact attestations | GA; `actions/attest@v4` | Sigstore bundles; public repos use public-good Sigstore, private use GitHub's own instance |
| Digest pinning in lockfile | Universal | Integrity only, not authenticity |

### Emerging (early but promising)

| Tool/Pattern | Signal | Worth Watching Because |
|---|---|---|
| Rekor v2 (tile log, no SET, external TSA) | GA post Oct 2025; v1 freeze announced a year ahead, no date | Changes what a verifier must support (TSA, tiles, TUF-distributed shard URLs) |
| PEP 740 attestations | PyPI GA; pip/uv do not verify yet | Shows publish-side attestation shipping years before install-side verification |
| Lockfile-carried attestation identities (`pylock.toml`) | Format carries them, installers ignore them | Lockfile-plus-signature design precedent |

### Declining (losing mindshare)

| Tool/Pattern | Signal | Avoid Because |
|---|---|---|
| Legacy cosign `.sig` tag scheme | Deprecated-flag removal planned for cosign v4, no date found | Separate copy/mirror step; not a referrer |
| Helm PGP `.prov` files | Docs page not updated for Helm 4 | Not the OCI-native path; cosign used out of band |

## Registry support matrix (referrers API / fallback)

Primary sources are scarce. Rows marked S rest on the stella-ops third-party matrix or undated issues; re-verify each before relying on it.

| Registry | Referrers API | Fallback tag needed | Cosign tested | Evidence |
|---|---|---|---|---|
| GHCR | Reported none (S) | Yes | Listed | No GitHub primary statement found |
| Docker Hub | Unclear: "partial" (S), "no subject support" (SOCI table), "blocked" (2023 talk) | Likely | Listed | Docker 2023 post said support "on the horizon" |
| GitLab Container Registry | No endpoint; stores subject/artifactType; referrers exposed in tags API only | Yes | Listed | GitLab work item "no OCI referrers endpoint today"; issue 1205 conformance failures Feb 2024, status not visible |
| Harbor | Reported native, version cutoff disputed (2.0 / 2.6 / 2.8.1) | Older versions | Listed | Sources conflict |
| Quay / Quay.io | Announced (Red Hat post) | Deployment-dependent (S) | Listed | Version-dependent |
| ECR | Supported (AWS blog on OCI 1.1) | No | Listed | AWS roadmap issue 2783: 405 on pushing referrer manifests from a ghcr.io source |
| ACR | Supported (Microsoft: OCI v1.1.0) | No | Listed | Microsoft announcement |
| GAR | "Full" (S) | -- | Listed | No Google primary found |
| zot | Supported | No | -- | Chainguard post |
| distribution / registry:2 | Not verified | -- | Listed | No source found on referrers in the `registry:2` image |

What breaks: a client that only calls the referrers endpoint finds nothing on GHCR/GitLab-class registries; a client that pushes a subject-bearing manifest must also update the fallback index (non-atomic read-modify-write). Mirroring signatures between registries needs the referrer copied, plus legacy `.sig` objects copied separately.

## Signing schemes: what each proves and needs

| Scheme | Proves | Online at verify | Offline story | Key burden |
|---|---|---|---|---|
| Cosign keyless | Identity (OIDC issuer+subject) signed digest at a logged time | Not strictly: bundle carries cert, proof; needs trusted root (TUF) refreshed online | Ship trusted root file; revocation/rotation staleness | None for publisher; verifier must pin identities |
| Cosign key-based | Holder of key signed digest | No | Full | Publisher key custody and rotation |
| Notation | X.509 chain to a trust-store cert, timestamp, expiry, revocation per level | CRL/OCSP for revocation at strict level | Trust store local; revocation needs network | PKI/cert management |
| GitHub attestations | Build ran in named repo/workflow (SLSA-style provenance) | `gh attestation verify` fetches bundle via GitHub API | Documented path: `gh attestation download` + `trusted-root` file; GitHub warns revocation unknowable offline; check repo ID not name | None for publisher on GitHub; GitHub-specific |
| SLSA provenance / in-toto | Statement about how artifact was built (predicate), signed by an above scheme | Same as carrier | Same as carrier | Same as carrier |

## Rust implementation options

| Option | Evidence |
|---|---|
| `sigstore` (sigstore-rs) | "Experimental", "not stable until 1.0". 0.15.0 released 2026-11-05. 247 stars, 85 forks, 52 open issues. README: cosign sign/verify, keyless, Rekor client, Fulcio client; "attestation verification not yet supported". Optional deps include `oci-client ^0.18`, `tough ^0.24`, `openssl`, `aws-lc-rs`, `reqwest ^0.13`. Rekor v2 and bundle v0.3 support not confirmed by any source found. |
| `sigstore-verify` family (`sigstore/sigstore-rust`) | 0.16.0 on 2026-11-09; owners baszalmstra, wolfv, jku; split crates (types, bundle, crypto, merkle, rekor, trust-root, tsa, sign, oidc, tuf). README: bundle v0.1-v0.3, keyless sign via OIDC, TUF trusted root, checkpoints/inclusion proofs/SETs, RFC 3161, identity policies. Rust 1.86+. Rekor v2 not stated. Releases section empty on page; README version snippets inconsistent (0.11 vs 0.14). No OCI referrer logic. |
| `sigstore-verification` | Used by mise for GitHub attestation, cosign, SLSA checks (docs.rs source of mise). Not examined further. |
| Shelling out to `cosign`/`notation` | Not researched in depth. Implies external binary on PATH and an out-of-process trust boundary; no source examined. |
| Notation Rust libs | None found. Reference implementation is Go (`notation-go`). |

Why a fork of sigstore-rs: no source found stating reasons. Plausible gaps visible in the evidence: no attestation verification, Rekor v2 unconfirmed, `oci-client` coupling (the owner's own fork of `oci-client` exists). These are inferences, not found statements.

Rekor v2 verifier requirements (primary): no signed entry timestamp, trusted time from RFC 3161 TSA, tile-based inclusion proofs, shard URLs distributed via TUF and not to be hardcoded. Clients listed with support: cosign, Go, Python, Java; Rust not named.

## Comparable tools

| Tool | Verify default | Policy model | Failure UX / lockfile | Source |
|---|---|---|---|---|
| Flux OCIRepository | Opt-in per resource (`spec.verify`) | `provider: cosign` or `notation`; keyless via `matchOIDCIdentity` issuer+subject regex, else secretRef key; declared in the resource, in-cluster | Reconcile fails on mismatch; no lockfile | flux2 PR 4727, source-controller commit |
| Helm | `--verify` is PGP `.prov`; cosign done out of band | Keyring | Separate step; `+` in tag breaks cosign | docs.helm.sh provenance |
| mise / aqua | On by default, per recipe | Registry recipe declares checks and expected signer; user can supply signer/issuer; `attestations_since` makes missing attestation fatal | Missing attestation tolerated unless `attestations_since`; lockfile can require SLSA and fail if no signer | mise aqua docs, release v2026.10.0 |
| Homebrew | Opt-in beta (`HOMEBREW_VERIFY_ATTESTATIONS`); default-on discussion closed "not planned" 2025-08 | Repo-bound `gh attestation` | Rate-limit failures; `HOMEBREW_NO_VERIFY_ATTESTATIONS` escape | brew issue 17019 |
| npm | Install does not verify; `npm audit signatures` separate | Registry key; provenance from Sigstore | Post-install, scripts already ran | npm docs, Ledger page |
| PyPI / pip / uv | Attestations generated by default with trusted publishing; installers do not verify | Publisher identity in attestation | `pypi-attestations` CLI experimental; `pylock.toml` carries identities, unused | PEP 740, pydevtools |
| cargo / crates.io | No signatures; trusted publishing shipped 2025, enforceable 2026 | OIDC publisher config on crates.io | Sigstore RFC 3403 appears open | RFC 3691, blog.rust-lang.org 2026-01-21 |
| ORAS, Nix, Bazel/bzlmod, uv (Rust crate sharing) | Not researched | -- | -- | unverified |

## Policy and UX patterns (findings)

- Notation: policy file names registry scopes, trust stores, trusted identities and a level (strict default; permissive, audit, skip). Source: Notary trust-store/trust-policy spec.
- Flux and mise declare trusted identities in the consuming config or in a vendored registry snapshot, not in the artifact. mise ships a registry snapshot per release; `registry_floating` opts into live registry.
- mise v2026.10.0 fixed keyless verification that checked chain-to-Fulcio but not signer: a pinned-identity miss is a real failure mode.
- GitHub offline docs warn that repo name in the cert can go stale after rename; compare repo ID.
- Repo-declared trust: no source examined states the threat directly for package managers; the project's own `AGENTS.md` records the CWE-426 reasoning for hooks consent. Not independently sourced.

## Code sharing between sibling Rust CLIs

| Mechanism | For | Against | Evidence |
|---|---|---|---|
| (i) crates.io library crates | Rust project guidance: "use crates.io for libraries useful to others"; semver gives explicit contract | Publishing cadence and semver burden; git/path deps cannot be published; `[patch]` crates cannot publish | rustc-dev-guide; Cargo book via search results |
| (ii) git dep pinned by rev | No publish step; cargo fetches and updates submodules in git deps | Cannot publish to crates.io; whole repo cloned; Cargo.lock rev bumps are manual per consumer | forum + Cargo docs (search summary) |
| (iii) submodule + path/`[patch]` | Local atomic-ish edit loop in the consumer | Rust project: submodules stay empty or on wrong commit if not checked out; accidental unrelated submodule changes ("relatively common mistake"); parent and sub changes cannot land atomically | blog.rust-lang.org/inside-rust 2026-06-04 |
| (iv) vendored copy + sync script | No checkout friction | Drift; sync tooling cost; git subtree reported "entirely unusable" at Rust repo size, syncs never finished (Miri), broken blame, duplicate commits | same blog |
| (v) one monorepo/workspace | Single lock, no cross-repo versioning (uv keeps ~70 crates in one workspace) | Merges two histories and release cadences; no source found on cost | pydevtools/instagit summaries (secondary) |
| (vi) third shared repo | Neutral home; combine with (i) or (ii) | Adds a third release cadence; no source found | -- |

Rust project's tooling answer: Josh ("subtree on steroids"), sync an order of magnitude faster, cost: many merge commits; used for Miri, rust-analyzer, compiler-builtins, stdarch. Cargo remains a submodule (tools independent of compiler).

Not found: maintainer write-ups on ruff/uv/ty, deno, rustup/cargo (`cargo-util`), jj, helix, `oci-client`/oras Rust crate sharing. Treated as unverified.

## Key Findings

1. Cosign 3.x stores bundles as OCI 1.1 referrers; fallback is `sha256-<hex>` index tag; legacy is `.sig` tag. https://oneuptime.com/blog/post/2026-08-11-cosign-signature-storage-oci-referrers-separate-repositories/view (secondary); https://opencontainers.org/posts/blog/2024-03-13-image-and-distribution-1-1/
2. Rekor v2 drops SETs, needs external TSA, shard URLs via TUF; Rust not listed among supporting clients. https://blog.sigstore.dev/rekor-v2-ga/
3. `sigstore` crate 0.15.0 experimental, attestation verification unsupported. https://github.com/sigstore/sigstore-rs ; https://docs.rs/crate/sigstore/latest
4. `sigstore-verify` 0.16.0 advertises bundle v0.3, TUF, RFC 3161. https://github.com/sigstore/sigstore-rust ; https://docs.rs/sigstore-verify
5. GitHub attestations: offline path via downloaded bundle + trusted root, revocation unknowable. https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/verify-attestations-offline
6. Notation: strict/permissive/audit/skip levels, trust store + policy. https://github.com/notaryproject/specifications/blob/v1.0.0/specs/trust-store-trust-policy.md
7. Flux keyless via `matchOIDCIdentity`. https://github.com/fluxcd/flux2/pull/4727
8. mise verifies by default per recipe; keyless identity check tightened 2026.10.0. https://mise.jdx.dev/dev-tools/backends/aqua.html ; https://newreleases.io/project/github/jdx/mise/release/v2026.10.0
9. Homebrew verification stayed opt-in beta; default-on closed not planned. https://github.com/Homebrew/brew/issues/17019
10. PyPI attestations: installers do not verify. https://peps.python.org/pep-0740 ; https://pydevtools.com/handbook/explanation/why-pylock-toml-includes-digital-attestations/
11. npm install does not verify provenance. https://docs.npmjs.com/generating-provenance-statements (via search); https://developers.ledger.com/docs/ledger-wallet-provider/package-attestation.md
12. crates.io trusted publishing shipped; signing RFC open. https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html ; https://github.com/rust-lang/rfcs/pull/3403
13. Submodule/subtree costs at Rust project. https://blog.rust-lang.org/inside-rust/2026/06/04/how-josh-helps-rust-manage-code-across-multiple-repositories/ ; https://rustc-dev-guide.rust-lang.org/external-repos.html
14. GitLab has no referrers endpoint. https://gitlab.com/gitlab-org/container-registry/-/issues/1205 ; https://gitlab.com/gitlab-org/container-registry/-/work_items/2375

## Negative / unverified

- GHCR, Docker Hub, Harbor, GAR, distribution referrers status: no primary source found.
- Whether any Rust crate fully verifies Rekor v2: not confirmed.
- Why the sibling project forked sigstore-rs: not found.
- Helm 4 cosign/`--verify` behaviour: not confirmed.
- Nix, Bazel, ORAS, uv, cargo-util, deno, jj, helix sharing practice: not researched or no source.
- `--new-bundle-format legacy` value: unconfirmed; flag form seen is `=false`.
- Some sources older than 18 months: 2023 Mitch talk, Docker 2023 post, Trail of Bits 2024 Homebrew posts, Sigstore RFC 3403 (2023). Re-verify.

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| https://oneuptime.com/blog/post/2026-08-11-cosign-signature-storage-oci-referrers-separate-repositories/view | Blog (secondary) | 2026-08 | Cosign storage models |
| https://opencontainers.org/posts/blog/2024-03-13-image-and-distribution-1-1/ | Spec blog | 2024-03 | Fallback tag duty |
| https://blog.sigstore.dev/rekor-v2-ga/ | Project blog | 2025-10 | Rekor v2 client changes |
| https://github.com/sigstore/sigstore-rs | Repo | 2026 | Crate status |
| https://docs.rs/crate/sigstore/latest | Docs | 2026-11-05 | Version, deps |
| https://github.com/sigstore/sigstore-rust | Repo | 2026 | Alt Rust workspace |
| https://docs.rs/sigstore-verify | Docs | 2026-11-09 | Version, owners |
| https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/verify-attestations-offline | Docs | 2026 | Offline verify |
| https://github.com/notaryproject/specifications/blob/v1.0.0/specs/trust-store-trust-policy.md | Spec | 2023 | Trust policy |
| https://github.com/fluxcd/flux2/pull/4727 | PR | 2024 | Flux keyless |
| https://mise.jdx.dev/dev-tools/backends/aqua.html | Docs | 2026 | mise verify |
| https://github.com/Homebrew/brew/issues/17019 | Issue | 2025-08 | Homebrew default |
| https://peps.python.org/pep-0740 | PEP | 2024 | PyPI attestations |
| https://github.com/rust-lang/rfcs/pull/3403 | RFC | 2023 | Cargo sigstore |
| https://blog.rust-lang.org/inside-rust/2026/06/04/how-josh-helps-rust-manage-code-across-multiple-repositories/ | Blog | 2026-06 | Sharing costs |
| https://rustc-dev-guide.rust-lang.org/external-repos.html | Docs | 2026 | Mechanism fit |
| https://gitlab.com/gitlab-org/container-registry/-/issues/1205 | Issue | 2024-02 | GitLab referrers |
| https://docs.sigstore.dev/cosign/system_config/registry_support/ | Docs | undated | Cosign-tested registries |
