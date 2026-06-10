# ADR: Rules may carry an optional sibling support directory

## Metadata

**Status:** Accepted
**Date:** 2026-06-10
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI distribution substrate unchanged; reuses the existing single-layer
      tar transport)
**Domain Tags:** integration, api
**Supersedes:** N/A

## Context

A Grimoire **rule** has been a single `.md` file (`ArtifactKind::Rule`,
`is_dir_artifact() == false`): the packer emits exactly one `<name>.md`
entry and the installer materializes exactly that file.

The common AI-config pattern, however, is an *index* rule
(`rules/my-rule.md`) that references extra context in a sibling directory
(`rules/my-rule/`: examples, schemas, scripts). Grimoire could not package
or install that — the index's relative links (`./my-rule/…`) had no target
on the consumer side.

The transport is **not** the gap. A skill (`ArtifactKind::Skill`) already
proves the whole pipeline: its directory tree packs into one
uncompressed-tar layer (`pack_skill_dir`) and the materializer safely
extracts arbitrary multi-entry tars. The gap is only that rule packing
emitted a single entry and the install seams assumed a rule is one file.

## Decision Drivers

- Support the index-plus-support-dir convention without changing the OCI
  wire contract (`artifactType`, layer media type, manifest shape).
- Keep the single-file rule the degenerate case — zero behavior change,
  byte-identical packing for existing rules.
- Smallest change that threads the "a rule is one file" assumption out of
  the install seams (pack, materialize, integrity hash, record, uninstall).
- KISS / YAGNI — no new artifact kind, no generic multi-path release.

## Considered Options

### Option 1 — Extend `Rule` with an optional sibling support dir — CHOSEN

The authoring input stays the index `.md` file (so `detect_kind` is
unambiguous). The packer additionally walks a sibling `<name>/` directory
when present and emits its files into the same single tar layer. The
installer materializes the index file plus the support dir beside it.

| Pros | Cons |
|------|------|
| OCI wire contract unchanged (same `artifactType`, same layer media type) | Two on-disk roots (index file + sibling dir) must be threaded through every install seam |
| Single-file rules pack byte-identically — no migration | Integrity hash must fold a two-root footprint |
| Reuses the skill transport (`collect_files`, multi-entry materializer) | |
| Clean name derivation + defined install destinations | |

### Option 2 — A generic "list of arbitrary paths per release"

A release carries N arbitrary paths with per-path destinations.

| Pros | Cons |
|------|------|
| Maximally flexible | Loses the index/support structure: no clean name derivation, no defined install destination per path |
| | Bloats the materializer with destination-mapping policy |

### Option 3 — A new artifact kind for multi-file rules

`ArtifactKind::RuleBundle` (or similar) distinct from `Rule`.

| Pros | Cons |
|------|------|
| Explicit at the type level | Fragments the wire contract + UX for what is still a rule |
| | Forces `--kind` ambiguity and a second discriminator path |

## Decision Outcome

**Chosen:** Option 1. A rule may carry an optional sibling support
directory; it is packed into the same single tar layer and installed beside
the index file. Single-file rules are unaffected.

### On-disk / wire model

Authoring layout (input to `grim build` / `grim release` is the index file):

```
rules/
  my-rule.md        # index, paths:-scoped — unchanged
  my-rule/          # optional support dir (same stem as the index)
    examples.md
    schema.json
```

Packed into one TAR layer (`artifactType = application/vnd.grimoire.rule.v1`,
unchanged): `my-rule.md`, `my-rule/examples.md`, `my-rule/schema.json`.
The sibling dir is detected from the index path as `file.with_extension("")`
and included only when it is a real directory. Entries are emitted in sorted
order, so the layer digest stays deterministic and a re-release of identical
content is idempotent.

Installed beside each other so the index's relative links resolve:

```
.claude/rules/my-rule.md
.claude/rules/my-rule/examples.md       # Claude / OpenCode: verbatim
.github/instructions/my-rule.instructions.md
.github/instructions/my-rule/…          # Copilot: index transformed, support verbatim
```

For every client the support dir is copied **verbatim** — only the index is
ever transformed (Copilot frontmatter strip + provenance header). Per-client
relative-link rewriting is out of scope (verbatim copy is the MVP, matching
skills).

### Integrity, record, uninstall

- **Integrity hash:** a rule-footprint hash folds the index file (keyed
  `<name>.md`) and the support-dir files (keyed `<name>/<rel>`) into one
  SHA-256 using the existing rel-keyed, walk-order-independent scheme. With
  no support dir the result is byte-identical to the previous single-file
  hash, so a drifted support file is detected like any other edit.
- **Install record:** `ClientRecord` gains
  `support_dir: Option<PathBuf>` (`#[serde(default, skip_serializing_if =
  "Option::is_none")]`). `InstallStateVersion` stays `V1` — the field is
  additive and optional, so old records load unchanged.
- **Uninstall:** removal iterates the recorded outputs and now reaps the
  support directory alongside the index file. Idempotent / absent-tolerant
  behavior is preserved.

### Consequences

**Positive:**
- The index-plus-support-dir convention round-trips through publish and
  install.
- No wire-contract change; existing artifacts and registries are unaffected.
- Reuses the proven skill transport — minimal new surface.

**Negative / Risks:**
- The two-root footprint (index file + sibling dir) is threaded through ~7
  modules; each seam must handle "rule = file + optional dir". Mitigated by
  centralizing the footprint hash and exposing
  `ClientRecord::current_hash()` so every reader computes it one way.
- Verbatim copy means a relative link in the index that points outside the
  support dir still will not resolve — acceptable for the MVP.

## Validation

- Rust unit tests: packer emits index + support files (and packs identically
  without one, deterministically); materializer extracts a multi-entry rule
  tar; footprint hash is stable and detects support-file drift; installer
  fresh→no-op→support-drift-refused→forced; uninstall removes both roots
  idempotently; per-client materialize copies the support dir verbatim
  (Copilot transforms only the index).
- Acceptance (live `registry:2`): `grim release` a rule with a support dir →
  `install` lands both `rules/<name>.md` and `rules/<name>/…` → editing a
  support file shows `modified` → `uninstall` removes both → idempotent
  re-release yields the same digest.

## Links

- Plan: [`plan_multifile_rules.md`](./plan_multifile_rules.md)
- Related ADR: [`adr_oci_artifact_type.md`](./adr_oci_artifact_type.md)
  (kind discrimination via OCI `artifactType` — untouched here)
- [OCI image-spec — `artifactType` & artifact guidance](https://github.com/opencontainers/image-spec/blob/main/manifest.md)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-06-10 | Michael Herwig | Initial draft, accepted |
