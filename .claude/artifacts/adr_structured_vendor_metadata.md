# ADR: Object-valued vendor metadata rendering — inline-parsed metadata values for allowlisted keys

## Metadata

**Status:** Proposed
**Date:** 2026-07-17
**Deciders:** Michael Herwig (maintainer)
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (Rust 2024, no new dependency — `serde_yaml`, already the projection
      engine's parser, both parses and re-serializes the inline value)
**Domain Tags:** integration, api
**Supersedes:** N/A

## Context

`adr_tool_namespaced_metadata_rendering.md` chose namespaced string keys
inside the agentskills `metadata` map (`<vendor>.<field>: "value"`) as the
sole mechanism for authoring vendor-specific capabilities, and deliberately
scoped v1 to **scalar** values. That decision is encoded directly in
`src/install/vendor.rs`: [`FieldType`][vendor-fieldtype] has exactly six
variants — `Bool`, `String`, `Enum(&'static [&'static str])`, `Integer`,
`Float`, `CommaList` — and every one of them converts a metadata string into
a YAML scalar or a flat sequence. The conversion itself happens in
[`convert`][render-convert] (`src/install/render.rs`), a single `match ty`
with one arm per `FieldType` variant, called from
[`partition_metadata`][render-partition] for every known `<vendor>.<field>`
key found in a skill, rule, or agent's `metadata` map.

That scalar-only design was deliberate and correct for its scope — see the
prior ADR's rejection of Option 1 (typed top-level vendor fields) precisely
because vendor capabilities diverge too fast for a Rust type-model change per
release. But it also means grim currently **cannot project any object-shaped
vendor capability**, because there is no `FieldType` variant that accepts a
structured value. Three known object-valued fields exist across the
supported clients:

- **OpenCode agent `permission`** — a central access-control map,
  documented as the replacement for the deprecated `tools` field
  ([opencode.ai/docs/agents][opencode-agents-docs]). `src/install/vendor_opencode.rs`
  already names this gap explicitly: `OPENCODE_AGENT_FIELDS`'s doc comment
  says "Object-valued fields (`permission`, the deprecated `tools` map) are
  deliberately absent: they cannot be expressed as a single string metadata
  value," and `OpenCodeVendor::agent_index` drops an authored `tools` value
  with a warning pointing at `permission` as the intended replacement. The
  [vendor capability watchlist][watchlist] independently tracks this row:
  "Agent `permission` map | OpenCode | dropped (scalar-only metadata) |
  shipped upstream | gated on `adr_structured_vendor_metadata.md`
  acceptance (FieldType::Json)" — this ADR is that gate.
- **Claude `hooks`** — both `adr_tool_namespaced_metadata_rendering.md`
  ("`hooks` is deliberately absent: it is an object-valued field that
  cannot be expressed as a single string metadata value. A separate hooks
  ADR owns that surface") and `docs/src/vendor-metadata.md` name this
  exclusion explicitly. `adr_hooks_support.md` (status: Proposed) already
  owns hooks as a dedicated artifact kind with its own materialization and
  config-registration machinery — **out of scope here**.
- **Claude agent `mcpServers`** and **Copilot agent `mcp-servers`** — both
  are excluded from their respective agent registries
  (`CLAUDE_AGENT_FIELDS`, `COPILOT_AGENT_FIELDS`) for the same reason, and
  `docs/src/vendor-metadata.md` points both exclusions at the dedicated MCP
  artifact kind (`grim add --kind mcp`, see
  [MCP Server Artifacts](../../docs/src/mcp-servers.md)) — a server
  registration is already representable as its own distributable artifact,
  not a scalar metadata value on an agent. **Out of scope here.**

Of the three, only `opencode.permission` has no existing coverage — it is
the one live capability gap this ADR needs to close.

## Decision Drivers

- Close the one confirmed live gap (`opencode.permission`) without
  reopening the scalar-purity decision the prior ADR made for the common
  case — most vendor fields really are scalars, and `convert`'s six
  existing arms should stay the fast, simple path.
- Preserve the agentskills wire contract: the canonical artifact's
  `metadata` map must stay string-valued on the wire, exactly as the prior
  ADR mandates — this rules out any format change to the artifact itself.
- YAGNI: build the smallest mechanism that covers the one confirmed
  consumer, not a general object-metadata subsystem speculatively sized for
  `hooks` or `mcpServers` (both already have dedicated, better-fitting
  homes).
- Keep publish-time validation as strict as the scalar path: a broken
  literal must fail `grim build`/`grim release` before it reaches a
  registry, exactly like a bad `claude.effort` enum value does today.
- Preserve determinism: `render.rs`'s existing contract — identical input
  yields byte-identical output — must hold for the new variant too, since
  rendered files are integrity-hashed (`generated: true`) and drift
  detection depends on it.

## Considered Options

### Option (a) — `FieldType::Json`: inline-parsed string, canonically re-serialized — CHOSEN

The metadata value stays an ordinary string in the artifact, preserving the
agentskills string→string contract uniformly (no format exception for this
one key). For allowlisted keys, the renderer parses that string as inline
YAML (a superset of JSON, so both YAML and JSON literals are accepted) and
re-serializes it canonically — sorted keys, deterministic emission — into
the native frontmatter at install time. Publish-time validation runs the
same parse and hard-fails exit 65 (DataError) on a parse error, exactly the
existing `RenderError::InvalidValue` path every other `FieldType` variant
already uses.

| Pros | Cons |
|------|------|
| Zero change to the artifact wire format — `metadata` values are strings before and after this ADR | Authors write YAML/JSON-inside-YAML: a string value like `"{write: allow, edit: ask}"` reads less naturally than a native nested block |
| One `FieldType` variant, one `convert` arm — same registry mechanism every other field already uses (`KnownField { field, native, ty: FieldType::Json }`) | The parsed value's shape is not validated against `permission`'s actual schema (nested key/value pairs of `allow`\|`ask`\|`deny`) — only that it parses as YAML, not that its structure is sane for OpenCode |
| Sorted-key canonical re-emit gives the same determinism guarantee as every other `FieldType` — same test shape as `render_is_deterministic_and_identity_detection_works` | A malformed nested value still round-trips through YAML parsing successfully (e.g. a scalar instead of a map) unless the convert arm also asserts `Value::Mapping` |
| Generalizes for free: the next object-valued consumer is a new allowlist entry, not a new mechanism | |

**Chosen.** Closes the one live gap with the smallest addition to an
already-proven mechanism, and keeps the wire format exactly as the prior
ADR left it.

### Option (b) — Reserved nested `vendor:` YAML block in frontmatter — REJECTED

Add a second, parallel authoring surface: a `vendor:` map at the top level
of `SKILL.md`/agent frontmatter, distinct from `metadata`, whose values are
native YAML (not strings) and pass through to the vendor-specific
frontmatter largely as-is.

| Pros | Cons |
|------|------|
| Natural YAML authoring — `permission.edit: ask` as a real nested map, no string-encoding tax | Breaks the agentskills spec-purity principle `adr_tool_namespaced_metadata_rendering.md` established: `metadata` is defined as string-valued by the spec, and a second top-level `vendor:` block is exactly the "spec contamination" Option 1 of that ADR was rejected for |
| No parsing step — the value is already structured YAML at read time | Two parallel mechanisms for the same conceptual thing (vendor-specific field authoring) is a maintenance and cognitive-load regression — every author and every registry consumer now has to know which of two paths a given vendor key takes |
| | Every downstream tool that reads `metadata` as canonical agentskills string map now also has to special-case a second block that isn't part of the spec it's implementing |

**Rejected.** Spec impurity is disqualifying for the same reason it was
disqualifying in the prior ADR, and running two parallel authoring
mechanisms contradicts the "single source of truth" driver both ADRs share.

### Option (c) — Defer entirely — REJECTED

Leave `opencode.permission` dropped, as it is today, and revisit only if
demand grows.

| Pros | Cons |
|------|------|
| Zero implementation cost, zero new surface to maintain | The gap is not speculative — it is `permission`'s documented role as the *replacement* for the already-supported (and now deprecated) `tools` field; every OpenCode agent migrating off `tools` needs it |
| | The vendor capability watchlist already carries this row as "shipped upstream," meaning the decline is actively rotting into a regression per the watchlist's own stated purpose |

**Rejected.** The gap is confirmed, not hypothetical, and the watchlist
process this repo already runs exists specifically to catch this class of
staleness.

## Decision Outcome

**Chosen Option:** (a) — add `FieldType::Json`, allowlisted to exactly one
entry: `opencode.permission`, registered in `OPENCODE_AGENT_FIELDS`
(`src/install/vendor_opencode.rs`).

**Rationale:** this is the minimum mechanism that closes the one confirmed
live gap while reusing every piece of the existing scalar-projection
pipeline (`KnownField`, `partition_metadata`, `convert`, `append_lifted`,
`validate_agent_metadata`) rather than inventing a parallel one. The
allowlist stays a single row deliberately — `hooks` and `mcpServers`/
`mcp-servers` already have better-fitting homes (a dedicated artifact kind
and the MCP artifact kind, respectively), so adding them to this allowlist
would duplicate functionality this repo already ships elsewhere. The
mechanism itself is general: a second consumer is a new allowlist row, not
a new `FieldType` variant or a new code path.

### Consequences

**Positive:**
- Closes the `opencode.permission` gap named in both
  `OPENCODE_AGENT_FIELDS`'s doc comment and the vendor capability
  watchlist, unblocking authors migrating off the deprecated `tools` field.
- The wire format is untouched: `metadata` stays string-valued, so no
  compatibility break, no lock/config schema version bump, no migration.
- The mechanism generalizes: closing a future object-valued gap (should one
  appear outside the two already-covered surfaces) is a registry-row change
  plus an allowlist entry, not new plumbing.

**Negative / Risks:**
- Authoring ergonomics regress slightly for this one key: a nested
  structure written as a quoted YAML/JSON string inside a YAML document is
  harder to read and easier to typo than a native block would be. Mitigated
  by `grim-authoring` skill guidance and a documented authoring example
  once implemented.
- `FieldType::Json`'s `convert` arm validates that the string *parses*, not
  that its parsed shape matches `permission`'s actual schema (map of
  action names to `allow`/`ask`/`deny`). A syntactically valid but
  semantically wrong value (e.g. a YAML scalar or a list where OpenCode
  expects a map) would pass `grim build` and only fail at OpenCode's own
  config load. Accepted as a v1 gap — `Enum`/`Bool`/`Integer` already stop
  at syntactic validation for their own failure classes; a schema-shape
  check for one nested vendor structure is disproportionate scope for the
  gap this ADR closes.
- The allowlist is a hardcoded gate inside the `Json`-arm convert path (or
  the `KnownField` construction site), not a config surface — adding a
  second entry is a code change, deliberately, per YAGNI.

## Implementation Plan

Gated on acceptance of this ADR — implementation happens in a **follow-up
cycle**, not this one.

1. [ ] Add `FieldType::Json` to `src/install/vendor.rs`, documented like the
       other five variants (what a valid literal looks like, what invalid
       looks like).
2. [ ] Add the matching arm to `convert` in `src/install/render.rs`:
       YAML-parse the string value (`serde_yaml::from_str`), reject a
       non-mapping top-level shape as `RenderError::InvalidValue`, then
       re-serialize the parsed `Value` with sorted keys for deterministic
       canonical emission.
3. [ ] Add the `opencode.permission` row to `OPENCODE_AGENT_FIELDS` in
       `src/install/vendor_opencode.rs` (`KnownField { field: "permission",
       native: "permission", ty: FieldType::Json }`), removing it from the
       doc comment's "deliberately absent" list.
4. [ ] Add the matching row to the `opencode.* agent registry` table in
       `docs/src/vendor-metadata.md` in the **same commit**. A doc/registry
       parity test already exists for the Claude skill registry
       (`src/install/vendor_claude.rs`, "Doc/registry parity" — asserts
       every `claude.*` key in the docs page matches `CLAUDE_SKILL_FIELDS`
       exactly); no equivalent test covers `OPENCODE_AGENT_FIELDS` today,
       so add one in the same commit rather than relying on manual sync.
5. [ ] Move the "Agent `permission` map" row in
       `.claude/rules/vendor-capability-watchlist.md` from "gated on ADR
       acceptance" to closed, per the watchlist's own re-verify procedure.
6. [ ] **Mandatory determinism test** (the acceptance gate for this whole
       change, per CLAUDE.md Core Principle 9, "Preserve Compatibility,"
       and `docs/src/stability.md`'s frozen/unstable split): rendering the
       same agent twice with an `opencode.permission` value present is
       byte-identical, mirroring the existing
       `render_is_deterministic_and_identity_detection_works` test shape.
       Additionally, prove the **self-heal** property Principle 9 requires
       for renderer changes: installing an agent carrying
       `opencode.permission`, then re-materializing it, leaves `grim
       status` reporting not-modified (no spurious drift from the new
       code path).
7. [ ] Unit tests mirroring the existing `FieldType` coverage in
       `src/install/render.rs`: valid nested map parses and canonicalizes;
       invalid YAML is a hard `RenderError`; a non-mapping top-level value
       (e.g. a bare string or list) is a hard `RenderError`; publish-time
       validation (`validate_agent_metadata`) surfaces the same error via
       the existing `metadata_invalid` → `SkillErrorKind::MetadataInvalid`
       → exit 65 (DataError) chain already wired at `src/command/build.rs:171`
       and `src/skill/local_pack.rs:76`.
8. [ ] Add an authoring example for `opencode.permission` to
       `docs/src/vendor-metadata.md`, following the existing "Authoring
       example — skill" / "— rule" pattern.

## Validation

Deferred to the follow-up implementation cycle; the acceptance criteria for
that cycle are:

- [ ] `FieldType::Json` convert arm: valid map parses and canonicalizes
      (sorted keys); invalid YAML syntax and non-mapping shapes are hard
      `RenderError::InvalidValue`.
- [ ] `OPENCODE_AGENT_FIELDS` doc/registry parity test covers the new row
      (mirroring the Claude skill-registry parity test in
      `src/install/vendor_claude.rs`).
- [ ] Determinism: re-rendering the same agent document is byte-identical.
- [ ] Self-heal: installing, then re-materializing, an agent carrying
      `opencode.permission` leaves `grim status` not-modified.
- [ ] Publish-time validation: a bad `opencode.permission` literal fails
      `grim build`/`grim release` with exit 65 (DataError), never silently
      installs.
- [ ] `task rust:verify` passes.

## Links

- [`adr_tool_namespaced_metadata_rendering.md`](./adr_tool_namespaced_metadata_rendering.md)
  — the precedent ADR that scoped v1 metadata values to strings and named
  `hooks` as the first known object-valued exclusion.
- [`adr_hooks_support.md`](./adr_hooks_support.md) — the separate,
  still-Proposed ADR that owns Claude `hooks` as its own artifact kind
  (out of scope here).
- [`adr_agent_artifact_kind.md`](./adr_agent_artifact_kind.md) — introduced
  the `agent` artifact kind and named `mcpServers`/`permission`/
  `mcp-servers` as v1 exclusions this ADR partially closes.
- [`.claude/rules/vendor-capability-watchlist.md`](../rules/vendor-capability-watchlist.md)
  — carries the "Agent `permission` map" row gated on this ADR.
- [OpenCode agents documentation][opencode-agents-docs]

[vendor-fieldtype]: ../../src/install/vendor.rs
[render-convert]: ../../src/install/render.rs
[render-partition]: ../../src/install/render.rs
[watchlist]: ../rules/vendor-capability-watchlist.md
[opencode-agents-docs]: https://opencode.ai/docs/agents/

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-17 | Michael Herwig | Initial draft, proposed |
