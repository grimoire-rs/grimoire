# Research: Artifact dependencies — forward-compat signalling

## Metadata

- **Date**: 2026-11-01
- **Domain**: packaging
- **Triggered by**: `.agents/adr/adr_artifact_dependencies.md` (in design)
- **Expires**: 2027-05-01
- **Axis**: data model / compatibility (OCI spec evolution; package-manager UX and lockfile design)
- **Method**: primary specs and live source fetched 2026-11-01. A source is marked **[OLD]** when it is older than ~18 months (before 2025-05) and should be re-verified before the ADR cites it. Items marked **[UNVERIFIED]** could not be confirmed from a primary source.

## Question

How does a newer artifact tell an older `grim` to refuse instead of silently degrading, and how does `grimoire.lock` gain a new provenance kind, under an additive-only freeze?

## The constraint that decides everything

Any gate has to be enforced by clients that are **already shipped** (0.15.x and earlier). A new `requires-grim >= X` field is read only by clients that already know the field, so it cannot protect the clients that need protecting. The mechanisms below split into two kinds:

1. **Fail-closed by existing parsing**: the old client refuses because its parser rejects what it does not recognise.
2. **Declared by the artifact**: a field the reader must understand. It works only once readers exist.

In-repo facts that fix the starting position (read 2026-11-01):

- `LockVersion` is a closed `serde_repr` enum. Its source says "an unknown discriminant fails deserialization at the serde layer (no silent fallback)" (`src/lock/lock_version.rs`).
- `grimoire_lock.rs`, `locked_artifact.rs` and `locked_bundle.rs` use `#[serde(deny_unknown_fields)]`, so a new lock key already makes an old grim fail (`src/lock/`; `research_artifact_dependencies_codebase.md` line 30).
- grim derives artifact kind from OCI `artifactType`, falling back to a `application/vnd.grimoire.<kind>.config.v1+json` config media type (`src/oci/annotations.rs` ~L369-381).

So old grim is already fail-closed for lock and manifest shape. What it probably lacks is a **useful message**; the refusal is likely an opaque TOML or serde parse error ("TOML parse failure (also fires on `deny_unknown_fields`...)", `src/lock/lock_error.rs:67`).

## 1. Feature-list gates

| Mechanism | Signal | Old-client failure mode | Precision | Adoption | Pitfalls |
|---|---|---|---|---|---|
| git `core.repositoryformatversion` + `extensions.*` | Version 1 plus named extension keys | Hard refuse. Spec: an implementation that does not understand the version "MUST NOT operate on that repository", and "if a version-1 repository specifies any `extensions.*` keys that the running git has not implemented, the operation MUST NOT proceed." ([repository-version](https://git-scm.com/docs/repository-version)) | High: per-feature, name-keyed, the value is checked too | Used for `objectFormat`, `refStorage`, `worktreeConfig`, `partialClone`, `preciousObjects` ([extensions.adoc](https://github.com/git/git/blob/master/Documentation/config/extensions.adoc)) | Bumping to 1 with no extensions "provides no benefit, and makes the repository incompatible with older implementations" ([repository-version](https://git-scm.com/docs/repository-version)). Three extensions (`noop`, `partialClone`, `preciousObjects`) are honoured even at version 0 "for historical reasons" ([extensions.adoc](https://github.com/git/git/blob/master/Documentation/config/extensions.adoc)): proof that retrofitting the gate onto already-shipped clients leaves holes. Claiming a name needs a note in git-config. |
| Cargo `cargo-features` (nightly) and edition gating | Opt-in list at top of `Cargo.toml`; unknown newer features and editions are errors | Stable cargo says "feature `X` is required ... not stabilized in this version of Cargo ... Consider trying a newer version of Cargo"; an unknown edition errors with "this version of Cargo is older than the `N` edition" ([cargo features.rs](https://github.com/rust-lang/cargo/blob/master/src/workspace/features.rs)) | High and explicit, but only for features that were registered with this mechanism | Cargo-specific. The reference documents it as nightly-only ([unstable.html#cargo-features](https://doc.rust-lang.org/cargo/reference/unstable.html#cargo-features)) | Plain new manifest keys are **not** gated: older Cargo "will give an 'unused manifest key' warning, but will otherwise function without erroring" ([unstable.html](https://doc.rust-lang.org/cargo/reference/unstable.html)). Cargo has both behaviours, chosen per feature. |
| Cargo `package.rust-version` (MSRV) | Minimum toolchain, bare version | Error on an unsupported toolchain, bypassed with `--ignore-rust-version`; changing it is "assumed to be a minor incompatibility" ([rust-version](https://doc.rust-lang.org/cargo/reference/rust-version.html)) | Coarse (one number) | Near-universal in the Rust ecosystem; respected since 1.56 | Only works for cargo >= 1.56. The MSRV-aware resolver prefers dependency versions at or below your `rust-version`; with `fallback` "the resolver won't error but will instead pick a version, even if its potentially suboptimal"; default flips to `fallback` under `resolver = "3"` (edition 2024, Rust 1.84+) ([resolver.html#rust-version](https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version)). That is a soft, resolver-side use of the same field. |
| Kubernetes feature gates | Component flag set by the operator | n/a | n/a | Not a fit: `--feature-gates` is set per component by the cluster operator ([feature-gates](https://kubernetes.io/docs/reference/command-line-tools-reference/feature-gates/)); manifests do not declare required gates | Skip. |

## 2. Minimum-client-version fields

| Mechanism | Enforcement | Notes |
|---|---|---|
| npm `engines` | Advisory. "Unless the user has set the engine-strict config flag, this field is advisory only", and it "will only produce warnings when your package is installed as a dependency" ([package-json#engines](https://docs.npmjs.com/cli/v10/configuring-npm/package-json#engines)) **[OLD: v10 docs]** | Textbook silent degradation: a warning scrolls past. |
| Terraform `required_version` | Hard. "Terraform prints an error and exits without taking actions." Modules carry their own constraint and all must be satisfied ([terraform](https://developer.hashicorp.com/terraform/language/terraform#specifying-a-required-terraform-version)) | Applies to the CLI only; providers have a separate constraint. |
| Go `go` directive | Hard since 1.21: "The Go toolchain refuses to load a module or workspace that declares a minimum required Go version greater than the toolchain's own version." Before 1.21 the line was advisory. Partly backported: 1.19.13+ and 1.20.8+ refuse modules declaring Go 1.22+. `GOTOOLCHAIN=auto` downloads the newer toolchain ([go.dev/doc/toolchain](https://go.dev/doc/toolchain)) **[OLD: 2023]** | The clearest case of a field advisory for years becoming mandatory. The backport shows the cost: pre-1.21 toolchains cannot be taught and keep misreading the line. Auto-download turns the refusal into a self-heal, which a package manager that is a single binary can only approximate (self-update hint). |
| Python `Requires-Python` | Resolver-side skip. Installers "may look at this when picking which version of a project to install" ([core-metadata](https://packaging.python.org/en/latest/specifications/core-metadata/#requires-python)); simple-index `data-requires-python` says installers SHOULD skip the download ([PEP 503](https://peps.python.org/pep-0503/)) **[OLD: 2015, field added 2016]**; override `--ignore-requires-python` ([pip install](https://pip.pypa.io/en/stable/cli/pip_install/)) | Strongest ecosystem evidence that the field works because the resolver **skips** an incompatible release and picks an older compatible one, rather than failing. This is the "optional upgrade, not breakage" path. |
| VS Code `engines.vscode` | Install-time compatibility filter. The field makes the extension install "only ... for clients that contain the API you depend on"; `^1.9.0` means 1.9.0 and later ([publishing-extension](https://code.visualstudio.com/api/working-with-extensions/publishing-extension)) | The docs do not say whether an incompatible version is hidden or errors **[UNVERIFIED]**. |
| Helm `apiVersion` / `kubeVersion` | `apiVersion: v2` is "for Helm charts that require at least Helm 3"; `kubeVersion` is checked at install and fails if the cluster is unsupported ([charts](https://helm.sh/docs/topics/charts/)) **[page notes it is not updated for Helm 4]** | What Helm 2 does with a v2 chart is not stated **[UNVERIFIED]**. `apiVersion` is the schema discriminant and `kubeVersion` the runtime range: two separate fields for two separate questions. |
| Claude Code plugin manifest | No minimum-client field in the manifest reference. Unknown top-level keys are **stripped** (plugin loads), but unknown keys inside strict objects (`userConfig`, `channels`, `lspServers`, `monitors`) are an error. Version-gated features are documented in prose: "If you declare `options` on any field, users on Claude Code versions before v2.1.271 can't load the plugin." ([manifest reference](https://code.claude.com/docs/en/plugins-reference)) | Vendor reality: version-gating is by strict-parser accident, not by declaration. `plugin.json` already has a `dependencies` array with semver `version` ranges, resolved against git tags ([dependencies](https://code.claude.com/docs/en/plugins/dependencies.md)). Any grim dependency model that exports to Claude should map onto it. |
| Nix | No mechanism found. The nix.dev conf-file reference describes no minimum-version setting, `nixConfig` or `flake.lock` version field ([conf-file](https://nix.dev/manual/nix/stable/command-ref/conf-file.html)). Older tooling rejects newer `flake.lock` versions through shims that "throw unsupported version" ([gist](https://gist.github.com/jtojnar/fd8818ba2d5eb20df20557cb0b623ed1)) **[secondary source]** | Not a model to copy. |

## 3. OCI media type and artifactType handling

What the spec says (image-spec, manifest.md):

- Implementations storing or copying manifests "MUST NOT error on encountering" an unknown `artifactType`, an unknown `config.mediaType`, or an unknown layer `mediaType`. For an unknown config media type they "MUST NOT attempt to parse the referenced content" and should treat it as opaque data. `artifactType` MUST be set when `config.mediaType` is the empty value ([manifest.md](https://github.com/opencontainers/image-spec/blob/main/manifest.md)).
- Unknown properties: implementations "SHOULD NOT generate an error if they encounter an unknown property in a known media type" ([considerations.md](https://github.com/opencontainers/image-spec/blob/main/considerations.md)). Unknown annotation keys: consumers "MUST NOT generate an error" ([annotations.md](https://github.com/opencontainers/image-spec/blob/main/annotations.md)).
- 1.1 added `subject` and `artifactType`, plus the referrers API with a fallback tag for registries lacking it. No new manifest media type was introduced, so older registries do not reject by manifest type. New fields "should be ignored by registries without generating errors" ([OCI 1.1 announcement](https://opencontainers.org/posts/blog/2024-03-13-image-and-distribution-1-1/)) **[OLD: Feb-Mar 2024]**.

So the norm is **"ignore unknown" for transport** (registries, copy tools), and the spec's tolerance is aimed at them. **Interpreting clients decide for themselves**, and the ones that act on content refuse:

- Helm pulls only an allowlist of media types and errors with "could not load config with mediatype ..." when the config descriptor is absent; its pull path whitelists the config, chart and provenance layer types ([helm pkg/registry/client.go](https://github.com/helm/helm/blob/main/pkg/registry/client.go)). Real-world fallout: Helm 3.18 rejected a provenance layer that 3.17.3 accepted ([opendesk issue 187](https://gitlab.opencode.de/bmi/opendesk/deployment/opendesk/-/issues/187)).
- containers/image raises `NonImageArtifactError` ("unsupported image-specific operation on artifact with type ...") and keys it on `artifactType`, falling back to config media type; its source comment says `artifactType` is "more relevant because config may be ~absent" ([errors.go](https://github.com/containers/image/blob/main/internal/manifest/errors.go)).
- Docker/containerd pulling an artifact with an empty config reported "unsupported media type application/vnd.oci.empty.v1+json" late rather than early ([image-spec issue 1131](https://github.com/opencontainers/image-spec/issues/1131)) **[secondary]**.
- Cosign, Flux, oras: handling of unknown `artifactType` or layer types **not confirmed from a primary source [UNVERIFIED]**. Flux `layerSelector` selects by media type with the first layer as default ([CRD bindings](https://docs.rs/crate/kcr_source_toolkit_fluxcd_io/latest/source/src/v1/ocirepositories.rs)); that says nothing about unknown types.
- Registry caveat: ECR was reported rejecting a manifest with the Sigstore bundle media type ([protobuf-specs 165](https://www.github.com/sigstore/protobuf-specs/issues/165)) **[secondary]**. New *manifest-level* or layer media types can hit registry allowlists; a new *config* media type or `artifactType` value on an existing manifest media type is the lowest-risk shape.

Implication for grim: an interpreter that selects behaviour from `artifactType` or a config media type version is following established practice (Helm, containers/image) when it refuses an unknown value. The OCI "MUST NOT error" text binds copy and storage paths such as `grim` mirror or publish-copy code, not install.

## 4. Lockfile evolution

| Tool | Scheme | Old-reader behaviour | Lesson |
|---|---|---|---|
| Cargo.lock | Integer `version`. V1 had none; V2 introduced 1.38 (default 1.41-1.52); V3 introduced 1.47 (default 1.53-1.82); V4 introduced 1.78 (default 1.83+) ([resolve.rs](https://github.com/rust-lang/cargo/blob/master/src/resolver/resolve.rs)) | Hard refuse with a purpose-written message: "lock file version `N` was found, but this version of Cargo does not understand this lock file, perhaps Cargo needs to be updated?" ([encode.rs](https://github.com/rust-lang/cargo/blob/master/src/resolver/encode.rs)) | Reader first, writer later: a new version "is not updated [as default] until *at least* the support for the version is in the stable release of Rust"; an existing lockfile stays as is until Cargo is rewriting it anyway; the default is MSRV-aware (`with_rust_version`). V4 stabilised in 1.78 and became default in 1.83, about 5 months ([PR 14595](https://github.com/rust-lang/cargo/pull/14595)). Cost still showed up: downstream tools broke on v4 (cargo-audit, cargo-outdated, rust-cache; same PR thread). |
| uv.lock | `version` (major) plus `revision` (additive). "A given uv version only supports a single major version"; revision changes are backwards-compatible, and a reader on a lower revision "will be able to read lockfiles with a revision greater than 1 (though they may ignore newer fields)". Live source: revision 5 ([uv-lock lock/mod.rs](https://github.com/astral-sh/uv/blob/main/crates/uv-lock/src/lock/mod.rs)) | Major mismatch gives `UnparsableVersion`/`UnsupportedVersion` errors. Lower-revision readers silently drop newer fields | Clean split of "must refuse" from "may ignore", but it **requires a lenient parser from day one**. Each revision adds data only a newer uv produces and can drop on rewrite (issue [uv#15220](https://github.com/astral-sh/uv/issues/15220), via secondary summary), which is acceptable only when the dropped data is derivable. |
| package-lock.json | `lockfileVersion` 1 (npm 5-6), 2 (npm 7-8, "Backwards compatible to v1"), 3 (npm 9+, "Backwards compatible to npm v7") ([package-lock-json](https://docs.npmjs.com/cli/v10/configuring-npm/package-lock-json#lockfileversion)) **[OLD: v10 docs]** | "npm will always attempt to get whatever data it can out of a lockfile, even if it is not a version that it was designed to support" | The opposite posture: best-effort read. v2 carried both old and new sections to stay readable (extra size), v3 dropped that. Best-effort suits a cache that npm can rebuild, not a security-relevant pin. |
| go.mod / go.sum | `go` line as minimum (see section 2); checksums live in go.sum and are untouched by toolchain changes ([go.dev/doc/toolchain](https://go.dev/doc/toolchain)) **[OLD: 2023]** | Go >= 1.21 refuses, toolchain auto-download | Version gate lives in the manifest, not the lock. |
| flake.lock | Integer `version` (7 current) | Old tooling errors "unsupported version" (shim) | Same fail-closed pattern; bumps are treated as disruptive (Lix issue 472, [thread](https://git.lix.systems/lix-project/lix/issues/472)) **[secondary]**. |
| Python wheels / core metadata | `Wheel-Version`, `Metadata-Version`: "Warn if minor version is greater, abort if major version is greater" ([binary-distribution-format](https://packaging.python.org/en/latest/specifications/binary-distribution-format/)); metadata: SHOULD warn on newer, MUST fail on newer major ([core-metadata](https://packaging.python.org/en/latest/specifications/core-metadata/)) | Major refuses, minor warns | Spec-level major/minor semantics, the uv model written as a standard. |

## 5. Required-versus-optional extension markers

- **JOSE `crit`** (RFC 7515 4.1.11): the header lists extension parameters that "MUST be understood and processed"; if any listed one is not understood and supported, "the JWS is invalid." It is integrity-protected, producers MUST NOT list names the base spec defines, duplicates or absent names, and MUST NOT use an empty list ([RFC 7515](https://www.rfc-editor.org/rfc/rfc7515#section-4.1.11)) **[OLD: 2015, stable]**. Pitfall: enforcement is the reader's job. PyJWT before 2.12.0 and python-jose 3.5.0 ignored `crit` and accepted tokens with unknown critical extensions (CVE-2026-32597 / [GHSA-752w-5fwx-jx9f](https://osv.dev/GHSA-752w-5fwx-jx9f), [python-jose#413](https://github.com/mpdavis/python-jose/issues/413), via search summaries **[secondary]**). A "must understand" list is only as strong as the weakest reader.
- **PNG chunk case bits**: first-letter case marks critical (uppercase) vs ancillary (lowercase); a decoder meeting an unknown critical chunk "shall indicate to the user" that the data cannot be safely interpreted, and may ignore an unknown ancillary one. A separate safe-to-copy bit tells *editors* whether to preserve an unknown chunk ([PNG spec 5.4](https://www.w3.org/TR/png/#5Chunk-naming-conventions)). Notable: criticality is in the name, so it needs no extra field and cannot drift.
- **git extensions** (section 1): required-by-default for any named extension at format version 1.
- **Cargo `cargo-features`** (section 1): an explicit opt-in list; nothing outside the list can break an older reader.
- **Claude Code plugin manifest**: ancillary-by-default for top-level keys (stripped), critical-by-default inside strict objects (section 2).

The shared design: **ancillary is the default, and a producer opts into critical by listing the feature by name** (JOSE, Cargo). Git inverts it: any extension is critical. The inversion matters for grim because the freeze says new fields must be additive and default-able, which maps to "ancillary unless listed."

## Adjacent options

- **RubyGems** `required_ruby_version` / `required_rubygems_version`: installer raises an install error if the requirement is not met ([Installer#ensure_required_rubygems_version_met](https://apidock.com/ruby/v2_2_9/Gem/Installer/ensure_required_rubygems_version_met)) **[OLD: apidock page for Ruby 2.2]**. A proposed RFC to make `required_ruby_version` mandatory notes many gems leave it unset, and the only fix for an already published gem is yanking it ([rubygems/rfcs](https://github.com/rubygems/rfcs/blob/4b65b604817c2750dfa90d0c8dfdd83917f9fc5a/text/0000_require_minimal_ruby_version.md)) **[OLD; adoption status unknown]**. Lesson: an optional gate field left empty is no gate; cargo's `rust-version` shares the weakness.
- **Config media type versioning** (`...config.v1+json` to `...v2+json`): Helm treats the config media type as the authoritative chart marker ([deckhouse operator-helm PR 68](https://github.com/deckhouse/operator-helm/pull/68), secondary). Combined with image-spec's "unknown config media type: do not parse" this is an OCI-native hard gate against any client that matches on the type.
- **`artifactType` as discriminator**: containers/image prefers it over config media type when present (see section 3). Both signals exist in grim's reader today.

## Decision-relevant observations

1. **Two jobs, two mechanisms.** (a) *Stop clients already shipped*: only existing fail-closed parsing works (lock `deny_unknown_fields`/closed version enum; unknown `artifactType` or config media type resolving to no known kind). (b) *Explain and future-proof*: a declared field read by clients that ship from now on (`requires`-list, minimum version), plus a purpose-written error message.
2. **Silent degradation is the dominant failure.** npm advisory `engines`, Go before 1.21, Cargo unknown manifest keys and the Claude Code stripped top-level keys all degrade silently. Go and Cargo both moved toward refusal for things that change meaning.
3. **Refuse-with-message beats refuse-with-parse-error.** Cargo's lock error carried the "perhaps Cargo needs to be updated?" hint long before it was needed (v3-era); the reader shipped first. grim's lock error today is generic.
4. **Lazy emission.** Cargo leaves an existing lockfile at its version until it is rewriting anyway and honours MSRV; the writer only bumps when needed. That keeps old clients working on repos that use no new feature.
5. **Lenient-reader revision schemes (uv, npm) cannot be retrofitted** onto a `deny_unknown_fields` reader. They only help for the *next* format step.

## Recommendation

1. **Fail-closed gate on shipped clients: a new config media type version and/or new `artifactType` for dependency-capable artifacts, not an annotation.** Annotations are "MUST NOT error" territory in the spec, so old grim ignores them. A new `application/vnd.grimoire.<kind>.config.v2+json` (and/or a new `artifactType` value) keeps the manifest media type unchanged (no registry allowlist risk, per the 1.1 design) and makes a kind-resolving reader fail on an unknown kind. **Before the ADR relies on it, publish a fixture with each signal and run a stock 0.15.x `grim`** to see which one refuses and with what message. I could not determine that from outside the binary.
2. **Declared gate for future clients: a `crit`-style list, ancillary by default.** Put `requires: ["dependencies"]` (feature names, not versions) inside the integrity-covered config blob, mirroring JOSE `crit` and git `extensions`: unknown names mean refuse, no name means the reader may ignore the new field. Pair with an advisory `min-grim` for the error message. Do not make a bare version number the gate: it cannot be enforced on old clients and an unset field is no gate (RubyGems).
3. **Ship the reader before the writer.** Cut a 0.15.x/0.16 patch that (a) turns unknown lock `version`, unknown lock keys and unknown kind into a single "this was produced by a newer grim; upgrade" error, and (b) tolerates the new config shape read-only. Hold dependency *publishing* until that release has been out for a Cargo-like period (about 5 months for V4; pick the window in the plan).
4. **Lockfile: stay on `version = 1` for files with no dependency edges; emit the new provenance kind only when an edge exists, and in that case bump `version`.** An old grim refuses either way (closed enum plus `deny_unknown_fields`), but a version bump gives the clearer, cheaper-to-test refusal and lets the new reader branch cleanly. Adopt uv's `version`/`revision` split for the step after this one: major means refuse, revision means additive and ignorable, with a lenient parser for the revision-additive parts only.
5. **Do not rely on `ignore unknown`.** For install and lock paths grim is an interpreter, where Helm and containers/image refuse. Keep OCI's tolerant "MUST NOT error" for copy, mirror and list paths.

## Open questions for the ADR

- What does stock 0.15.x grim print and which exit code does it return for (a) an unknown config media type, (b) an unknown `artifactType`, (c) an unknown lock key, (d) `version = 2`? This is a one-hour fixture test and decides recommendation 1.
- Do `grim describe`, `grim search` and the TUI (which render manifests but do not install) refuse or degrade on the new kind? "Refuse everywhere" and "refuse on install, degrade on browse" are both defensible.
- Does the Claude Code `dependencies` field (name plus semver range resolved by git tag) constrain how grim should express edges, so that exports map one to one?

## Sources

- https://git-scm.com/docs/repository-version — format version 0/1, MUST NOT proceed on unknown extensions
- https://github.com/git/git/blob/master/Documentation/config/extensions.adoc — extension list; historical exceptions at version 0
- https://doc.rust-lang.org/cargo/reference/unstable.html#cargo-features — `cargo-features`; unknown keys only warn
- https://github.com/rust-lang/cargo/blob/master/src/workspace/features.rs — "feature is required" and edition error text
- https://doc.rust-lang.org/cargo/reference/rust-version.html — `rust-version` semantics
- https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version — MSRV-aware resolver, `fallback`, resolver 3
- https://github.com/rust-lang/cargo/blob/master/src/resolver/resolve.rs — lockfile V1-V4 history, default policy
- https://github.com/rust-lang/cargo/blob/master/src/resolver/encode.rs — "does not understand this lock file" message
- https://github.com/rust-lang/cargo/pull/14595 — lockfile v4 default (1.83) after 1.78 stabilisation
- https://github.com/astral-sh/uv/blob/main/crates/uv-lock/src/lock/mod.rs — uv `version` vs `revision` semantics
- https://docs.npmjs.com/cli/v10/configuring-npm/package-json#engines — advisory `engines` [OLD]
- https://docs.npmjs.com/cli/v10/configuring-npm/package-lock-json#lockfileversion — lockfileVersion 1/2/3 [OLD]
- https://go.dev/doc/toolchain — `go` minimum, toolchain auto-download [OLD]
- https://developer.hashicorp.com/terraform/language/terraform — `required_version`
- https://code.visualstudio.com/api/working-with-extensions/publishing-extension — `engines.vscode`
- https://helm.sh/docs/topics/charts/ — apiVersion v1/v2, kubeVersion, dependencies
- https://github.com/helm/helm/blob/main/pkg/registry/client.go — config/layer media type allowlist
- https://code.claude.com/docs/en/plugins-reference — plugin manifest, unknown-field handling
- https://code.claude.com/docs/en/plugins/dependencies.md — plugin `dependencies` with semver ranges
- https://github.com/opencontainers/image-spec/blob/main/manifest.md — artifactType, empty descriptor, unknown media types
- https://github.com/opencontainers/image-spec/blob/main/considerations.md — unknown property guidance
- https://github.com/opencontainers/image-spec/blob/main/annotations.md — unknown annotation keys
- https://github.com/opencontainers/image-spec/blob/main/artifacts-guidance.md — config media type for non-image artifacts
- https://opencontainers.org/posts/blog/2024-03-13-image-and-distribution-1-1/ — 1.1 compatibility notes, fallback tag [OLD]
- https://github.com/containers/image/blob/main/internal/manifest/errors.go — NonImageArtifactError
- https://github.com/opencontainers/image-spec/issues/1131 — artifact pull failure with empty config [secondary]
- https://gitlab.opencode.de/bmi/opendesk/deployment/opendesk/-/issues/187 — Helm 3.18 media type rejection [secondary]
- https://github.com/deckhouse/operator-helm/pull/68 — config media type as chart marker [secondary]
- https://www.github.com/sigstore/protobuf-specs/issues/165 — ECR rejecting bundle media type [secondary]
- https://packaging.python.org/en/latest/specifications/core-metadata/ — Requires-Python, Metadata-Version
- https://packaging.python.org/en/latest/specifications/binary-distribution-format/ — Wheel-Version major/minor rule
- https://peps.python.org/pep-0503/ — data-requires-python [OLD]
- https://pip.pypa.io/en/stable/cli/pip_install/ — `--ignore-requires-python`
- https://www.rfc-editor.org/rfc/rfc7515#section-4.1.11 — JOSE `crit` [OLD, stable]
- https://osv.dev/GHSA-752w-5fwx-jx9f — PyJWT `crit` non-enforcement [secondary]
- https://www.w3.org/TR/png/#5Chunk-naming-conventions — critical vs ancillary chunks
- https://apidock.com/ruby/v2_2_9/Gem/Installer/ensure_required_rubygems_version_met — RubyGems check [OLD]
- https://github.com/rubygems/rfcs/blob/4b65b604817c2750dfa90d0c8dfdd83917f9fc5a/text/0000_require_minimal_ruby_version.md — mandatory `required_ruby_version` proposal [OLD]
- https://kubernetes.io/docs/reference/command-line-tools-reference/feature-gates/ — feature gates are operator-set
- https://nix.dev/manual/nix/stable/command-ref/conf-file.html — no min-version mechanism found
- In-repo: `src/lock/lock_version.rs`, `src/lock/grimoire_lock.rs`, `src/lock/locked_artifact.rs`, `src/lock/lock_error.rs`, `src/oci/annotations.rs`
