# Discussion: Adopting more of ocx-sh/ocx, and signing support

State: active · Updated: 2026-11-17

## Intent

The owner asked how reasonable it is to adopt more of the `ocx-sh/ocx`
repository in grimoire, to consolidate the implementation and to support
signing. Target outcome of this discussion: an ADR.

Out of scope: replacing grimoire's existing OCI access, content store, lock,
index or exit-code layers with ocx crates before 1.0; linking today's
`ocx_sign` or `ocx_trust` crates.

## Requirements

Provisional. The design is inherited from ocx; the full inventory, with ocx
references, is `.agents/research/research_ocx_signing_design_inventory.md`.

Inherited from ocx unchanged:

- A signature is a Sigstore bundle v0.3 holding a DSSE in-toto Statement over
  the manifest digest, P-256 and sha256, readable by stock `cosign verify`.
- It is pushed as an OCI referrer. On a registry with no Referrers API it is
  written into the `sha256-<hex>` fallback index, with read-back and retries.
  A per-registry capability result is cached.
- Legacy cosign sidecar tags are read: a `sha256-<hex>.sig` tag goes through
  the same trust gate as a bundle, and a keyless sidecar needs transparency
  log evidence. A `.sig` sidecar is written only on request.
- Verification runs after the manifest digest resolves and before any layer
  download. It is ANY-of over a bounded candidate set. The signing instant
  must fall inside the certificate validity window.
- Two rounds must both pass: the publishers the index declares for the
  package, then the operator's own trust policies.
- Strictness is per registry: `if_declared`, `required`, `off`. A package
  neither declared nor covered by a policy installs unverified with an
  informational line.
- Keyless signing takes its identity token from a file, standard input,
  ambient CI, or a browser login. Key-based signing does not upload to the
  transparency log by default; keyless always does. Signing twice with the
  same identity adds no second referrer.
- Trust material comes from a ladder ending in the Sigstore public-good root
  over TUF, cached with a time limit. Offline mode removes only the Sigstore
  network, never the registry, and fails closed without a pinned log key.
- Tests run against a self-hosted Fulcio, Rekor, CT log and Dex stack with a
  committed trusted root, plus golden bytes from real cosign.

Where grimoire deviates, because one of its own contracts forces it:

- **No environment variable skips verification or supplies trust.** ocx has
  `OCX_NO_VERIFY`, an environment-set trusted root, and `env://` verification
  keys. grimoire's rule (`AGENTS.md` › Environment Variables, last row) is
  that the environment may make a run stricter and never looser, because a
  repository carries its own environment. grimoire gets a `--no-verify` flag
  only. An environment reference stays allowed for a signing secret, which is
  a credential and not a trust input.
- **Exit codes are grimoire's own.** ocx reports a blocked policy as 81 and
  uses 77; grimoire's 81 is `OfflineBlocked` and its 77 is a filesystem
  permission error (`src/cli/exit_code.rs`). The shared crate carries no exit
  codes; it returns classified errors and each tool maps them. grimoire adds
  new codes, never reuses a taken one.
- **A project file may raise strictness only.** As in ocx, a
  repository-carried trust policy does not gate install. In grimoire a
  project `grimoire.toml` may additionally set a registry to `required`.
- **Out of scope in the first release:** `.att` sidecars, provenance
  attestations, SBOMs, GitHub artifact attestations, Notation.

Files and interfaces this touches in grimoire: `src/fetch.rs` and
`src/resolve/resolver.rs` (the gate), `src/oci/access.rs` and
`src/oci/access/registry_client.rs` (referrers, fallback index, sidecar
tags; the transport trait implementation), `src/oci/tag_cache.rs` (version
enumeration must not read `sha256-*` tags as versions), `src/command/publish.rs`
(signing), `src/config/declaration.rs` (trust policy and `signatures` keys),
`src/catalog/index_package.rs` and `docs/src/content/docs/package-index.md`
(the `publishers` field), `src/cli/exit_code.rs`, `Cargo.toml` and
`deny.toml` (the new dependency), `docs/src/content/docs/stability.md`,
`AGENTS.md` (environment table), `.claude/rules/arch-threat-model.md` and
`.agents/adr/adr_artifact_trust_model.md` (revisited, as that ADR requires).
Outside grimoire: a new repository for the shared crate; later, ocx's
`crates/ocx_sign`.

## Decisions

Working positions, not yet ratified.

- **Signing is built on `sigstore/sigstore-rust`, not on ocx's crates.**
  grimoire's sign and verify path uses the `sigstore-rust` crate family
  (`sigstore-verify`, `sigstore-sign`, `sigstore-rekor`, `sigstore-tuf`,
  `sigstore-trust-root`, `sigstore-tsa`; 0.16.0, 2026-11-09) and is driven
  through grimoire's own `OciAccess`. Owner's choice, 2026-11-17, with the
  council majority. ocx is a design reference: trust-policy model, strictness
  vocabulary, offline trust cache, the self-hosted Sigstore test stack.
- **Existing layers stay.** grimoire's `src/oci/`, store, lock, index and
  exit codes are not replaced by ocx crates before 1.0. Owner's choice,
  2026-11-16.
- **First release signs and verifies.** `grim publish` signs; install-side
  commands verify. Owner's choice, 2026-11-17.
- **Keyless and key-based from the first release.** Owner's choice,
  2026-11-16. `sigstore-verify` covers both on the verify side
  (`VerificationPolicy`, `PublicKeyVerificationPolicy`). `sigstore-sign`
  documents keyless signing only, so key-based signing would be grimoire's
  own code (see Threads).
- **The OCI-artifact sign and verify logic lives in a shared,
  transport-neutral crate in its own repository from day one.** Owner's
  choice, 2026-11-17. The crate sits on `sigstore-rust`, names no grimoire or
  ocx type in its API, and reaches the registry through a transport trait its
  consumer implements. grimoire is the first consumer. Porting ocx onto it is
  the intended end state, so both tools run one verifier, but it is ocx's own
  follow-up and does not gate grimoire. ocx's existing `VerifyContext`
  (`ocx:crates/ocx_sign/src/verify/pipeline.rs`) is the checklist for what a
  second consumer needs.
- **grim signs with publisher-held keys as well as keyless.** Owner's choice,
  2026-11-17. `sigstore-sign` documents keyless signing only, so the
  managed-key bundle construction and key handling (key files, passwords,
  environment references) are code this work owns.
- **Trusted signer identities come from the index plus the user's global
  config.** Owner's choice, 2026-11-17. The index declares each namespace's
  publisher identity; the user's global config can add or override; a
  project's `grimoire.toml` may only make verification stricter, never widen
  trust. No environment variable and no committed file grants trust.
- **ocx's signing design is the baseline.** Owner, 2026-11-17: ocx has
  already answered these design questions, so grimoire's design adheres to
  the ocx implementation, including how registries without a referrers API
  are handled and how sidecars are evaluated. grimoire deviates only where
  one of its own contracts forces it (exit-code numbers, the environment
  policy, config and index schemas). What changes is the library underneath
  (`sigstore-rust`) and the transport seam, not the design.
- **Default strictness: verify if declared.** Owner's choice, 2026-11-17. A
  namespace with a declared signer is verified and fails closed; a namespace
  with none installs as today. Same vocabulary as ocx
  (`if_declared | required | off`).
- **First release covers signatures only.** Owner's choice, 2026-11-17.
  Provenance attestations, SBOMs, GitHub artifact attestations and Notation
  are out of scope.
- **Success is judged by** interoperation with cosign and ocx in both
  directions, and by working on registries that have no referrers API.
  Owner, 2026-11-17.
- **Why now:** close the accepted signature gap before the 1.0 contract
  freezes, and tell one supply-chain story with ocx, which shipped signing on
  2026-08-18. Owner, 2026-11-17.
- **Shape of the gate is already fixed** by
  `.agents/adr/adr_artifact_trust_model.md` § "When signatures arrive": one
  verification gate at fetch/resolve time, uniform across kinds, with a
  strictness policy. Not reopened here.

## Threads

- **Closed: linking ocx's signing crates as built.** `ocx_trust` depends
  only on `ocx_util`, `ocx_env` and `sigstore`; `ocx_sign` hard-depends on
  `ocx_oci`, `ocx_env` and `ocx_exit`, and every entry point takes
  `ocx_oci::Client`. Linking it would put a second OCI client, the `OCX_*`
  environment registry and ocx's exit-code taxonomy (ocx 81 `PolicyBlocked`,
  grimoire 81 `OfflineBlocked`) inside grim. ocx's signing crates also took
  5 breaking reshapes in the 60 days to 2026-11-13. Resolved by the first
  decision above.
- **Closed: council on the mechanism.** Three seats. All rejected replacing
  grimoire's layers and rejected copying ocx code. Two (operability,
  simplicity) favoured a native verifier; one (premortem) favoured a narrow
  ocx dependency, because a home-grown verifier fails silently. The premortem
  objection rested on grimoire re-writing the transparency-log checks that
  `sigstore` 0.14 leaves undone (ocx's `verify/pipeline.rs` is 8.8k lines for
  that reason). `sigstore-rust` ships those checks as its own crates
  (`sigstore-merkle`, `sigstore-rekor` with `RekorV2Client`, `sigstore-tsa`)
  and has identity matching built in, which removes most of that risk.
- **Closed: what ocx's `sigstore-rs` fork carries.** One recorded purpose,
  `SigstoreTrustRoot::new_with_client`. Moot for grimoire under the first
  decision.
- **Closed: one shared signing crate for both tools.** Raised by the owner,
  2026-11-17, to avoid two verifiers on two Sigstore stacks (ocx on a fork of
  `sigstore-rs` 0.14, grimoire on `sigstore-rust` 0.16). Resolved as its own
  repository from day one; see Decisions. Trade accepted: a third CI and
  release train exists before the second consumer does.
- **Open: maturity of `sigstore-rust`.** Owned by the sigstore organisation,
  0.16.0, about 30 stars, no stated stability level, no conformance status on
  its README. Rekor v2 is present as types (`RekorV2Client`,
  `HashedRekordV2`, `RekorV2Tile`); whether `sigstore-verify` accepts a
  Rekor v2 bundle end to end was not confirmed. It has no OCI code: referrers
  push and list, and the `sha256-<hex>` fallback tag, are grimoire's to
  write.
- **Open: registry referrers support.** GHCR, GitLab and Docker Hub are
  reported as fallback-tag only or unclear, from secondary sources. Working
  without referrers is a success criterion; ocx's handling is the baseline.
- **Closed: default strictness.** Verify if declared; see Decisions.
- **Closed: where trusted identities are declared.** Index plus user config;
  see Decisions. Comparable tools: catalog-declared verification with user
  opt-out (aqua, mise), privilege-gated widening (Nix). ocx moved to "verify
  against publishers declared in the index" on 2026-11-13. Accepted limit: a
  compromised index can redirect trust, as it can already redirect
  repositories.

## Research

- `.agents/research/research_ocx_reuse_codebase_recon.md` — crate map,
  overlap inventory, signing as built in ocx, mechanics.
- `.agents/research/research_oci_signing_and_code_sharing_prior_art.md` —
  signing schemes, registry referrers support, Rust libraries, comparable
  tools, code-sharing mechanisms.
- `.agents/research/research_ocx_reuse_history.md` — every past borrow from
  ocx, submodule cost, ocx signing timeline.
- `.agents/research/research_signature_policy_ux_comparables.md` — defaults,
  identity location, strictness vocabulary, lock interaction in comparable
  tools.
- `.agents/research/research_ocx_signing_design_inventory.md` — every
  signing decision ocx recorded, with the grimoire contract each one touches.
- Inline check, 2026-11-17: `sigstore-rust` 0.16.0 crate docs on docs.rs
  (`sigstore-verify`, `sigstore-sign`, `sigstore-rekor`) and the repository
  README. grimoire already uses `reqwest` 0.13, which `sigstore-rust`
  requires.

## Related

- `.agents/adr/adr_artifact_trust_model.md` — the trust boundary and the
  decided shape of the verification gate.
- `.claude/rules/arch-threat-model.md` — attackers in scope, non-goals.
- `docs/src/content/docs/stability.md` — the additive-only contract.
- `.gitmodules`, `Cargo.toml` — the two existing `ocx-sh` fork submodules.
- ocx side: `ocx:crates/ocx_sign`, `ocx:crates/ocx_trust`,
  `ocx:AGENTS.md` (ecosystem tier), `ocx:test/sigstore` (self-hosted
  Sigstore acceptance stack).
- https://github.com/sigstore/sigstore-rust — the chosen Sigstore library
  family.

## Open questions

- [NEEDS CLARIFICATION: the index `publishers` list is not authenticated. An
  attacker who controls the index can replace a declared signer, or delete
  the declaration so that `if_declared` installs the package unverified. ocx
  records this as a partly mitigated threat and has a signed catalog
  designed, with no code. Does grimoire ship with the same limit?]
  Recommended: yes, inherit it and name it in the threat model — an operator
  who needs more sets the registry to `required` with their own policy, and
  the signed catalog follows ocx's design when it exists.
- [NEEDS CLARIFICATION: `sigstore-rust` is at 0.16.0 with no stated stability
  level. Is it mature enough to carry verification, and does
  `sigstore-verify` accept everything the inherited design needs (bundle
  v0.3 with DSSE, key-based bundles, an injected trust root, a private
  Sigstore instance)?] Recommended: prove it with a spike against ocx's
  golden cosign bytes before the design is fixed — a failed spike reopens the
  library choice, not the design.
- [NEEDS CLARIFICATION: Rekor v2. ocx supports v1 only. `sigstore-rust` has
  v2 client types; end-to-end verification of a v2 bundle was not confirmed.]
  Recommended: v1 in the first release, v2 accepted when the library proves
  it — additive.
- [NEEDS CLARIFICATION: how grimoire consumes the shared crate: a git
  dependency pinned by revision, a submodule, or crates.io.] Recommended: a
  git dependency pinned by revision — the crate needs no `[patch]`, so it
  avoids a third submodule; publish to crates.io once ocx has ported and the
  API has held.
- [NEEDS CLARIFICATION: which exit codes the new failures take.]
  Recommended: one new code for a failed or missing required signature and
  the existing offline code for missing trust material offline — 82 is free.
- [NEEDS CLARIFICATION: grimoire's config types reject unknown fields, where
  ocx's trust types tolerate them. Which rule do the new trust keys follow?]
  Recommended: grimoire's existing rule, for consistency with every other
  key.
- [NEEDS CLARIFICATION: what `grim publish` signs for a bundle and its
  members, and whether any grimoire artifact is a multi-platform index.]
  Recommended: sign each pushed manifest digest; settle the bundle case in
  the design.
- [NEEDS CLARIFICATION: ocx's ADRs and code disagree in eight places (listed
  in the inventory). Which is the baseline where they differ?] Recommended:
  the code, since it is what ocx ships and tests.
- [NEEDS CLARIFICATION: registries without a Referrers API were tested by ocx
  on zot, `registry:2` and GHCR only.] Recommended: add GitLab and Docker Hub
  to grimoire's live probes before release.

## Verification

- `task verify` passes, and `task docs:check` for the documentation pages.
- An upgrade fixture proves that a project with no declared signer installs,
  updates and reports status exactly as before (Principle 9).
- Acceptance tests under `test/tests` cover: verify on a registry with a
  Referrers API and on `registry:2` without one; keyless and key-based; a
  wrong signer, a missing signature under `required`, and a `.sig` sidecar;
  offline with and without cached trust material.
- Interoperation, both directions: stock `cosign verify` accepts what grim
  signs; grim accepts what cosign and ocx sign.
- The keyless path runs against a self-hosted Sigstore stack ported from
  `ocx:test/sigstore`.
- The `grim-hook` dependency allowlist test still passes: no signing
  dependency reaches the launcher.
- The release matrix builds, including musl and Windows. Windows proof on
  this host needs a CI deep run.
