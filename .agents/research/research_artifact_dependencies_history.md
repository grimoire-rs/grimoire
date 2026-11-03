# Research: Artifact dependencies — composition history in this repo

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/discussions/artifact-dependencies.md` — repo archaeology lane
**Expires:** 2027-05-01

## Findings

**Nested-bundle rejection**
- It originates in commit `49944cd9` (2026-06-01). That commit lists the rejection under untrusted-registry hardening, next to the 512 KiB / 512-member caps (CWE-770), with no design trade-off discussed.
- Code: `src/oci/bundle.rs:46` ("no recursion in v1"), `src/resolve/resolver.rs:503`.
- Commit `171c3290` added a second guard in the TUI tree.
- There was no prior attempt at nesting and no revert.

**Shared-member provenance** (`.agents/plans/bugfix_plan_shared_bundle_members.md`, fix `2673bb2d`)
- **Bug.** The lock held one provenance per member, so removing that bundle evicted a member another bundle still declared, and the result depended on removal order.
- **Fix.** `LockedArtifact.bundles: Vec<BundleProvenance>`. A member is dropped only when its list is empty.

**Effective-set mutations** (`.agents/adr/adr_effective_set_mutations.md`, implemented in `2abec216`)
- **Bug.** A direct declaration masked a bundle's contribution.
- **Fix.**
  - Each bundle's expansion is cached in the lock's `[[bundle]]` section.
  - `E(declaration, cached_bundles)` is computed offline.
  - Origin is `Direct | Bundles{id, contributors} | Conflicted`.
- An older grim rejects a lock carrying `[[bundle]]`.

**Floating vs pinned**
- `grim release --pin` arrived in `49944cd9`, to freeze a bundle for reproducible, tunnel-safe consumption.
- Floating is the default, re-resolved on each consumer `grim lock`.
- Docs: `docs/src/content/docs/concepts.md:179`, `docs/src/content/docs/publishing.md:840`.

**Relative member refs** (`src/oci/member_ref.rs`, commit `54fd29d1`, fixes #31)
- `./y:1` and `../skills/x:0` resolve against the bundle's deployed repo, so a bundle survives mirroring.
- The lock stores absolute ids, and `--pin` freezes them to absolute form.
- `catalog/bundles/grim-essentials.toml` uses this form.

**Lock evolution template** (`2673bb2d`, `910dc312`)
- One provenance keeps the legacy `bundle` + `bundle_tag` pair, so existing locks stay byte-identical. Two or more provenances emit a `bundles` array.
- No lock version bump was needed (still `V1`).
- `docs/src/content/docs/stability.md:415-460`: an older grim exits 78 on newer lock fields, but only when the feature is used.

negative:
- No rationale for the nesting rejection beyond "v1" and the resource cap.
- No cycle detection.
- No plan or ADR covers artifact-level dependencies.

leads:
- `git show 2673bb2d` (the dual-shape lock wire format, as a template).
- `.agents/adr/adr_hook_workspace_consent.md` (~l.260): consent covers the resolved set.
