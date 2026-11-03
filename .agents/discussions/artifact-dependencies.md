# Discussion: Dependencies between artifacts

State: handed-off → architect · Updated: 2026-11-01
Ratified: 2026-11-01 → architect
Confidence: ratified by the owner (explicit yes, 2026-11-01); research vintage 2026-11-01 (5 artifacts, expire 2027-05-01)

## Intent

The question as asked: "How difficult would it be to implement dependencies between skills, artifacts, and so forth?" The outcome is an ADR.

The user's framing: dependencies resemble bundles, except that every artifact can act as a bundle, and the mechanism applies to itself recursively.

## Requirements

- Any artifact kind can declare dependencies. A dependency must be present. There is no "recommends" tier.
- Resolution is transitive and recursive, with cycle detection and a total-node cap. This replaces the current flat bundle pass and its nested-bundle rejection.
- Removing an artifact also removes the dependencies nothing else needs, through the existing effective-set comparison (`src/lock/effective_set.rs`).
- The lock gains dependency provenance next to bundle provenance. The change is additive, following the dual-shape template of commit `2673bb2d`.

## Decisions

- **Model.** Flat edges with no version ranges and no solver. Every artifact can act as a bundle, and resolution reuses `expand_bundles` / `merge_bundle_members`.
- **Declaration.** An optional `requires` list in the artifact's source, published as the existing bundle members JSON layer (`application/vnd.grimoire.bundle.v1+json`) on that artifact. This reuses the existing fetch, size-cap and `MemberRef` hardening, relative refs included.
- **Pinned vs floating.** Inherited from bundles: floating by default, and `grim release --pin` freezes dependencies to digests at publish time.
- **Diamond.** When dependents disagree on a ref, resolution fails with `BundleConflict` (exit 78) and reports every conflict in one pass. A direct declaration overrides. This is the existing policy, unchanged.
- **Older grim.** Accept degradation once: grim ≤ the current release installs a dependent artifact without its dependencies. Ship a general forward-compatibility signal alongside dependencies, so that any later feature makes an older grim refuse with a clear "update grim" message. The signal is a `com.grimoire.requires-features` annotation listing feature names (`["dependencies"]`). A grim that does not know a listed feature refuses the artifact with a dedicated exit code and an "update grim" message. Precedent: git's `extensions.*`.
- **Cross-registry.** Allowed, matching bundle members, which already accept any absolute ref. `grim add` / `grim install` output shows the set that was pulled in.
- **Trust.** Already decided by the existing ADRs, not reopened. The declaration gesture grants trust that transitive pulls inherit (`.agents/adr/adr_artifact_trust_model.md:46-48`). Hook consent records the resolved set, so a hook that arrives transitively drifts the set and re-gates (`.agents/adr/adr_hook_workspace_consent.md` ~l.260).

## Threads

- **Pinned vs floating (closed: inherit `--pin`).** Bundles already answer this at publish time. Floating members re-resolve on each consumer `grim lock`. `grim release --pin` freezes them to digests (`docs/src/content/docs/concepts.md#bundle-pinning`). Candidate: dependencies inherit the same switch.
- **Recursion (closed: recursive).** Bundle expansion today is one flat pass with no cycle detection, and nested bundles are rejected (`src/resolve/resolver.rs` ~l.497). The only recorded reason is "no recursion in v1" (`src/oci/bundle.rs:46`) plus a resource cap (CWE-770). Treating every artifact as a bundle means recursive expansion, a visited set and a total node cap.
- **Old-grim behavior (closed: degrade once, then a forward signal).** An older grim would ignore a new dependency layer or annotation and install an incomplete set without saying so. The lock's `deny_unknown_fields` (`src/lock/locked_artifact.rs`) also makes an older grim reject a newer lock. A forward-compatibility story is needed within the additive-only freeze (`docs/src/content/docs/stability.md`).

## Out of scope

- Version ranges, minimum versions, and any solver (MVS, PubGrub).
- A recommends/suggests tier.
- System or binary dependencies (Chrome, pip packages, CLI tools).
- Proposing a dependency field to the Agent Skills spec. SKILL.md stays client-portable.
- Any new trust gate for transitive hooks or MCP servers (APM style). Existing ADRs already decide this.

## Research

- `.agents/research/research_artifact_dependencies_codebase.md`
- `.agents/research/research_artifact_dependencies_prior_art.md`
- `.agents/research/research_artifact_dependencies_vendor.md`
- `.agents/research/research_artifact_dependencies_history.md`
- `.agents/research/research_artifact_dependencies_demand.md`

## Related

- `src/resolve/resolver.rs`: `expand_bundles`, `merge_bundle_members`
- `src/lock/locked_artifact.rs`, `src/lock/locked_bundle.rs`, `src/lock/effective_set.rs`
- `.agents/adr/adr_effective_set_mutations.md`, `.agents/adr/adr_artifact_trust_model.md`, `.agents/adr/adr_hook_workspace_consent.md`
- `docs/src/content/docs/concepts.md#bundle-pinning`, `docs/src/content/docs/stability.md`

## Open questions

- [NEEDS CLARIFICATION: What does grim 0.15.x do with a skill manifest carrying a second, JSON members layer? Unknown media types fall back to raw tar (`src/oci/layer.rs:92-105`).]
  Recommended: test it against a local registry before the ADR commits to "degrades silently". If it fails loudly, the refusal comes for free.
- [NEEDS CLARIFICATION: Which exit code does a requires-features refusal use?]
  Recommended: an existing sysexits code from `.claude/rules/quality-rust-exit_codes.md` (65 data error or 78 config), not a new one. Choose by what the user must fix: their grim, not the data.
- [NEEDS CLARIFICATION: How large is the total-node cap for recursive expansion?]
  Recommended: reuse the 512-member cap as a whole-graph cap (CWE-770 rationale, commit `49944cd9`).
- [NEEDS CLARIFICATION: Where does `requires` live in source?]
  Recommended: a grim-namespaced key in frontmatter `metadata` (per `adr_tool_namespaced_metadata_rendering.md`), or a sidecar for kinds without frontmatter (MCP, hook). The ADR picks one per kind.
- [NEEDS CLARIFICATION: Do `status`/`describe` JSON and the TUI tree show dependency edges?]
  Recommended: yes, as additive always-present fields (`docs/src/content/docs/stability.md`).

## Verification

- Acceptance tests (`test/`, real registry) cover these cases:
  - transitive install of A→B→C;
  - a cycle A→B→A refused with a clear error;
  - a diamond conflict reporting exit 78 with every conflict listed;
  - a direct declaration overriding a dependency;
  - `grim remove A` dropping B and C unless something else needs them;
  - a `--pin` release freezing dependencies to digests;
  - relative dependency refs surviving a mirror;
  - an unknown `requires-features` entry refused with the chosen exit code and an "update grim" message.
- Upgrade fixture: a lock written by grim 0.15.1 reads unchanged, and a lock with no dependencies stays byte-identical (stability contract).
- Old-grim check: grim 0.15.1 installing a dependent artifact behaves as the ADR documents.
- `task verify` passes, along with `task catalog:verify` if a catalog skill adopts dependencies.
