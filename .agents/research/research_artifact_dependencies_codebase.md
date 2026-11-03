# Research: Artifact dependencies — codebase recon

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/discussions/artifact-dependencies.md` — what declared inter-artifact dependencies would touch in grim
**Expires:** 2027-05-01

## Direct Answer

No dependency edge exists today. Bundles are the only transitive mechanism. Their expansion is a single flat pass, and nested bundles are rejected. The lock records bundle provenance only. Bundle publication already offers a floating-versus-pinned choice (`grim release --pin`).

## Findings

**Bundle expansion (`src/resolve/resolver.rs`)**
- The flow is `build_work` (~l.300), then `expand_bundles` (l.447), then `merge_bundle_members` (l.372).
- Conflict policy (doc comment ~l.296):
  - A direct declaration wins over any bundle.
  - Bundles that agree on an identifier coalesce, and every contributing bundle is recorded as provenance.
  - Bundles that disagree fail with `BundleConflict` (exit 78). All conflicts are collected in one pass.
- Nested bundles are rejected at ~l.497-503. Expansion has no recursion, no visited set and no cycle detection.
- Recorded rationale is thin:
  - `src/oci/bundle.rs:46` says "no recursion in v1".
  - Commit `49944cd9` lists the rejection under untrusted-registry hardening, next to the 512-member cap (CWE-770).
  - No ADR weighs nesting as a design question.
- The fetch path is hardened against untrusted data (size cap, name validation): `fetch_bundle_members`. `src/oci/member_ref.rs` resolves relative member ids against the bundle's own id.

**Lock (`src/lock/`)**
- `LockedArtifact.bundles: Vec<BundleProvenance{repo, tag}>` (`locked_artifact.rs:12-64`) holds bundle provenance. `RawLockedArtifact` uses `deny_unknown_fields`, so an older grim rejects new lock keys.
- `LockedBundle` (`locked_bundle.rs:79-85`) is the `[[bundle]]` member-list cache.
- `effective_set.rs` (`adr_effective_set_mutations.md`) computes before/after desired sets offline. It drops `E_before \ E_after` and re-derives provenance.
- `grim remove` and `uninstall` (`uninstall.rs:92`) keep files a declared bundle still provides. This is the orphan-removal mechanism.
- No non-bundle edge type exists. Only `set.bundles` is read.
- Lock version `V1` (`lock_version.rs`).

**Metadata**
- Annotations are `com.grimoire.{kind,keywords,summary,compatibility,deprecated,replaced-by}` (`src/oci/annotations.rs`).
- `compatibility` is free text and never parsed.
- No `requires` or `depends` field exists anywhere.
- A bundle's members live in a JSON layer, `application/vnd.grimoire.bundle.v1+json`.

**Versions**
- References are floating `registry/repo:tag` or digest `@sha256:`. The lock pins manifest digests.
- The `semver` crate is used only for tag cascade and sort (`oci/release.rs:154-197`, `command/publish.rs:673-748`, `tui/app.rs:2368`). There is no range matching.
- `docs/src/content/docs/concepts.md#bundle-pinning`: floating members re-resolve on every consumer `grim lock`, while `grim release --pin` freezes them to digests at publish time.

**Trust**
- `adr_artifact_trust_model.md:46-48`: the declaration gesture grants trust. "everything that gesture transitively pulls in inherits it. Transitivity is the bundle feature."
- `adr_hook_workspace_consent.md` (~l.260):
  - The consent record stores the resolved set, so a hook member that drifts re-gates.
  - `install` and `update` never write consent (T3).
- `arch-threat-model.md` has no text on bundles or cross-registry pulls.

**Stability (`docs/src/content/docs/stability.md`)**
- Frozen: `grimoire.toml` and `grimoire.lock` schema, bundle source manifest, `--format json`, and `com.grimoire.*` annotations.
- Evolution is additive only: new optional fields, always-present nulls in reports, widening inputs.

**Issues and plans:** none concern artifact dependencies.

negative:
- No rationale document for the nesting rejection.
- No dependency edge, range parser or cycle detector.
- `compatibility` is unparsed prose.

leads:
- `adr_effective_set_mutations.md`
- `bugfix_plan_shared_bundle_members.md`
- `src/oci/member_ref.rs` (relative refs)
