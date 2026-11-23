# Research: history of ocx reuse in grimoire

## Metadata

**Date:** 2026-11-17
**Domain:** packaging
**Triggered by:** Decision on whether and by what mechanism grimoire reuses more ocx code, including ocx's signing/trust crates. Lane: repo archaeology (history only, no recommendation).
**Expires:** 2027-02-17 (ocx signing/trust churn is weekly; re-verify sooner)

## Direct Answer

Grimoire has borrowed from ocx three ways: copied-and-adapted source (since the first scaffold commits, 2026-05-15), design precedent studied and partly rejected (hook launcher, consent record, dependency "why"), and two shared git-submodule forks (`docker_credential` since 2026-06-01, `rust-oci-client` since 2026-07-06). It has never depended on an `ocx_*` crate. The two submodules cost 8 CI/release fixes-to-checkout lines and 4 pointer commits in 5.5 months, with no reverts found. ocx's signing/trust stack (`ocx_sign`, `ocx_trust`, plus a third fork `sigstore-rs`) is 3 months old, was reshaped by breaking commits as recently as 2026-11-13, and its own ADR records that satellites linking it was a design driver of the 2026-09-19 crate split. Grimoire's trust ADR records "no signature story" as accepted and says verification would arrive as a uniform fetch/resolve gate; no grimoire document weighs a shared crate or ocx signing code before the 2026-11-16 untracked research/discussion files.

## Item 1: borrows from ocx into grimoire (timeline)

| Date | SHA / path | What | Mechanism | Recorded reason |
|---|---|---|---|---|
| 2026-05-15 | `cdf17b89` | Identifier, Digest, PinnedIdentifier, exit codes | copied, adapted | "Adapted from OCX oci/{identifier,digest,pinned_identifier}.rs" |
| 2026-05-15 | `866d790e` | config, lock, atomic store (`src/config.rs:8`, `src/lock.rs:7`, `src/store/atomic_write.rs:6`) | copied, adapted, trimmed | header comments "Adapted from OCX project::{config,lock}" |
| 2026-05-15 | `3d2ed571` | OciAccess seam, cache, resolve (`src/oci/access.rs:7`, `registry_client.rs:6`, `resolve.rs:6`) | re-implemented from design, collapsed (no chained multi-source index) | "Adapted from OCX oci/index.rs + project/resolve.rs" |
| 2026-05-15 | `f47c39b6` | scaffold; ocx toolchain use | process | none beyond scaffold |
| 2026-06-01 | `044f6a13` | `external/docker_credential` submodule + `[patch.crates-io]` | submodule fork (ocx-sh) | upstream crate only reads; login/logout need store/erase (`Cargo.toml:144-150`) |
| 2026-06-01 | `9f2c2343`, `c0b3cf64`, `c95ed422` | publish grim to ocx.sh with ocx tooling | tooling use (plan `.agents/plans/archive/plan_ocx_publish.md`) | dogfood distribution |
| 2026-07-06 | `d44bee50` | `external/rust-oci-client` submodule (branch `ocx/integration`) | submodule fork | upstream `Client::default()` panics on empty trust store; "mirroring the docker_credential pattern" |
| 2026-07-12 | `f016e419` | `.tar.gz` release archives | ocx consumer constraint | consumer workflows (publish-ocx) read archive names |
| 2026-09-06 | `069bfe38` | shared ocx agentic artifacts | config copy | not read in detail |
| 2026-10-04 | `82b6c9f4`, `src/hook/consent.rs:40-55` | consent record: five details | design borrowed, re-implemented | "The sibling OCX project shipped this shape first (`crates/ocx_lib/src/project/consent.rs`)" |
| 2026-10-26 | `d992ffdf`, `ef16d1a2`, `.agents/adr/adr_hook_native_dispatcher.md:99-112` | native hook launcher | design studied, build mechanism **rejected** | committed prebuilt blobs "solve a problem grimoire does not have"; size floor and recency gate cost; operational patterns adopted |
| 2026-11-01 | `2ee3ff28`, `.agents/research/research_artifact_dependencies_ocx_why.md` | dependency model, `--why` views | design studied | feeds `adr_artifact_dependencies.md:46,166` |
| 2026-11-16 | untracked | `research_ocx_reuse_codebase_recon.md`, `research_oci_signing_and_code_sharing_prior_art.md`, `discussions/ocx-adoption-and-signing.md` | current exploration | this decision |

Also `src/oci/identifier.rs:6`, `pinned_identifier.rs:6`, `digest.rs:6`, `tag_cache.rs:6`, `store/paths.rs:6`, `resolve/resolve_options.rs:6`, `config/declaration.rs:8` carry "Adapted from OCX" headers. No `use ocx_` or path dependency exists anywhere.

## Item 2: submodule cost

Grimoire (`git log -- external .gitmodules`, 4 commits total):
- `044f6a13` 2026-06-01 add docker_credential; `d44bee50` 2026-07-06 add rust-oci-client; `23b1b7a5` and `fa92f264` 2026-10-18 move the rust-oci-client pointer (pagination fixes). docker_credential pointer never moved after add.
- Friction commits: `4d83a1f1` 2026-06-01 build jobs fail on empty `external/docker_credential` until `submodules: recursive` added; `43437015` 2026-07-26 supply-chain job failed on first run for the same reason (author: "I copied the checkout without it"). `.github/workflows/{verify-basic,verify-deep,docs,release}.yml` each carry the checkout line (about 17 sites). `taskfiles/p9.taskfile.yml:65-74` special-cases `git archive` skipping submodules.
- Pin drift: local checkout reads `feature/catalog-63-g7ad928e`; per the recon file, grimoire pin `7ad928e` and ocx pin `3ee5a1f` are not ancestors of each other (both on `origin/ocx/integration`).
- No revert, no cargo-dist breakage attributable to submodules found; release.yml already checked out submodules.

ocx (`git log -- external .gitmodules`):
- Three submodules, all `ignore = untracked`, branch `ocx/integration`, listed in `[workspace] exclude` (`ocx:Cargo.toml:10`).
- oci-client fork pointer moves: `302db117` 2026-03-12 rebase onto upstream; `6214aec0` 07-06, `b434e1c6` 07-09, `d697600c` 08-23 ("rebased auth-cache branch"), `b9f8b7fa` 08-30. Rebase ledger records a conflict re-evaluation (`ocx:.claude/artifacts/rebase_ledger_signing_and_trust.md`).
- sigstore-rs: added once, `d80c2e8e` 2026-09-15, pinned at a seam commit adding `SigstoreTrustRoot::new_with_client`; fork PR "must be fast-forwarded before this series merges, because the submodule pins its commit". Pointer never moved since (1 commit in `git log -- external/sigstore-rs`). The submodule is uninitialised in the local ocx checkout, so fork contents beyond that commit are unread.
- ADR for sigstore delegation rejected "Fork sigstore-rs and land TODOs upstream first" at the time as blocking the milestone (`ocx:.claude/artifacts/adr_real_sigstore_stack_and_delegation.md:114`); an earlier ADR called a fork "a tax" (`adr_oci_referrers_signing_v1.md:445`). The fork was later added anyway for a TLS/TUF seam (ocx#470).

## Item 3: divergence

- Adapted-then-diverged code (`src/config.rs`, `lock.rs`, `resolve.rs`): per recon file, `ocx_project` is internal and the types differ now. Exit codes share 0/1/64/65/69/74/75/77/78/79/80, then diverge (grimoire 81 = OfflineBlocked; ocx 81+ reshaped in `ocx:adr_exit_code_taxonomy.md`).
- `src/oci/access.rs:8`: "OCX chained multi-source index, platform/variant..." deliberately dropped.
- Hook launcher: ocx blob embedding rejected for build-mechanism reasons (ADR `adr_hook_native_dispatcher.md:99-112`). ocx "never replaces a running exe"; grimoire has one registered path, so the trick does not transfer.
- `src/resolve/resolve_error.rs:8` "mirroring OCX's precedent" boxed error kind: tracked, not diverged.
- ocx's tag-range-free pinned-dependency model (`research_artifact_dependencies_ocx_why.md`) informed grimoire's ADR; no recorded divergence reason found in that ADR beyond scope.

## Item 4: prior consideration of shared crate or signing in grimoire

- `.agents/adr/adr_artifact_trust_model.md:155-159` "When signatures arrive": verification becomes a gate at fetch/resolve for every kind with a strictness policy, not a per-kind prompt; "Revisit this ADR then". `:173`: absence of a signature story "known, accepted gap ... tracked as its own work".
- `cca2745c` 2026-07-26 SECURITY policy states the no-signing boundary; `a4faf3fc` 2026-10-22 release build provenance attestations (grimoire's own releases, not package verification); `bd4a102d` 2026-08-26 trust model recorded.
- `.agents/plans/meta-plan_promotion_1_0.md:14-15`: grimoire verifies no signatures; every cosign/sigstore grep hit was a false positive. `research_promotion_positioning.md:101` notes skillctl has Sigstore signing as a competitor signal.
- Shared-crate idea: no tracked ADR/plan found. ocx side did consider it: `ocx:.claude/artifacts/adr_crate_split_workspace.md:64` driver D2 "Can `grimoire` link the signing stack without naming a product-only crate, and at what compile cost?"; it states consumers are "lockstep submodule path dependencies" and satellites must not name `ocx_store` (`:291,323`); "Full-tree git-submodule vendoring is a hard requirement" because cargo cannot depend on individual crates inside a git repo as path deps (cargo#14946, `:472-474`). `ocx_sign` depends on `ocx_oci`, `ocx_trust`, `ocx_util`, `ocx_env`, `ocx_exit`; `ocx_trust` on `ocx_util`, `ocx_env`.
- ocx PRD (`prd_release_signing_trust_default.md:10`) names grimoire as an "other own product" whose releases should be signed under a stable per-family identity.

## Item 5: ocx signing/trust timeline

| Date | SHA | Event |
|---|---|---|
| 2026-04-19 | `adr_oci_referrers_signing_v1.md:448` | ADR: no sigstore-rs DSSE support, fork = tax |
| 2026-08-18 | `1243a8a4` | `feat(sign)!`: keyless Sigstore signing and identity-pinned verification over OCI referrers (ocx v0.6.0 on 2026-08-31 per CHANGELOG:429) |
| 2026-08-18 | ADR real_sigstore_stack | delegate crypto to sigstore-rs, "delete, do not add" |
| 2026-09-15 | `d80c2e8e` | sigstore-rs vendored as submodule |
| 2026-09-19 | `b79abbe8` | `refactor!` dissolve `ocx_lib` into 17 crates, producing `ocx_sign`, `ocx_trust` |
| 2026-10-10 | `737c870c` | OIDC transient failure maps to exit 75 |
| 2026-10-25 | `e8204d7b` | Bazel cross-build |
| 2026-11-13 | `2e375085` | `feat(trust)!`: verify packages against publishers declared in the index |
| 2026-11-14 | `7405c7a1`, `9c17fcc8`, `582e4352`, `3f8e4b0f` | release binaries from stamped Bazel builds; index-signing fix; verify UX; idempotent re-sign |

Last 60 days (since 2026-09-17): 18 commits touch `crates/ocx_sign`, `crates/ocx_trust`, `external/sigstore-rs` of 333 total repo commits; 5 carry `!`. Status: shipped in releases, not labelled experimental in source or CHANGELOG; no default trust policy exists (`prd_release_signing_trust_default.md:49`: `Config::default().trust_policies()` empty, install skips verification with INFO), so verification is opt-in per policy; docs roadmap lists "Signature discovery: Planned" (`ocx:website/src/content/docs/roadmap.md:52`). Open problems: Rekor v2 unsupported pending sigstore-rs (recon file, spike issue #107); the PRD records unsigned own releases as the motivating gap. Non-initialised sigstore-rs submodule limited fork-content inspection.

## Item 6: release coupling

- grimoire: cargo-dist (`f97307f7` 2026-06-01), 31 tags, v0.15.1 on 2026-10-28; cadence about 2 per month, 0.1.0 to 0.15.x in 5 months. Release builds check out submodules (`release.yml:61,125,190,259,324`).
- ocx: v0.5.6 2026-08-06 through v0.6.8 2026-11-15; a 36-day gap between v0.6.5 and v0.6.6; release is Bazel-stamped since `7405c7a1` 2026-11-14, so ocx no longer uses cargo-dist (no `dist-workspace.toml`) while grimoire does.
- Cross-repo release blocking: not found in either direction. ocx's own coupling mechanism is `task satellite:verify` (`b6abe75f` 2026-10-10) checking `../ocx-mirror` builds against its ocx pointer; its release note `ca1e66aa` shows the readiness run is no longer required before a bump. Fork PR ordering is the one recorded gate ("must be fast-forwarded before this series merges", `d80c2e8e`).
- Grimoire's published install path itself depends on ocx (installs via `ocx.sh/grimoire/cli`, `38800825` 2026-08-01; setup via ocx). Renames track ocx (`629af05a`, `e0e96da9`).

## Sources

| Source | Type | Date | Relevance |
|---|---|---|---|
| grimoire `git log` (SHAs above), `Cargo.toml:144-161`, `.gitmodules`, `src/**` headers | Repo | 2026-11-17 | items 1-3, 6 |
| `.agents/adr/adr_artifact_trust_model.md`, `adr_hook_native_dispatcher.md` | ADR | 2026-08/10 | items 3-4 |
| `.agents/research/research_hook_dispatcher_ocx_shim_precedent.md`, `research_artifact_dependencies_ocx_why.md`, `research_ocx_reuse_codebase_recon.md` | Research | 2026-10/11 | items 1,3 |
| `ocx:.claude/artifacts/adr_crate_split_workspace.md`, `adr_real_sigstore_stack_and_delegation.md`, `prd_release_signing_trust_default.md`, `rebase_ledger_signing_and_trust.md` | ADR/PRD | 2026-08/11 | items 2,4,5 |
| `ocx:CHANGELOG.md`, ocx `git log` | Repo | 2026-11-16 | items 5,6 |
