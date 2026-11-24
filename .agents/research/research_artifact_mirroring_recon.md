# Research: Artifact mirroring recon (grimoire and ocx-mirror)

## Metadata

**Date:** 2026-11-23
**Domain:** packaging
**Triggered by:** pre-plan discussion `.agents/discussions/artifact-mirroring.md` ("Mirroring of artifacts into grimoire")
**Expires:** 2027-05-23

Read-only recon. No recommendation. Marks: [src] = read from source with cite, [inf] = inferred.
Meanings: (A) ingest foreign content and republish as grim artifacts; (B) copy grim artifacts registry to registry. A third usage of the word exists in grimoire records (C): the harness marketplace "projection" (grim to harness catalog), see 1.4.

## Direct Answer

1. Grimoire has no mirroring feature of either kind. `grep -i mirror` over `src/` finds only doc-comments that use "mirrors" as a verb; no `[mirrors]`-style config, no import or sync command [src: `src/config/*.rs`, `src/oci/*.rs`].
2. For (B), grimoire has two partial hooks: publisher-side `--push-registry` (push to one host, bake another name) and relative bundle/`requires` refs that survive a mirror. It has no consumer-side registry rewrite, and its lock embeds the registry host.
3. For (A), the only way content becomes a grim artifact is `grim build|release|publish` over a local directory or TOML source. Nothing reads git repos, harness marketplaces or other registries. The export direction exists (`grim export plugin|marketplace`), the import direction does not.
4. ocx-mirror (sibling repo) implements both (A) (`mirror.yml`: GitHub Releases, URL index, PyPI) and (B) (`registry sync`, `dist sync`), including signature carriage and signing. Its records are mostly marked "Proposed" while the code has shipped (see 2.5).

## 1. Grimoire inventory

### 1.1 Paths by which content becomes a grim artifact or moves between registries

| Path | What it does | Cite |
|---|---|---|
| `grim build <path>` | Validate and pack a local dir, no push | `src/command/build.rs:715`; `.claude/rules/subsystem-cli-commands.md:38` |
| `grim release <path> <ref>` | Pack and push one artifact, cascade tags (never moves a float off a newer release), `--push-registry` | `.claude/rules/subsystem-cli-commands.md:40` |
| `grim publish` | Batch release from `publish.toml`; `push_registry`/`--push-registry`; `--announce` opens an index PR | `src/command/publish.rs:1058`, `:135`, `:1119`, `:1667`; `:113` (announce) |
| `grim fetch <ref> [--out]` | Print payload or companion tree without installing; `--digest-only` | `src/command/fetch.rs:76`; `subsystem-cli-commands.md:30` |
| `grim export plugin / marketplace` | Render locked artifacts as harness plugin dirs or a marketplace repo (OCI to harness, one way) | `src/command/export.rs:177,202,244` |
| Local-path sources | Config value starting `./`, `../` or absolute is a path source, packed and hash-pinned, no registry identity | `src/config/path_source.rs:6-24`; `docs/src/content/docs/concepts.md:105` |
| Index | Pointers only; `grim publish --announce` writes `metadata.json` and opens a PR | `docs/src/content/docs/package-index.md:24-27,225-262,520-560` |

[inf] No code path reads an artifact from one registry and writes it to another. Registry-to-registry copy is documented only as "use skopeo or oras" (`docs/src/content/docs/publishing.md:317-325`, `docs/src/content/docs/artifacts.md:606`, `.agents/adr/adr_artifact_dependencies.md:198`).

### 1.2 Reference shape, registry resolution, host embedding (matters for B)

- Reference: `registry/repository:tag` or `@sha256:` (`docs/src/content/docs/concepts.md:97-98`).
- Lock entry embeds host and repo: `pinned = "ghcr.io/michael-herwig/arcana/hex-architect@sha256:..."` plus `bundle = "<abs ref>"`, `bundle_tag` [src: `grimoire.lock:10-14`; type `LockedSource::Registry(PinnedIdentifier)` at `src/lock/locked_source.rs:43`, `src/oci/pinned_identifier.rs:20`]. Content equality compares registry + repository + digest (`locked_source.rs:57-63`).
- `grimoire.toml` declarations are full references (`docs/src/content/docs/configuration.md:32-34`). Short ids resolve through `--registry` > `GRIM_DEFAULT_REGISTRY` > project/global `default_registry` > built-in `ghcr.io/grimoire-rs` (AGENTS.md env table). `[[registries]]` is `oci` or `index` entries with `alias` (`configuration.md:269-320`).
- Relative member refs (`./x:1`, `../skills/x:0`) resolve against the parent's current pinned registry/repo, so "a mirror switch needs no lock rewrite" for those edges; absolute ids do not follow a mirror [src: `.agents/adr/adr_artifact_dependencies.md:412-421,664-671`; `docs/src/content/docs/artifacts.md:600-625`].
- `grim lock` keeps a held pin "after only the registry moved" for tagless pins (`docs/src/content/docs/concepts.md:313-315`) [inf: this is a pin-retention rule, not a host-rewrite feature].
- Push/pull split (issue #39): manifest `registry` is the canonical PULL name baked into every ref, annotation and report; `push_registry` names only the network push endpoint. Pins for same-registry bundle members are resolved via the push endpoint, sound "for a true mirror (identical bytes, identical digests)"; the description companion (`__grimoire` tag) lands only on the push registry [src: `.agents/adr/adr_push_pull_registry_split.md:15-35,60-70,85-96`; `docs/src/content/docs/publishing.md:1417-1428`].
- Description companion is a separate manifest under the `__grimoire` tag; a single-tag copy drops it, a full-repo sync keeps it [src: `docs/src/content/docs/publishing.md:305-332`; `.agents/adr/adr_description_companion.md:221`].
- Client-side registry rewrite (ocx `[mirrors]` equivalent): none found [src: grep of `src/config`, `src/oci`; `docs/src/content/docs/configuration.md` has no rewrite section].

### 1.3 Publisher identity, provenance, trust

- Trust model: integrity (digest pin), not authenticity. Decision 3: "Until signatures exist, an artifact is treated as if signed by a trusted authority." Out of scope: whether the author is who they claim; compromise of a registry account or publisher toolchain [src: `.agents/adr/adr_artifact_trust_model.md:119-126,155-176`].
- "When signatures arrive": verification becomes a fetch/resolve-time gate for every kind, with a strictness policy, not a per-kind prompt; "Revisit this ADR then" [src: `adr_artifact_trust_model.md:155-160`]. The absence of a signature story is "known, accepted, tracked as its own work" (`:173-175`).
- No signing, cosign, referrer or attestation ADR exists in `.agents/adr/` (names checked). `grep -il "cosign|referrer"` hits only research and the dependency ADR, which rejects referrers as a carrier for dependency data because "support for the Referrers API and for mirroring referrers is uneven" [src: `.agents/adr/adr_artifact_dependencies.md:222-232`; research list: `.agents/research/research_artifact_dependencies_*.md`, `research_catalog_scale_security.md`, `research_hooks_autoexec_supply_chain.md`].
- Threat model: T1 is a malicious or compromised publisher (core exposure), T2 mutable-identity swap; N1-N5 non-goals, "insiders with commit access" among them [src: `.claude/rules/arch-threat-model.md:44-48,57-61`].
- Provenance annotations: `org.opencontainers.image.revision|created|source` from git; opt-in `--git` first (`.agents/adr/adr_git_provenance_annotations.md:1-60`), later made default and `created` always emitted (`.agents/adr/adr_default_provenance_and_support_channels.md`; `docs/src/content/docs/stability.md` additive-annotations paragraph) [src for ADR text; the "now default" reading is inferred from the stability.md excerpt]. Code: `src/oci/git_provenance.rs`.
- Index namespace ownership: namespace = forge identity; auto-merge requires PR author login or public org membership, `owner.id` matches the live account ID, `ref` anonymously reachable. `ref` carries no tag or digest. Index row mirrors annotations `deprecated`, `replaced_by`, `license`, `created` [src: `docs/src/content/docs/package-index.md:259-312,520-560`].

### 1.4 Existing mentions of "mirror"

- `.agents/discussions/artifact-dependencies.md:81` records only a test case: "relative dependency refs surviving a mirror". It anchors to the ADR decision that relative ids re-resolve against the parent's current pinned repository (`adr_artifact_dependencies.md:412-421`). No mirroring feature decision.
- `.agents/discussions/marketplace-phase-2.md:77`: "Scope: the marketplace stays a curated view (hand-picked seed, grown by hand), not a mirror of every index bundle - reaffirmed by the owner 2026-09-28." Meaning (C) in a negative sense: do not project the whole index.
- `.agents/discussions/harness-native-marketplaces.md:50-58`: "Purpose: on-ramp ... The projection is lossy by design (no digest pinning, lockfile, drift, ratings) ... No parity contract is frozen under Principle 9 for the mirror." and (owner 2026-09-27) "a marketplace is a curated view, not a mirror - the index is already criticized as too full". Index complaint is recorded here only; no size figure or source found.
- `.agents/discussions/harness-native-marketplaces.md:424-428` (research summary): "projections survive only when bot-generated off the canonical source and explicit about lost guarantees; polyfill.io as the unpinned-mirror failure; Open VSX shows hand dual-publish -> staleness by default." Source: `.agents/research/research_harness_marketplace_adjacent.md:19,36-41,57-60` ("Mirrors silently drop cryptographic guarantees unless a second, explicit mechanism restores them", Terraform network mirror drops signatures).
- `.agents/discussions/harness-native-marketplaces.md:484-490`: annotation mirrors on the index row (`deprecated`, `replaced_by`, `license`) are copies of OCI annotations because "the pointer is the only source a browse reads" and an additive optional row field is mirrored by the indexer.
- `.agents/discussions/harness-native-marketplaces.md:410-411`: "inverse question (grim-owned plugin containers on the user's machine) stays deferred per `research_plugin_schemes.md`".
- `.agents/research/research_plugin_format_compat_a.md:92`: harness plugin sources use `github`, `url`, `git-subdir` with `ref` and `sha` (40-char) pinning. This is the shape (A) would ingest from [src].

### 1.5 Index growth, indexer, annotations

- Index = phone book of pointers, never versions (`package-index.md:24-27`). Locators: `<base>/c/root.json` and gzip blobs; both permanent once emitted (`stability.md` package-index row).
- Scale records: `.agents/discussions/index-structure.md:49,131,135` (search size trigger, 1 GB Pages limit open), `.agents/discussions/index-scale-rollout.md` (lead goal "robust at scale", `:83`), `.agents/adr/adr_catalog_at_scale.md`, `.agents/adr/adr_index_site_at_scale.md`. [inf] No record of a numeric "too full" threshold; the phrase appears only at `harness-native-marketplaces.md:56`.
- Indexer copies `["title","summary","version","license","created"]` from `grim describe --format json` into an enrich sidecar (`.agents/adr/adr_default_provenance_and_support_channels.md` context) [src].

### 1.6 Stability contracts any new source or field is bound by

Frozen at 1.0 (`docs/src/content/docs/stability.md:25-55`): CLI surface and exit codes; `--format json` reports; `grimoire.toml`/`grimoire.lock` schema and "grim lock keeps existing pins"; `publish.toml` incl. every spelling a key has accepted; the OCI wire format (artifact kinds, release/push mechanics, `com.grimoire.*` annotations, layer media types, the `__grimoire` rules); package-index transport; MCP tool surface; env vars; published schema URLs. Evolution is additive-only: new optional fields, annotations that do not apply are absent, an old reader must ignore unknown output fields; manifest inputs are `deny_unknown_fields` (`stability.md` additive-fields section; AGENTS.md Principle 9). New `ArtifactKind` CLI values are "appended, never inserted" (`subsystem-cli-commands.md:38`).

## 2. ocx-mirror inventory (read-only; repo `ocx-sh/ocx-mirror`, records in `ocx-mirror/.claude/artifacts/` and `ocx/.claude/artifacts/`)

### 2.1 What it does end to end

- Separate binary on `ocx_lib`. Input: one `mirror.yml` per tool (source, per-platform asset regexes, target, version bounds, metadata, cascade, announce). Sources: `github_release`, `url_index` (remote JSON, inline versions or generator command), `pypi`/`pylock` [src: `ocx-mirror/docs/index.md`; `ocx-mirror/docs/reference/mirror-yml.md:66-130,191-272`; `ocx-mirror/.claude/artifacts/adr_ocx_mirror.md:Decision`].
- Canonical internal form: a version string plus a map of asset name to URL; every download optionally checked against a per-asset `sha256` (`adr_ocx_mirror.md` Canonical Internal Representation; `docs/index.md` "What it does").
- Two phases: prepare (download, verify, bundle, concurrent) then push (sequential by version so cascade tags `X.Y.Z`, `X.Y`, `X`, `latest` land in semver order); per `(version, platform)` unit, incremental (skip what the target holds), idempotent [src: `docs/index.md`; `adr_ocx_mirror.md` Requirements 2,5].
- Tags: `build_timestamp` suffix `datetime|date|none` (`mirror-yml.md:21`); canonical `sha256.<hex>` tags (plan doc `plan_mirror_capability_cut.md`).
- Run model: `pipeline generate ci` renders GitHub Actions workflows (scheduled discovery, smoke test per pair, then publish, Discord report), plus `cascade.yml` repair on an optional cron and `verify-generated.yml` drift guard [src: `docs/getting-started.md:85-117`; `mirror-yml.md:717-730`]. `versions.poll_interval` is a cron string passed through (`mirror-yml.md:726`).
- Staleness and drift: `pipeline plan` reports `metadata-drift`; `pipeline patch` repoints metadata over a version range without re-uploading layers; `pipeline announce` re-announces from the registry (`mirror-yml.md:511`; `plan_mirror_capability_cut.md` Status and table). Generated-workflow drift: `plan_mirror_pin_drift_renovate.md` (State: in-progress, 2026-06-07).
- Corporate extras: `url_rewrite` sends downloads through a proxy (`docs/index.md`).
- (B) equivalents: `registry sync` copies every package of upstream OCX indexes by digest into a registry you control and writes a rewritten index tree (`destination:` template with `{registry}/{namespace}/{package}`, collision refusal); `dist sync` mirrors ocx's own release archives plus `dist.json` to a generic HTTP store [src: `docs/index.md`; `docs/reference/registry-yml.md:63,162-199`; `adr_registry_mirror_sync.md:Context`; `adr_dist_mirror_sync.md`].
- Namespace: first path segment of `target.repository` must be the mirror repository's own owner (`GITHUB_TOKEN` scope); logical index package `<namespace>/<package>` is "related by convention, not by a rule" (`mirror-yml.md:37-53`). Republish under `ghcr.io/ocx-contrib/*` with two-segment names is ocx-mirror#18 and was explicitly out of scope of the capability cut (`plan_mirror_capability_cut.md` "Explicitly NOT in scope").

### 2.2 Recorded decisions, by topic

| Topic | Decision (record) | Cite |
|---|---|---|
| Signing: produce | Mirror signs what it builds with its own identity; `sign:` block in `mirror.yml` (keyless or key; `Ref` grammar literal / `env://` / `file://`); platform manifests signed inline by push, indexes by a sweep | `ocx-mirror/.claude/artifacts/adr_mirror_signing.md:53,277-300,454-` (D1, D2) |
| Signing: carry | Upstream signatures and referrers copied verbatim; destination digest equals source digest; copy adds no signature and never verifies at copy time; subject lands first, then referrers (depth ceiling 2, count ceiling 64/subject), then `.sig/.att/.sbom` sidecar tags byte-for-byte | `adr_mirror_signing.md:53-54,577-640` (D3), C-062 at `:1210` |
| Changed or multiple upstream signatures | Sidecar tag already at a different digest at the destination: skipped and counted as `sidecar_conflicts`, package still succeeds; cosign merge semantics for several signatures on one `.sig` tag "are not documented", so merging is refused as an option. ECR refuses referrer manifests: package fails `SubjectRejected` | `adr_mirror_signing.md:610-625` |
| Signing: backfill | `ocx-mirror package pipeline sign` signs published subjects that carry no signature (D4) | `adr_mirror_signing.md:752-` |
| Fail-closed | A silently unsigned package and a silently dropped signature "are the same defect class" | `adr_mirror_signing.md` Decision Drivers |
| Trust anchor | In ocx, a mirror or proxy is never a trust point; clients verify each package against index-declared `publishers` and the mirror holds no signing identity | `ocx/.claude/artifacts/adr_publisher_trust_distribution.md:26-32,148` (T2); `system_design_publisher_trust.md` |
| SBOM / sidecars | `.sbom` sidecars travel as ordinary tags verbatim (C-062). CHANGELOG 0.6.5: "Backfill signatures and SBOMs of published versions (release)" and per-target SBOM fixes concern ocx-mirror's own release artifacts [inf] not mirrored packages | `ocx-mirror/CHANGELOG.md:15,37,43`; `ocx/.claude/artifacts/adr_sbom_attestations.md` Status: Proposed |
| Lock portability | ocx lock records logical identity only; rewritten `repository` lives in index documents, so `ocx.lock` is byte-identical between mirrored and direct hosts. A design recording the physical address was Rejected 2026-07-30 (`adr_lock_records_physical_address.md`) | `ocx-mirror/.claude/artifacts/research_mirror_lock_portability.md:1-35` |
| Client-side redirect | ocx `[mirrors]` per-host rewrite, replace semantics, digest-verified (Cargo-style) | `ocx/.claude/artifacts/adr_oci_registry_mirror.md:153-155`; error handling `bugfix_plan_327_mirror_provenance.md` (despite the name it is about redirect/HTML-manifest/digest-error classification, not provenance) |
| Namespace ownership | Owner-of-repo segment rule above; `ocx-contrib` republish deferred (#18); index claim command is Draft | `mirror-yml.md:53`; `ocx/.claude/artifacts/system_design_index_claim_command.md` (Draft 2026-09-05) |
| Licensing | No decision recorded. `mirror.yml` has an `annotations` surface (example `org.opencontainers.image.licenses: Apache-2.0`, `mirror-yml.md:1279`); no ADR on licence checking, redistribution or attribution found in either repo's artifacts [negative; grep of `-i licen` in the ocx-mirror artifacts hit only Cargo `license.workspace` and unrelated lines] | |

### 2.3 Implementation status

| Item | Record status | Code / release evidence |
|---|---|---|
| `adr_ocx_mirror` (A: sources, spec) | Proposed (2026-03) | Shipped: crates `ocx_mirror_source` (`github_release.rs`, `url_index.rs`, `pypi.rs`), `ocx_mirror_spec` [src: crate listing] |
| `adr_mirror_source_generators` | Proposed (2026-03-14) | `generator.rs` present in `ocx_mirror_source/src` [src] |
| `adr_registry_mirror_sync` (B) | Proposed (2026-08-13) | Shipped: `src/command/registry/`, `registry_sync.rs`, `registry_copy.rs`; CHANGELOG 0.5.4 "Document the registry sync verb and registry.yml" (`CHANGELOG.md:235,270`) |
| `adr_dist_mirror_sync` | Accepted (2026-08-18) | `src/command/dist/sync.rs` [src] |
| `adr_mirror_signing` (D1-D6) | Proposed (2026-09-02) | Shipped in 0.6.1 (2026-09-15): CHANGELOG `:134-150` "Parse and validate the sign: block", "Sign every leg that publishes, and the index above it", "Carry signatures and attestations across the mirror", "Sign published subjects that carry no signature"; `ReferrersPresent` removed (`registry_sync/tests.rs:187`); `sign_backfill.rs`, `sign_config/` present |
| Capability cut (patch, multi-spec) | Status block: PR #41 open, awaiting owner merge, 2026-07-28 | later releases 0.5.x to 0.6.5; PR state not verified [inf] |
| Publisher trust (ocx) | Draft, slice 1 (rewritten 2026-11-13) | not checked in ocx code |
| SBOM attestations (ocx) | Proposed | `adr_sbom_strategy`: partially implemented |
| Latest ocx-mirror release | | 0.6.5, 2026-11-15 (`CHANGELOG.md:8`) |

Record-status caveat: ocx-mirror ADR files stay "Proposed" after shipping, so status needs the CHANGELOG, not the ADR header.

### 2.4 Compatibility: ocx-mirror decision vs grimoire contract (facts only)

| ocx-mirror decision | Grimoire contract found | Relation |
|---|---|---|
| Mirror signs its own output with its own identity | No signing exists; trust model treats artifacts as trusted-authority-signed until signatures arrive, and says revisit then (`adr_artifact_trust_model.md:119,155`) | Silent (grimoire has no verification surface to conflict with); the trust ADR names itself as the revision point |
| Carry referrers and sidecars verbatim, destination digest equals source | `adr_push_pull_registry_split.md:60-70` assumes identical digests across names; `adr_artifact_dependencies.md:228` notes uneven mirroring of referrers; `__grimoire` companion survives only full-repo copy (`publishing.md:317-325`) | Compatible on digest identity; silent on referrers; companion carriage is a documented caveat |
| Never verify at copy time | Integrity-only trust, no verification step (`adr_artifact_trust_model.md:119-126`) | Compatible |
| Lock records logical identity, host rewrite lives in index | Grim lock embeds host/repo in `pinned` and bundle refs (`grimoire.lock:10-14`, `locked_source.rs:43`); no client rewrite config; relative refs follow a mirror (`adr_artifact_dependencies.md:412-421`); `lock keeps pins` frozen (`stability.md`) | Conflicts for absolute refs under a registry rewrite (grim's lock cannot stay byte-identical across a host change); compatible for relative refs |
| Client-side `[mirrors]` rewrite (replace semantics) | None in grim | Silent (no contract; a new config key would fall under the additive-only rule) |
| Tags: cascade `X.Y.Z`/`X.Y`/`X`/`latest` plus build timestamp suffix | OCI wire format and release/push mechanics frozen incl. "a cascading release never moves a float off a higher release" (`stability.md`) | Compatible only if grim's release path is used; an external tag scheme is silent |
| Namespace = owner of the mirror repository; logical name by convention | Index namespace = announcing forge identity with live `owner.id` check; first accepted PR creates the namespace (`package-index.md:259-312`) | Compatible for a mirror announcing under its own account; silent on a third party republishing a package that carries another author's name |
| Index `ref` may point at a rewritten repository | Index `ref` is a tag-less OCI ref and the "only source a browse reads" (`package-index.md`) | Compatible |
| Licence not recorded | Index row carries `license` mirrored from `org.opencontainers.image.licenses` (`package-index.md:296`) | Silent on both sides |
| Scheduled poll plus announce from registry | Index stores pointers, never versions, tags resolve live (`package-index.md:24-27`) | Compatible |
| Fail closed on a dropped signature | Trust ADR: absence of signature story is "known, accepted" | Silent |
| `.sig/.att/.sbom` sidecar tag conflict is skipped, not refused | No grim record | Silent |

## Negative (dead ends, contradicting evidence)

- negative: no "mirror" feature, config key, ADR, plan or discussion decision in grimoire other than the three discussions above and the push/pull ADR; the discussion hits in `marketplace-phase-2.md:77` and `harness-native-marketplaces.md:53-56` use "mirror" for the harness projection (C), not (A) or (B).
- negative: `.agents/adr/` has no signing, cosign, referrer or SBOM ADR; the only forward statement is `adr_artifact_trust_model.md:155-160`.
- negative: no grimoire record of an index size number or the "too full" complaint beyond `harness-native-marketplaces.md:56`; `index-structure.md` and `index-scale-rollout.md` discuss scale technically, not curation.
- negative: no licensing decision in ocx-mirror or its sibling records.
- negative: ocx-mirror ADRs "Proposed" despite shipped code; do not read status from headers.
- negative: `bugfix_plan_327_mirror_provenance.md` is not about provenance of mirrored content (it classifies redirect and digest errors on the ocx `[mirrors]` path).
- negative: ocx-mirror docs and `docs/reference` were sampled, not read in full; ADR bodies of `adr_ocx_mirror`, `adr_registry_mirror_sync` and `adr_dist_mirror_sync` were read for header and context only. Items with no `file:line` are the `Decision Outcome` D4-D6 summaries.

## Leads

- leads: `ocx-mirror/.claude/artifacts/adr_registry_mirror_sync.md:Decisions` (1631 lines) and its owner-rejected "lock records physical address" counterpart; the full B-side design (destination templates, collision refusal, SSRF notes) was not read past Context.
- leads: ocx `.agents/discussions/signing-dogfood-mirror.md` and `release-signing-trust-default-2026-11-12.md` (ocx repo): newest owner statements on mirror signing identity and "mirror is never a trust point".
- leads: `ocx/.claude/artifacts/system_design_index_claim_command.md` and `design_spec_announce_initiative.md` (Ratified 2026-07-22): how ocx lets a third party claim and announce a namespace, relevant to grim's `--announce` ownership gate under republish.
- leads: harness plugin source formats (`research_plugin_format_compat_a.md:92`, `research_plugin_schemes.md`) for the ingest side of (A): git `sha` pinning, subdir sources.
- leads: licence and redistribution rules for republishing third-party skills (no record in either repo; community packs research `research_community_skill_packs.md` notes `license` frontmatter norm).
- leads: OCI registry behaviour for copy (Referrers API support, ECR 405, zot GC of untagged referrers) from `adr_mirror_signing.md` D3/D6 and `ocx/.claude/artifacts/research_oci_referrers_2026.md`, relevant if grim ever carries signatures.
- leads: grim `lock` host-embedding vs a consumer rewrite is the single largest contract question for (B); `.agents/discussions/lock-pin-stability.md` and `adr_install_state_portability.md` were not read for it.
