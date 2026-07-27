# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.11.1] - 2026-07-23

### Added

- Add a never|auto|always fork policy to announce *(publish)*

### Documentation

- Freeze the remaining 1.0 contract surfaces *(stability)*

### Fixed

- Always serialize grim_render warnings *(mcp)*

## [0.11.0] - 2026-07-22

### Added

- Add color resolution module *(cli)*
- Add --color flag and styled clap help *(cli)*
- Add grim completions command *(cli)*
- Stub wave-1 vendor surface *(install)*
- Thread rule scope, shared-skill refcount guard, client matrix docs *(install)*
- Cursor client renders (rules, agents, mcp) *(install)*
- Gemini client renders (agents, mcp) *(install)*
- Kiro client renders (steering rules, mcp) *(install)*
- Junie client mcp render *(install)*
- Zed client mcp render *(install)*
- Amp client mcp render *(install)*
- Add publishing bootstrapper reference to grim-authoring *(catalog)*
- Auto-fork index repo on announce without push access *(publish)*

### Changed

- Route print_json through shared write_json_pretty *(cli)*
- Bundle materialize params *(install)*
- Universal skill renderer, shared-pool dedup, cursor serializer *(install)*

### Documentation

- Document --color and grim completions
- Document broken-pipe exit-0 convention
- Tidy stale stub comment and registry group label
- Correct 4-vs-10 client drift across docs, catalog, and rules
- Scope the empty-registry claim and disclose the publish-gate narrowing *(adr)*
- Align shipped skills with the ten-client release *(catalog)*

### Fixed

- Count vendor MCP config file as client-detection signal *(install)*
- Gate batch action targets by per-op install state *(tui)*
- Tag stdout broken-pipe errors with a sentinel *(cli)*
- Exit 0 silently when stdout pipe closes *(cli)*
- Pipe-safe schema and completions output *(cli)*
- Best-effort stderr writes in publish batch and tui guards *(cli)*
- Harden vendor-wave renders (opencode scope-warn, cursor glob escaping, error-list drift) *(install)*
- Correct vendor config-path resolution and harden anchor fallbacks *(install)*
- Compile the Zed Windows root test under forbid(unsafe); harden drift guards *(install)*
- Recover from an anchor escape instead of wedging *(install)*
- Carry deprecation through the package index *(catalog)*

## [0.10.0] - 2026-07-19

### Added

- Emit license annotation for rules, agents, bundles, and mcp *(oci)*
- Allow periods in artifact names *(skill)*
- Verify credentials against the registry by default *(login)*
- Recover modified state with --force *(add)*
- Support a push registry distinct from the pull registry *(publish)*
- Add Codex (OpenAI Codex CLI) as a client vendor *(install)*
- Accept "manual" permission-mode *(vendor-claude)*
- Project claude.allowed-tools skill key *(vendor-claude)*
- Project agent model into frontmatter *(vendor-copilot)*
- Warn when dropping agent tools *(vendor-opencode)*
- Map remote MCP headers to http_headers/env_http_headers *(vendor-codex)*
- Additive refinement fields on the descriptor *(mcp)*
- Project refinement fields per vendor *(mcp)*
- Reap moved outputs on re-install (layout-migration migrator) *(install)*
- Route global rules to native ~/.copilot instructions *(vendor-copilot)*
- Ws transport variant *(mcp)*
- Structured oauth block *(mcp)*
- Grim schema --kind mcp *(cli)*
- Add --dry-run to grim config set *(config)*
- No-config and locked reasons, retryable envelope field *(error)*
- Grim config registry fields *(config)*
- Machine-readable constraints on config list entries *(config)*
- Report client-set drift, network-free *(status)*
- Add --check surfacing deprecated and replaced_by *(status)*
- Add update_available via fresh re-resolution under --check *(status)*
- Forward check to grim_status *(mcp)*
- Reap dropped-client outputs on update *(update)*

### Changed

- Rename used param _scope to scope *(vendor-opencode)*
- Extract classify_codex_headers *(vendor-codex)*
- Extract shared tree-separator predicate *(config)*
- Extract update-availability seam from tui *(catalog)*
- Fold autodetect gate into client_drift *(status)*

### Documentation

- Ws + oauth reference, emit matrix, forward-incompat note *(mcp)*
- Correct copilot model row, add allowed-tools, note unknown-fields stance
- Add contributor style rule for config key copy *(config)*
- Document grim status --check, client drift, and update reaping
- Note codex namespace reservation and upgrade impact *(vendor-metadata)*
- Qualify reap/drift as explicit-[options].clients-only
- Autodetect carve-out on report contracts + reap no-op test

### Fixed

- Accept leading ./ in description paths *(publish)*
- Browse the built-in index on the no-scope fallback path *(search)*
- Restore "xhigh" reasoning-effort literal *(vendor-codex)*
- Remove stale MCP entry on pin-change decline *(install)*
- Reaper resolved-identity guard + support-dir reap tests *(install)*
- Decline case-insensitive duplicate header names *(vendor-codex)*
- Validate oauth.auth_server_metadata_url env refs *(mcp)*
- Path-identity hardening for reaper and decline splice *(install)*
- Rewrite key descriptions for plain, user-facing copy *(config)*
- Resolve commit verification state from the target worktree *(hooks)*
- Reap dropped clients only on explicit [options].clients *(update)*
- Gate client drift on explicit [options].clients *(status)*
- Skip fcntl lock test on Windows via skipif decorator *(test)*
- Update stale org in grim-usage search term *(catalog)*

## [0.9.1] - 2026-07-12

### Fixed

- Advertise z fold toggle in the tree-mode hint bar *(tui)*
- Drop logo image from published package readmes *(catalog)*
- Separate changelog link refs from the last list item *(release)*

## [0.9.0] - 2026-07-12

### Added

- Surface curated OCI metadata in the detail pane *(tui)*
- Hide deprecated artifacts by default in search and TUI
- Refuse to clobber untracked destinations *(install)*
- Refuse conflicting same-name re-declaration *(add)*
- Report per-client output paths in --format json *(status)*
- Wrap list reports in an items envelope *(cli)* **BREAKING**
- **Migration:** --format json consumers must read the items array from the envelope object instead of the top-level array; publish JSON key entries is now items. MCP grim_search/grim_status payloads follow.
- Emit structured JSON error document on failure *(cli)* **BREAKING**
- **Migration:** JSON consumers should treat a top-level "error" key on stdout as the error document.
- Preserve the #:schema directive across config rewrites *(config)*
- Seed the #:schema directive in new configs *(init)*
- Add --kind lock for the grimoire.lock schema *(schema)*
- Add read-only resolution introspection command *(context)*
- Port the MCP grim_fetch tool as a CLI command *(fetch)*
- NDJSON progress events via --progress json *(progress)*
- Unify publish/release version and cascade interface *(publish)* **BREAKING**
- **Migration:** `grim publish --tag <x>` is removed; use `grim publish --version <x>` instead. A channel re-publish now requires --force to move an existing tag instead of moving it silently.
- Warn when an install shadows the same name at the other scope *(install)*
- Enforce the artifact-name charset on skill/rule/agent bindings *(add)*
- Add path-source core types for local path dependencies *(config)*
- Parse local path values into DeclaredSource across the pipeline *(config)*
- Pin lock entries and install records to a LockedSource *(lock)*
- Pin, install, and update local path sources end to end *(resolve)*
- Declare local path sources via grim add <path> *(add)*
- Dev-install a local path with grim install <path> *(install)*
- Local-path bundles and TUI Local group *(local-path)*
- Grim describe, replaced_by metadata, and binary fetch payloads *(cli)*
- Annotate stale-lock refusals with a machine-readable reason *(errors)*
- Report per-registry authentication in grim context *(context)*
- Pack well-known README/logo companions into the agent layer *(agents)*
- Carry keywords and summary through the package index *(index)*
- Repository description companion for every artifact kind **BREAKING**
- **Migration:** grim publish now auto-publishes a description companion when conventional files are present; opt out with publish = false under [description] or per-entry description = false.
- Collapse tree to configurable depth on open + z fold toggle *(tui)*
- Typed config-key registry and config list --all with settings metadata *(cli)*
- Closed string-set semantics for options.clients *(cli)*

### Changed

- Collapse redeclared scope flags onto GlobalOptions *(cli)*
- Extract neutral fetch core, break command↔mcp cycle *(fetch)*
- Dedupe error-wrapping into a local helper *(fetch)*
- Typed ErrorReason via unified classify *(errors)*
- Fold typed defaults into ValueType variants *(cli)*
- Add ResolvedOptions view with compile-time tripwire *(config)*

### Documentation

- Document the 1.0 stability contract
- Add JSON interface reference for the 1.0 contract
- Correct frozen-contract drift before 1.0 *(json-interface)*
- Document the two-ceiling fetch download cap *(commands)*
- Record the fetch-service extraction and blob-size hardening *(adr)*
- Link SearchEntry manual serialize count to its guard test *(api)*
- Document config list --all, string-set metadata, and validation

### Fixed

- Keep install status readable in the catalog list *(tui)*
- Aggregate bundle install progress across members *(tui)*
- Show full artifact detail for catalog-backed bundle members *(tui)*
- Release search focus on arrow keys *(tui)*
- Correct ocx catalog link to ocx.sh/catalog/grim *(docs)*
- Exit 79 for missing explicit --config path *(cli)*
- Resolve login/logout through [[registries]] *(registry)*
- Serialize optional report fields as explicit null *(cli)* **BREAKING**
- **Migration:** registry list/show rows always carry both oci and index keys (exactly one non-null); publish announce carries url: null off the pull-request outcome.
- Bound blob download against a lying descriptor (CWE-770) *(oci)*
- Gate install and render against oversized layer descriptors (CWE-770) *(oci)*
- Classify pre-download oversize as a data error *(fetch)*
- Materialize --name-rebound artifacts and rewrite skill name *(install)*
- Harden packing, source-loss status, pins, and dev intent *(local-path)*
- Correct bundle removal, dev-install collision, and status source *(local-path)*
- Close lock-integrity and reverse-order collision gaps *(local-path)*
- Correct stale kind-inference wording and schema help string *(cli)*
- Accept OS-native path separators for local path CLI args *(cli)*

## [0.8.4] - 2026-07-07

### Added

- Enforce repository prefix via --registry host/prefix *(publish)*
- Catalog-wide version with ${version} inheritance and --version override *(publish)*
- Deployment-relative member refs resolved at install *(bundle)*

### Fixed

- Adopt patched oci-client fork to survive an empty trust store *(oci)*
- Dedupe merged registries by normalized locator *(config)*
- Arrow keys always navigate the list, even with detail open *(tui)*

## [0.8.3] - 2026-07-05

### Fixed

- Resolve HOME on Windows for global-scope installs *(install)*

## [0.8.2] - 2026-07-05

### Added

- Install added artifact by default, --no-install to opt out *(add)*

### Fixed

- Include MCP in direct-declared set for via-bundle badge *(tui)*
- Even split for stacked detail pane *(tui)*
- Bundle CA roots so TLS works without a system trust store *(oci)*
- Add missing claude_user_dir to macOS test AnchorRoots *(install)*
- Canonicalize tempdir root in uninstall path assertions *(test)*
- Classify Windows lock contention as TempFail *(lock)*
- Make acceptance suite pass on Windows *(test)*

## [0.8.1] - 2026-07-03

### Added

- Machine-readable announce outcome + GitLab job-token push fallback *(publish)* **BREAKING**
- **Migration:** `grim publish --format json` now emits
- BUMP/VERSION selection for release:prepare *(release)*

### Documentation

- Recommend ocx --global add grim for installation
- Refresh stale pages against 0.8 behavior
- Index-first quick loop in grim-usage (0.8.1) *(catalog)*

### Fixed

- Correct stale help strings and doc comments *(cli)*

## [0.8.0] - 2026-07-02

### Added

- Span-preserving JSON splice editor *(install)*
- Add mcp artifact kind and wire format *(oci)*
- Publish mcp descriptors *(release)*
- Entry-typed client outputs with semantic drift *(install)*
- Register mcp servers in client configs *(install)*
- Publish grim mcp descriptor *(catalog)*
- Per-call scope for MCP tools; drop --global/--config from grim mcp *(mcp)* **BREAKING**
- **Migration:** grim mcp --global / --config exit 64 (the root-level scope flags are rejected with a migration hint). Migration: pass the scope inside each tool call's arguments instead.  Design record: .claude/artifacts/adr_mcp_percall_scope_fetch_render.md. SearchArgs/StatusArgs gain a non-CLI #[arg(skip)] workspace seed; the CLI surface is unchanged. grim_status now takes an all-optional args object; empty-object calls stay valid (serde(default), no deny_unknown_fields — SSRF inert-key contract retained).
- Grim_fetch read tool (in-context artifact content) *(mcp)*
- Grim_render write tool gated behind --allow-writes *(mcp)*

### Changed

- Extract shared managed-JSON parsing helpers *(install)*
- Seedable project-config walk-up and scope resolution *(config)*

### Documentation

- Mcp artifact kind
- Adr for per-call mcp scope and fetch/render tools
- Per-call scope + fetch/render docs and catalog drift *(mcp)*

## [0.7.0] - 2026-07-02

### Added

- Package-index browse sources and publish --announce
- Default to the public index and GHCR, retire grim.ocx.sh
- Rename registry `url` key to `oci` and seed index-first init defaults *(config)* **BREAKING**
- Add index/oci source-type step to the init dialog *(tui)*
- Forge-API announce with CI auto-detection *(publish)* **BREAKING**
- **Migration:** non-GitHub announces previously wrote pointers under index/github.com/ — they now land under the real index/<host>/ (delete stale entries); the gh-CLI namespace fallback is gone (set [announce] namespace or rely on CI env / GitHub token).

### Documentation

- Update ocx installer URL to setup.ocx.sh
- Setup.grimoire.rs installers, setup-grimoire action, announce and index flags
- Lead README install with setup.grimoire.rs installers
- Publishing-from-CI page + GitLab CI/CD component coverage
- Self-hosted GitLab setup guide + announce reference update

### Fixed

- Resolve GitLab user-namespace owner id via public /users lookup *(publish)*
- Expand short ids via the default chain when the browse set is index-only *(add)*
- Group index-sourced rows under their source root in the tree *(tui)*

## [0.6.2] - 2026-07-01

### Added

- Add grim config command for settings and registries *(config)*
- Accept multiple --registry values *(cli)*

### Fixed

- Group namespaced-registry rows under their configured root *(tui)*

## [0.6.1] - 2026-06-30

### Added

- Browse all configured registries (#16) *(tui)*
- Mark, warn on, and highlight deprecated packages *(deprecation)*
- Embed git provenance via opt-in --git *(publish)*
- Join single-child group chains in the browse tree (#19) *(tui)*
- Show a progress bar during install *(install)*
- Show a progress dialog during install/update/uninstall *(tui)*

### Fixed

- Detect updates via fresh tag discovery, not the cached catalog tag *(tui)*

## [0.6.0] - 2026-06-21

### Added

- Add grouped tree view with scrollable help overlay *(tui)*
- Show bundle members as virtual tree children *(tui)*
- Make [[registries]] the single source of truth for the default registry *(config)*
- Collapsible bundle nodes with per-member install *(tui)*
- Add via-bundle badge; key member actions by member name *(tui)*

### Changed

- Extract fetch_bundle_members seam from expand_bundles *(resolve)*

### Fixed

- TOML-escape the registry url written by grim init *(config)*
- Keep log output off the alternate screen and clarify the bundle-supersede note *(tui)*
- Mark a bundle installed when its members are installed directly *(tui)*
- Keep files when a declared bundle still provides the artifact *(uninstall)*
- Derive bundle row state from the declaration, not member installs *(tui)*
- Delete orphaned bundle members and refresh stale member badges *(tui)*
- Protect bundle-provided members and derive via-bundle from the snapshot *(tui)*

## [0.5.0] - 2026-06-19

### Added

- Multi-registry support, shared catalog core, and grim mcp server
- Add shell and PowerShell installers hosted on the docs site *(release)*
- Support repository_prefix / per-entry repository in publish.toml *(publish)*

### Documentation

- Document grim mcp and multi-registry config
- Lead the install page with ocx, then the install script
- Supersede artifact-type ADR with empty-config compatibility *(adr)*
- Document registry compatibility for catalog discovery
- Record that GitLab rejects the custom artifactType *(adr)*

### Fixed

- Reconcile install state against the active client set *(install)*
- Tolerate destroyed or malformed state when removing and syncing *(install)*
- Re-materialize all active clients on a partial-client version bump *(install)*
- Warn instead of failing on vendor-config sync after install/uninstall *(tui)*
- Use the OCI empty config media type so GitLab accepts manifests *(oci)*
- Drop the custom artifactType too — GitLab rejects it *(oci)*
- Apply swarm-review remediations across gitlab-registry-compat

## [0.4.3] - 2026-06-14

### Added

- Add `grim schema` to emit JSON Schemas for the TOML formats *(cli)*
- Portable anchor-relativized install state *(install)*

## [0.4.2] - 2026-06-13

### Added

- Manifest-driven batch release command *(publish)*
- Popup init dialog persisting default registry *(tui)*

### Fixed

- Checkout LFS logo so ocx describe publishes real PNG *(ci)*
- Reap empty OpenCode rules dir on last uninstall *(install)*
- Align table columns by chars, not bytes *(cli)*

## [0.4.1] - 2026-06-12

### Added

- Add first-party skills and starter bundle *(catalog)*
- Add validation and release tooling *(catalog)*
- Cross-link companion skills bidirectionally *(catalog)*
- Add install-by-identifier fallback to companion links *(catalog)*
- Selective publish flags and release-triggered publishing *(catalog)*
- Add --skip-existing for manifest-driven publishing *(release)*
- Skip published versions and start the 0.x line *(catalog)*
- Document the scripted-publishing pattern in grim-usage *(catalog)*
- Default to grim.ocx.sh when nothing is configured *(registry)*
- Snapshot the default registry into the seed config *(init)*
- Offer config init when the scope has no grimoire.toml *(tui)*

### Fixed

- Correct registry resolution precedence *(docs)*
- Render error chains once *(error)*
- Degrade to anonymous when the credential store fails *(oci)*
- Remove the lock sidecar on drop *(lock)*
- Open repository URLs on all platforms *(tui)*
- Build the same catalog window the TUI loads *(search)*
- Fall back to all clients when none are detected *(install)*
- Set DOCKER_CONFIG via GITHUB_ENV in publish-catalog *(ci)*

## [0.4.0] - 2026-06-11

### Added

- Allow non-version tags without cascade *(release)*
- Infer artifact kind from manifest annotation *(oci)*
- Replace editor option with clients array *(config)* **BREAKING**
- **Migration:** config key `editor` renamed to `clients` (array); CLI flag `--target` renamed to `--client`.
- Infer kind, optional name, honor default registry *(add)*
- Discriminate artifact kind by OCI artifactType *(oci)* **BREAKING**
- **Migration:** published manifests change shape (new digests) and the com.grimoire.kind annotation is removed. No migration is provided (provisional project).
- Support multi-file rules with a sibling support dir *(install)*
- In-file metadata with summary column and width-aware search *(catalog)*
- Render skills and rules per client with vendor env overrides *(install)*
- Shared multi-term matcher for CLI and TUI, tui --refresh *(search)*
- Detect installed clients and centralize registry precedence *(clients)*
- Flat kind-sorted list, shared search matcher, clients line *(tui)*
- Live background update checks for catalog and floating tags *(tui)*
- Add agent artifact kind with canonical frontmatter and packing *(oci)*
- Declare, hash, lock, and resolve agents *(config,lock,resolve)*
- Per-vendor agent materialization *(install)*
- Agent command surface and TUI parity *(cli,tui)*
- Cache bundle expansion and compute effective sets offline *(lock,resolve)*
- Authored repository metadata wins the source annotation *(oci)*
- Read back repository URL from the source annotation *(catalog)*
- Semantic detail pane with scrolling and o-to-open *(tui)*
- Page keys scroll the detail pane without focusing it *(tui)*
- Clamp detail scrolling at the content's end *(tui)*

### Changed

- Drop dead effective_default_registry helper *(command)*

### Documentation

- Update for clients array, new add CLI, non-version release tags
- Describe artifactType-based kind discrimination
- Document the rule support directory
- Align rig README, detection rule, env table, auth doc comments
- Document TUI declare/relock semantics
- Agent artifact reference and ADR
- Lock [[bundle]] cache and effective-declaration removal semantics
- Add artifact reference page
- Repository metadata key, source annotation, and TUI detail pane

### Fixed

- Resolve catalog under namespaced default registry *(catalog)*
- Use authorized catalog endpoint of oci dep *(oci)*
- Harden background update checks and catalog merge *(tui)*
- Release global registry tier, dedup helper, clients display *(command)*
- Longest-term prefilter and visible catalog truncation *(search)*
- Surface catalog truncation on the legend line *(tui)*
- Generation-key in-flight dedup and stamp catalog refreshes *(tui)*
- Extract shared declare/undeclare seams *(command)*
- Declare installs in grimoire.toml, flip outdated badge fast *(tui)*
- Install bundles as bundles, not skills *(tui)*
- Keep shared bundle members on bundle removal *(lock,resolve)*
- Pack [agents] members of an authored bundle *(build)*
- Mutate the lock via before/after effective sets *(remove,uninstall,tui)*
- Recompute all row states after a batch operation *(tui)*
- Hold the config flock on a sidecar, not the file itself *(lock)*

## [0.3.0] - 2026-06-04

### Added

- Add login and logout commands *(auth)*
- Add OCI bundles with conflict policy and provenance *(bundles)*
- Prune lock-orphaned artifacts, preserving local edits *(update)*

### Changed

- Collapse access modes to online/offline *(access)* **BREAKING**
- **Migration:** the `--remote` flag and `GRIM_REMOTE` environment variable are removed. Online resolution (always fresh) is now the default; no flag is needed. Use `--offline` for cache-only behaviour.

### Documentation

- Update project logo
- Document registry authentication, login and logout

## [0.2.0] - 2026-06-01

### Added

- Build musl archives and publish grim to ocx.sh *(release)*

### Documentation

- Refresh README for the v0.1.0 release
- Replace SVG logo with PNG

## [0.1.0] - 2026-06-01

### Added

- Domain core, errors, exit codes, output (phase 1)
- Config (global+project), lock, atomic store (phase 2)
- OCI access seam, cache, resolve (phase 3)
- Install, integrity, spine commands (phase 4)
- Skill standard, build, release/cascade, multi-editor transform (phase 5)
- Catalog + TUI (phase 6)
- Richer status states + color/icon polish *(tui)*
- Multi-select marks + batch install/update *(tui)*
- Uninstall seam + grim uninstall + TUI delete
- Runtime Global<->Project scope toggle *(tui)*
- Fixed-width columns, full colorization, ? help overlay *(tui)*
- Grouped tree view with version picker and UX polish *(tui)*

### Documentation

- Document multi-select/batch/scope/delete in manual rig *(tui)*
- Add mdBook documentation site
- Document registry resolution precedence

### Fixed

- Make release-update.sh executable; add rolling-release regression tests
- Contact loopback registries over plain HTTP on any port

[0.11.1]: https://github.com/grimoire-rs/grimoire/compare/v0.11.0..v0.11.1
[0.11.0]: https://github.com/grimoire-rs/grimoire/compare/v0.10.0..v0.11.0
[0.10.0]: https://github.com/grimoire-rs/grimoire/compare/v0.9.1..v0.10.0
[0.9.1]: https://github.com/grimoire-rs/grimoire/compare/v0.9.0..v0.9.1
[0.9.0]: https://github.com/grimoire-rs/grimoire/compare/v0.8.4..v0.9.0
[0.8.4]: https://github.com/grimoire-rs/grimoire/compare/v0.8.3..v0.8.4
[0.8.3]: https://github.com/grimoire-rs/grimoire/compare/v0.8.2..v0.8.3
[0.8.2]: https://github.com/grimoire-rs/grimoire/compare/v0.8.1..v0.8.2
[0.8.1]: https://github.com/grimoire-rs/grimoire/compare/v0.8.0..v0.8.1
[0.8.0]: https://github.com/grimoire-rs/grimoire/compare/v0.7.0..v0.8.0
[0.7.0]: https://github.com/grimoire-rs/grimoire/compare/v0.6.2..v0.7.0
[0.6.2]: https://github.com/grimoire-rs/grimoire/compare/v0.6.1..v0.6.2
[0.6.1]: https://github.com/grimoire-rs/grimoire/compare/v0.6.0..v0.6.1
[0.6.0]: https://github.com/grimoire-rs/grimoire/compare/v0.5.0..v0.6.0
[0.5.0]: https://github.com/grimoire-rs/grimoire/compare/v0.4.3..v0.5.0
[0.4.3]: https://github.com/grimoire-rs/grimoire/compare/v0.4.2..v0.4.3
[0.4.2]: https://github.com/grimoire-rs/grimoire/compare/v0.4.1..v0.4.2
[0.4.1]: https://github.com/grimoire-rs/grimoire/compare/v0.4.0..v0.4.1
[0.4.0]: https://github.com/grimoire-rs/grimoire/compare/v0.3.0..v0.4.0
[0.3.0]: https://github.com/grimoire-rs/grimoire/compare/v0.2.0..v0.3.0
[0.2.0]: https://github.com/grimoire-rs/grimoire/compare/v0.1.0..v0.2.0
[0.1.0]: https://github.com/grimoire-rs/grimoire/tree/v0.1.0

