# Research: Artifact dependencies — architecture map and claim diff

## Metadata

**Date:** 2026-11-01
**Domain:** packaging
**Triggered by:** `.agents/adr/adr_artifact_dependencies.md` (in design), hex-architect Phase 1
**Expires:** 2027-05-01

## Claim diff (anchor 2026-11-01, HEAD cdeedcaa v0.15.1)

Claims about these paths were confirmed, some with line shifts:

- `src/resolve/resolver.rs`
  - flat `expand_bundles`: 447-550
  - conflict policy: 298-302, implemented in `merge_bundle_members` at 372-438
  - nested reject: 500-505
- `src/oci/bundle.rs`
  - "no recursion in v1": 47
  - caps (512 KiB, 512 members): 28-37
- `src/lock/locked_artifact.rs`: `BundleProvenance` at 15-21, `deny_unknown_fields` at 70
- `src/lock/locked_bundle.rs`: 79 and 143
- `src/lock/effective_set.rs`: 52, one level only. It returns `None` for path-sourced or unsnapshotted bundles (92-99).
- `src/oci/annotations.rs`: 231-290
- `.agents/adr/adr_artifact_trust_model.md`: 46-48
- `.agents/adr/adr_hook_workspace_consent.md`: 262-266, written via `src/command/add.rs:517`
- `docs/src/content/docs/stability.md`
  - "Forward compatibility": 413-462
  - at 64-66, an absent annotation must never mean "this grim is too old"
- exit codes: `BundleConflict` → 78 (`src/error.rs:473`), `BundleInvalid` → 65 (468)

Three claims were refuted:

- **Metadata namespacing.** Namespaces are client names only (`KNOWN_NAMESPACES`, `src/install/render.rs:67`). No `grim.*` precedent exists.
- **"Degrades silently".** Old grim **fails loudly** (see below).
- **The layer.rs fallback.** The raw-tar fallback for unknown media types is never reached, because the single-layer check comes first.

## Old-grim behavior (grim ≤ 0.15.1)

- `add`, `lock` and `resolve` succeed. `resolve_one` pins the digest only (`src/resolve/resolver.rs:869-895`).
- **Install fails.** `fetch_verified_layer` calls `manifest.single_layer()` (`src/oci/manifest.rs:79-85`). A two-layer manifest gives `MaterializeFailed("expected a single-layer artifact, manifest has N layers")` (`src/install/installer.rs:2367-2375`). That is exit 65, with no reason slug and no "update grim" text.
- `grim fetch` fails the same way (`src/fetch.rs:349-354`).
- `src/install/expected_outputs.rs:108` silently returns `None` for the mcp descriptor.
- Unknown annotations are carried in a map and ignored, so old grim cannot honor a `requires-features` annotation.
- A lock with new provenance fields makes old grim exit 78 (`deny_unknown_fields`).
- A new field in `McpDescriptor` (`src/oci/mcp.rs:171`) or `hook.toml` (`src/oci/hook.rs:396`) makes old grim exit 65.

## Reusable components

- `BundleManifest` / `BundleMember` (`src/oci/bundle.rs`). Members are sorted, which gives a byte-stable digest.
- `fetch_bundle_members` (`src/resolve/resolver.rs:711`, pub): timeout and caps. Its `single_layer()` call at 780 must change for the two-layer shape.
- `merge_bundle_members` (`src/resolve/resolver.rs:372`): pure and unit-tested.
- Member validation (`src/resolve/resolver.rs:506-529`): `SkillName::parse` + `MemberRef::parse`.
- `pin_members` (`src/command/release.rs:563`): private, but not bundle-specific.
- Lock dual-shape provenance (`2673bb2d`).
- `src/error.rs:457-475` mapping. Refusal precedents:
  - `HookError::UnsupportedSchema` (`src/oci/hook.rs:1008`)
  - unsupported `declaration_hash_version` → 78 (`src/error.rs:402`)
  - slug `unsupported-layer-media-type`
  - The `ErrorReason` slug set is append-only and pinned at `src/error.rs:1585-1610`.

## Source-side surfaces

- **skill / agent / rule:** frontmatter `metadata: BTreeMap<String,String>` (string-only; `keywords` is comma-joined).
- **mcp:** `McpDescriptor` has `deny_unknown_fields`. A `requires` field must be stripped before the layer is packed.
- **hook:** `hook.toml` is packed verbatim and versioned by `schema`.
- **bundle:** `BundleSource` per-kind tables (`src/config/project_config.rs:779`).

## Publish

- The members layer is built at `src/command/release.rs:418-445`, and `--pin` → `pin_members` runs at 414-416.
- `--pin` is documented "ignored for skills/rules" (`src/command/release.rs:69-74`). `publish.toml` rejects `pin=true` for non-bundle entries (`src/command/publish.rs:1884`).
- `same_content` compares layer lists pairwise (`src/oci/layer.rs:229-246`).

## Risks

1. The ADR premise is wrong: old grim refuses with exit 65, with an unhelpful message.
2. Resolve cost. A manifest fetch, plus a layer fetch, is needed per artifact. `resolve_work` fans out unbounded tasks (`src/resolve/resolver.rs:836-840`), and recursion makes that fan-out wire-controlled (ASYNC-05).
3. The member cap applies per bundle only. A whole-graph cap is needed.
4. Path-sourced artifacts need their own `requires` rule.
5. `effective_set` and `[[bundle]]` are flat. Transitive removal needs a closure-aware cache, added additively.
6. `stability.md:64-66` conflicts with an annotation that makes grim refuse, so the contract text needs updating.
7. Schema-bound source formats (mcp, hook) make old grim fail to parse.
8. The "update grim" refusal could reuse 78 or 65 plus a new append-only slug.

## Consumers of provenance

- `src/command/remove.rs:156`, `src/command/uninstall.rs:97`
- `src/command/status.rs:409-421`
- TUI: `src/tui/app.rs:535,3387`, `src/tui/bundle_members.rs`
- hook consent: `src/command/add.rs:517`
- grimoire-vscode parses the prose `StatusItem.source` (see `research_artifact_dependencies_ocx_why.md`).
