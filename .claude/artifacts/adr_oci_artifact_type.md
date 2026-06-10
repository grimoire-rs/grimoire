# ADR: Type Grimoire artifacts with OCI `artifactType` + a Grimoire config media type

## Metadata

**Status:** Accepted
**Date:** 2026-06-10
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI is the distribution substrate; this uses standard OCI 1.1 fields)
**Domain Tags:** integration, api
**Supersedes:** N/A

## Context

`grim` publishes a plain OCI **image** manifest: a generic config blob
(`{"architecture":"","os":""}`, media type
`application/vnd.oci.image.config.v1+json`), `artifactType` left unset, and the
artifact kind (skill/rule/bundle) carried **only** in the `com.grimoire.kind`
manifest annotation (push path `src/oci/access/registry_client.rs:413`,
`config_blob()` at `:233`; annotation writers in `src/oci/annotations.rs`).

Two problems:

1. **Wrong type signal.** To any registry, `oras`, scanner, or UI a Grimoire
   artifact is indistinguishable from a runnable container image — the generic
   image config is a small lie, and nothing at the OCI type level says "this is
   a Grimoire skill."
2. **Annotation misused as a discriminator.** Annotations are arbitrary
   metadata: not indexed, not filterable, and not what the Referrers API or
   registry tooling inspect. OCI provides a dedicated mechanism for "what is
   this": `artifactType` (OCI image-spec 1.1, 2024) with `config.mediaType` as
   the pre-1.1 fallback.

## Decision Drivers

- Make a Grimoire artifact self-describing at the OCI type level.
- Use the standards-blessed mechanism, not a bespoke annotation convention.
- Preserve the idempotent-re-release contract (deterministic manifest digest).
- Maximize registry compatibility — the project targets "bring your own
  registry," public or private (`product-context.md`).
- KISS / single source of truth — the project is provisional, so no migration
  shims are warranted (`design_registry_tags_clients.md`).

## Considered Options

### Option 1 — `artifactType` + `config.mediaType` (both set) — CHOSEN

Set the manifest-level `artifactType` **and** the config descriptor's
`mediaType` to a Grimoire per-kind media type. Tiny deterministic `{}` config
blob. Drop `com.grimoire.kind`.

| Pros | Cons |
|------|------|
| Modern registries index/filter on `artifactType`; pre-1.1 tooling still types via `config.mediaType` | Two fields to keep in sync (mechanised via `ArtifactKind` methods) |
| Removes the fake image-config | |
| Read works on any registry that round-trips manifest bytes | |

### Option 2 — `artifactType` only (OCI empty config)

`artifactType` set; config is the OCI empty config
(`application/vnd.oci.empty.v1+json`).

| Pros | Cons |
|------|------|
| Cleanest, most modern | No type signal at all for tooling that only reads `config.mediaType` |
| One discriminator | Relies entirely on `artifactType` being surfaced |

### Option 3 — `config.mediaType` only (pre-1.1 "Helm style")

No `artifactType`; kind lives in `config.mediaType`.

| Pros | Cons |
|------|------|
| Widest legacy support | Not discoverable via the Referrers API |
| | Tooling that filters on `artifactType` sees nothing |

### Option 4 — keep `com.grimoire.kind` annotation

Status quo, or annotation alongside a type.

| Pros | Cons |
|------|------|
| Zero change / human-readable | Annotation is not a type; not indexed/filterable; duplicates a field OCI already provides |

## Decision Outcome

**Chosen:** Option 1. `com.grimoire.kind` is **dropped** (not kept as a
fallback) — `artifactType` is the single source of truth, `config.mediaType`
the secondary read fallback. Provisional project ⇒ no migration of existing
artifacts.

### Wire contract (per kind)

| Kind | `artifactType` | config `mediaType` |
|---|---|---|
| skill | `application/vnd.grimoire.skill.v1` | `application/vnd.grimoire.skill.config.v1+json` |
| rule | `application/vnd.grimoire.rule.v1` | `application/vnd.grimoire.rule.config.v1+json` |
| bundle | `application/vnd.grimoire.bundle.v1` | `application/vnd.grimoire.bundle.config.v1+json` |

Layer media types unchanged: `application/vnd.grimoire.artifact.layer.v1.tar`
(skill/rule payload), `application/vnd.grimoire.bundle.v1+json` (bundle members
layer). Config blob: deterministic `{}` (the type lives in the descriptor's
`mediaType`, not the blob bytes).

### Read/write model

- **Write:** kind is known at the release site → stamp `artifactType` + config
  `mediaType`; stop writing `com.grimoire.kind`. Metadata annotations
  (`org.opencontainers.image.*`, `com.grimoire.keywords`) are unchanged — that
  is the correct use of annotations.
- **Read:** the single seam `kind_from_manifest` resolves `artifactType` first,
  then `config.mediaType`. A foreign image (generic config, no `artifactType`) →
  `None` → `grim add` still errors `KindInferenceFailed` asking for `--kind`
  (unchanged UX).

### Consequences

**Positive:**
- `grim` can verify "is this a Grimoire artifact, and which kind?" before pull.
- Registry UIs / `oras` / scanners show a typed artifact, not a mystery image.
- Enables the Referrers API later (signatures, SBOMs, attestations key off
  `artifactType`).

**Negative / Risks:**
- Manifest shape change → existing published artifacts get new digests.
  Acceptable: provisional, no install base to migrate.
- `artifactType` Referrers-API *filtering* needs OCI 1.1 registry support; the
  `config.mediaType` mirror covers pre-1.1 tooling, and plain manifest pull
  returns the bytes (incl. `artifactType`) on any conformant registry.
  Validated against the acceptance suite's `registry:2`.

## Validation

- Rust unit tests: manifest round-trip of `artifactType`/`config_media_type`;
  `kind_from_manifest` resolves from type; annotation builders no longer emit
  kind; in-memory `ManifestKey` distinguishes manifests by type.
- Acceptance: release → `add`/install → catalog against live `registry:2`
  (proves the registry accepts the custom `artifactType` + config media type and
  that kind inference works with no annotation). Idempotency tests prove
  deterministic digests.

## Links

- Plan: `.claude/plans/plan-the-adoption-as-shiny-dove.md`
- Related design note: [`design_registry_tags_clients.md`](./design_registry_tags_clients.md)
- [OCI image-spec — `artifactType` & artifact guidance](https://github.com/opencontainers/image-spec/blob/main/manifest.md)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-06-10 | Michael Herwig | Initial draft, accepted |
