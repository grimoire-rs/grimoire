# ADR: Type Grimoire artifacts with OCI `artifactType` + the OCI empty config descriptor + a `com.grimoire.kind` annotation fallback

## Metadata

**Status:** Accepted
**Date:** 2026-06-19
**Deciders:** Michael Herwig (maintainer)
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md`
      (OCI is the distribution substrate; this uses standard OCI 1.1 fields)
**Domain Tags:** integration, api
**Supersedes:** [adr_oci_artifact_type.md](./adr_oci_artifact_type.md)

## Context

`adr_oci_artifact_type.md` introduced a per-kind Grimoire config media type
(`application/vnd.grimoire.<kind>.config.v1+json`) as the config descriptor's
`mediaType`. This design works against `registry:2` (Docker Distribution) but
fails against GitLab Container Registry.

GitLab Container Registry validates **every media type referenced in a
manifest** — including the config descriptor `mediaType` — against a
server-managed allowlist. Custom (non-OCI, non-Docker) types are rejected
with:

```
400 MANIFEST_INVALID: unknown media type: application/vnd.grimoire.skill.config.v1+json
```

The `REGISTRY_FF_DYNAMIC_MEDIA_TYPES` server flag that would disable this
check is **not available on GitLab SaaS** (see [GitLab supported media
types][gitlab-media-types]).

The OCI image-spec "Guidance for an Empty Descriptor" (see [OCI manifest
spec][oci-manifest]) blesses `application/vnd.oci.empty.v1+json` for use as
the config descriptor when an artifact has no meaningful config payload and
sets `artifactType` instead. This type is on GitLab's allowlist. ORAS follows
the same convention (see [ORAS manifest-config docs][oras-manifest-config]):
when no config is needed, use the OCI empty config media type.

The **only meaningful type discriminator** for Grimoire artifacts is
`artifactType`. The config descriptor's `mediaType` functioned as a secondary
fallback in `kind_from_manifest`, not as the primary signal. Switching the
config descriptor to the OCI empty type costs nothing on the read path.

## Decision Drivers

- GitLab Container Registry is a target registry ("bring your own registry"
  principle, `product-context.md`). The previous design fails at push time on
  GitLab SaaS.
- `artifactType` is the OCI-sanctioned primary discriminator; config
  `mediaType` as a kind signal was always a secondary fallback.
- The OCI empty config (`application/vnd.oci.empty.v1+json`, blob `{}`) is
  explicitly spec-blessed and on GitLab's allowlist.
- A `com.grimoire.kind` annotation restores a registry-agnostic fallback for
  oldest-version clients that predate `artifactType`.
- No migration: the project is provisional and the existing `adr_oci_artifact_type.md`
  already accepted the consequence of digest changes on re-release.

## Considered Options

### Option 1 — OCI empty config + keep `artifactType` + add `com.grimoire.kind` annotation — CHOSEN

Config descriptor `mediaType` becomes `application/vnd.oci.empty.v1+json`
(blob is `{}`, sha256 digest
`sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a`, size 2).
`artifactType` per kind unchanged. `com.grimoire.kind` annotation re-introduced
as a registry-agnostic fallback.

| Pros | Cons |
|------|------|
| Passes GitLab's media-type allowlist check | Manifest digest changes on re-release (same consequence accepted by the superseded ADR) |
| `artifactType` remains the primary kind signal — no information lost | Tag-tracking consumers see a re-release as an update |
| `com.grimoire.kind` closes the oldest-version read gap | |
| 3-tier read model stays fully backward-compatible | |
| Spec-blessed pattern (OCI image-spec + ORAS) | |

### Option 2 — Keep per-kind config `mediaType`, request GitLab allowlist expansion

Ask GitLab to add `application/vnd.grimoire.*` types to their registry
allowlist.

| Pros | Cons |
|------|------|
| No code change | Not actionable: allowlist is server-managed, not a user-controllable flag on SaaS |
| | Blocks the use case on all GitLab SaaS tenants until/if accepted |
| | Per-OCI-spec guidance, custom configs should only be used when the blob carries meaningful data |

### Option 3 — Replace `artifactType` with the empty config type, drop custom typing entirely

Use `application/vnd.oci.empty.v1+json` everywhere and rely only on the
`com.grimoire.kind` annotation.

| Pros | Cons |
|------|------|
| Simplest wire format | Abandons OCI-native type discrimination (`artifactType`) |
| | Annotation is not indexed/filterable; Referrers API filtering requires `artifactType` |
| | Loses forward path to signatures, SBOMs, attestations (keyed on `artifactType`) |

## Decision Outcome

**Chosen:** Option 1. The config descriptor `mediaType` changes to
`application/vnd.oci.empty.v1+json`; the blob is the byte-identical `{}`
(only the type string changes). `artifactType` per kind is **unchanged** — it
remains the primary discriminator. `com.grimoire.kind` is **re-introduced** as
a registry-agnostic fallback annotation.

### Wire contract (per kind)

| Kind | `artifactType` | config `mediaType` | `com.grimoire.kind` annotation |
|------|----------------|--------------------|-------------------------------|
| skill | `application/vnd.grimoire.skill.v1` | `application/vnd.oci.empty.v1+json` | `skill` |
| rule | `application/vnd.grimoire.rule.v1` | `application/vnd.oci.empty.v1+json` | `rule` |
| agent | `application/vnd.grimoire.agent.v1` | `application/vnd.oci.empty.v1+json` | `agent` |
| bundle | `application/vnd.grimoire.bundle.v1` | `application/vnd.oci.empty.v1+json` | `bundle` |

Layer media types unchanged: `application/vnd.grimoire.artifact.layer.v1.tar`
(skill/rule/agent payload), `application/vnd.grimoire.bundle.v1+json` (bundle
members layer). Config blob: deterministic `{}` (type lives in the descriptor's
`mediaType`, not the blob bytes — same as before).

### Read/write model

- **Write:** kind is known at the release site → stamp `artifactType` +
  `config.mediaType = application/vnd.oci.empty.v1+json` + `com.grimoire.kind`
  annotation. The custom config blob is replaced with the standard `{}` empty
  blob. Metadata annotations (`org.opencontainers.image.*`, `com.grimoire.keywords`,
  `com.grimoire.summary`, `org.opencontainers.image.source`) are unchanged.
- **Read:** the single seam `kind_from_manifest` is a 3-tier resolver:
  1. `artifactType` — primary; present on all new and existing Grimoire
     artifacts.
  2. Legacy `config.mediaType` — retained for artifacts published under the
     previous ADR (custom per-kind type strings). No strict-equality check
     anywhere forces a specific config type, so this tier never blocks.
  3. `com.grimoire.kind` annotation — fallback for pre-`artifactType` artifacts
     and for registries or tools that surface annotations but not `artifactType`.
  A manifest that matches none of the three tiers → `None` → `grim add` errors
  `KindInferenceFailed` asking for `--kind` (unchanged UX).

### Backward and forward compatibility

| Scenario | Outcome |
|----------|---------|
| New grim reads a **legacy artifact** (custom config `mediaType`) | `artifactType` resolves at tier 1; tier 2 legacy fallback also resolves. No breakage. |
| Old grim 0.4.x reads a **new artifact** (empty config + annotation) | `artifactType` resolves at tier 1; the changed config fallback is never reached because there is no strict-equality check on config type anywhere. No breakage. |
| Pre-`artifactType` grim reads a **new artifact** | `com.grimoire.kind` annotation resolves at tier 3. Closes the oldest-version gap. |
| Digest-pinned ref or existing lockfile | Resolves to the old immutable manifest (content-addressed). No breakage. |
| Tag-tracking consumer after a re-release | Sees an update (new manifest digest). Same consequence already accepted by the superseded ADR. |

### Consequences

**Positive:**
- `grim publish` succeeds against GitLab Container Registry SaaS without any
  server-side configuration.
- Wire format is spec-blessed: OCI image-spec "Guidance for an Empty
  Descriptor" + ORAS convention.
- No information lost: `artifactType` carries the kind; the `com.grimoire.kind`
  annotation provides a human-readable and annotation-queryable redundant copy.
- Full backward compatibility on both read directions (see table above).

**Negative / Risks:**
- Manifest shape change → existing published artifacts get new digests on
  re-release. Acceptable: provisional project, no install base to migrate, same
  consequence accepted by the superseded ADR.
- **Residual unknown:** whether GitLab also validates and rejects the custom
  `artifactType` (`application/vnd.grimoire.<kind>.v1`). The acceptance suite
  runs against `registry:2` (Docker Distribution), which accepts all
  `artifactType` values. Only the issue reporter can confirm against real GitLab
  SaaS. The `com.grimoire.kind` annotation is positioned so that dropping or
  generic-izing `artifactType` is a safe, non-breaking follow-up if needed.

## Validation

- Rust unit tests: manifest round-trip confirms `config.mediaType` is
  `application/vnd.oci.empty.v1+json`; `kind_from_manifest` resolves from
  `artifactType` (tier 1), from legacy config type (tier 2), and from annotation
  (tier 3); annotation builders emit `com.grimoire.kind`.
- Acceptance: release → `add`/install → catalog against live `registry:2`
  (proves the registry accepts `artifactType` + OCI empty config, kind inference
  works end-to-end, and idempotent re-releases produce stable digests).
- GitLab compatibility: requires reporter confirmation that a push with OCI
  empty config + custom `artifactType` succeeds against GitLab SaaS.

## Links

- Supersedes: [`adr_oci_artifact_type.md`](./adr_oci_artifact_type.md)
- [OCI image-spec — "Guidance for an Empty Descriptor"][oci-manifest]
- [ORAS manifest-config concepts][oras-manifest-config]
- [GitLab Container Registry supported media types][gitlab-media-types]

[oci-manifest]: https://github.com/opencontainers/image-spec/blob/main/manifest.md
[oras-manifest-config]: https://oras.land/docs/concepts/manifest/
[gitlab-media-types]: https://docs.gitlab.com/ee/user/packages/container_registry/

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-06-19 | Michael Herwig | Initial draft, accepted; supersedes adr_oci_artifact_type.md |
