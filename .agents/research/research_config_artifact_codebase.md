# Research: Config Artifact (harness settings) — Codebase Recon

## Metadata

**Date:** 2026-11-03
**Domain:** packaging
**Triggered by:** `.agents/discussions/config-artifact-type.md`
**Expires:** 2027-05-03

Scope: what existing code and design records establish about grim distributing harness configuration (settings.json / config.toml keys: permissions, model defaults, env, behaviour options) as a new artifact kind. Read-only recon; no recommendation. All paths relative to the repo root.

## Direct Answer

No artifact kind today distributes arbitrary harness settings. Every existing write into a harness-owned config file is either (a) a grim-computed key/element (`claudeMdExcludes`, OpenCode `instructions` glob, hook marker element), or (b) a publisher-supplied value at a grim-fixed location (an MCP server entry at `<container>/<name>`). The shared splice engine addresses exactly two nesting levels (`/container/member`), one array element, or one marked nested handler; it has no general deep-key merge. Adding a kind is a known-shaped, large, one-way change (six kinds exist; the last two each touched ~40 files). The trust ADR says all kinds are equally trusted and prohibits gating an already-shipped kind, and prior records treat "a config key grim writes" as accepted Tier-1 "class 2" work but contain no record that considered a publisher-authored settings payload.

## 1. Artifact kinds and the touch list for a new one

Kinds: `Skill, Rule, Agent, Bundle, Mcp, Hook` (`src/oci/artifact_kind.rs:27-60`; `ALL` at `:79`). Wire discriminator is the manifest annotation `com.grimoire.kind` (`src/oci/artifact_kind.rs:16`); `artifactType`/config media types are read-only legacy tiers (`:160-185`, `src/oci/annotations.rs:380-395`).

What a kind touches, evidenced by `ArtifactKind::Mcp` reference counts per file (grep, 102 files reference `ArtifactKind::` in total):
- Kind enum, `kind_str`, `subdir`, media types, `is_dir_artifact`: `src/oci/artifact_kind.rs:79-205`. A total `match` in `all_index` plus a runtime injectivity test forces completeness (`:110-125`, test at `:256`).
- Placement/anchors: `src/install/path_anchor.rs` (26 refs), `src/install/uninstall.rs` (21), `src/install/installer.rs` (13), `src/install/expected_outputs.rs`, `src/install/prune.rs`.
- Lock: `src/lock/grimoire_lock.rs:166-167` (per-kind arrays; `[[mcp]]`/`[[hook]]`), `src/lock/lock_io.rs`, `src/lock/effective_set.rs`; schema fixture `src/lock/testdata/lock.schema.json`.
- Config: `src/config/project_config.rs:775-807` (bundle member tables `[skills]/[rules]/[agents]/[hooks]/[mcp]/[bundles]`), declaration hash.
- Publish/pack: `src/command/build.rs:278,582,640,668`, `src/command/release.rs:251,580`, `src/command/publish.rs:1575,1827,1956,2101`, `src/skill/local_pack.rs:39,95` (mcp path sources rejected), `src/fetch.rs:387,583` (per-kind layer size limit), `src/oci/layer.rs:109`.
- CLI kind parsing: `src/command/add.rs:1419-1447` (infer from manifest, `NoKind` fallback to `--kind`), `remove.rs:52`, `uninstall.rs:78`, `why.rs:231`.
- Export: `src/export/stage.rs` (14 refs), `src/export/family.rs:528` (admission list; Hook is admitted by no family, `.agents/adr/adr_harness_plugin_export.md:635`), `src/export/resolve.rs:189`.
- JSON reports `src/api/*_report.rs`, TUI `src/tui/app.rs`, status badge, docs (`artifacts.md`, `json-interface.md`, `commands.md`, `stability.md`, `clients.md`, `mcp-servers.md`), catalog skills drift review (AGENTS.md "First-Party Catalog").
- Schemas generated via `taskfiles/schema.taskfile.yml:7` into `docs/public/schemas/` (frozen URLs listed in `docs/src/content/docs/stability.md` "Published schema URLs").
- Template for adding a kind additively: `.agents/adr/adr_agent_artifact_kind.md` (referenced as such at `.agents/adr/adr_hooks_support.md:1803`); wire format for a new kind: `adr_oci_empty_config_compat.md`.
- `.agents/adr/adr_managed_context_block.md:151` records a prior decision *not* to add a kind because it "would cut across every exhaustive `match kind` site (client_target, path_anchor, materializer, render, installer, bundle resolver)"; that ADR is Status: Proposed and `managed_brief` has no code hits.

Renderer organisation: one `src/install/vendor_<client>.rs` per client implementing the `Vendor` trait (`src/install/vendor.rs:666`). Capability hooks: `kind_support(kind) -> KindSupport {Native, Degraded, Declined}` (`vendor.rs:84-93`, `:682`), scope-aware `kind_surface` (`:709`), `mcp_config_path(s)` (`:1138-1150`), `mcp_entry` (`:1179`), `sync_config` (`:1286`), hook surface methods (`:770-1057`), per-kind field registries `skill_fields/rule_fields/agent_fields` (`:1064-1092`). A decline is: `kind_support = Declined` (installer warns, skips, records zero outputs; e.g. Zed declines Rule and Agent, `src/install/vendor_zed.rs:48-54`), or `None` from `mcp_entry`/`mcp_config_path` (warn and skip, `installer.rs:2628` onward), or `HookDecline` enum (`vendor.rs:256`). Doc parity: `clients.md` matrix under a table-parity test (`.agents/adr/adr_vendor_support_tiers.md:20-22`).

## 2. Writes into harness-owned files (inventory)

| Write | File | Keys, who chose | Merge | Reversal / ownership | Drift |
|---|---|---|---|---|---|
| MCP entry | `.mcp.json`, `~/.claude.json`, `opencode.json(c)`, `.vscode/mcp.json`, `.github/mcp.json`, Codex `config.toml`, Gemini/Zed/Amp/Qoder `settings.json`, others (`docs/src/content/docs/mcp-servers.md:201-228`) | Container key fixed per vendor; member name = binding name; value = publisher descriptor rendered by `Vendor::mcp_entry` (`vendor.rs:1179`) | `json_splice::upsert_member` / `toml_splice::upsert_member` (`src/install/json_splice.rs:57`, `toml_splice.rs:63`), span-preserving; refuses non-object | `ClientOutput.entry` pointer + semantic content hash (`install_state.rs:84-111`); `adopted` flag; uninstall removes entry (`uninstall.rs:227-245`) | `integrity_gate` (`installer.rs:1557`); untracked/different member refused unless `--force` (`installer.rs:2784`); drifted entry refuses uninstall unless `--force` (`uninstall.rs:106`); vendor-owned keys excluded from hash (`vendor.rs:1159`) |
| `claudeMdExcludes` element | Claude `settings.json` (project and global) | Key and element value both computed by grim (`claude_config.rs:96,116`) | `managed_config::sync_managed_element` array add/remove (`managed_config.rs:84`) | Derived from install state + retired outputs; never from filesystem probing (`claude_config.rs:23-45`) | Add strict (unparseable config fails), remove tolerant (`managed_config.rs:14-26`); user-typed identical element is adopted and later removed, accepted (`claude_config.rs:48-52`) |
| `instructions` glob | `opencode.json(c)` | grim-computed | same array driver (`opencode_config.rs:146`) | pure function of state | same |
| Hook registration | Claude `settings.json` (global) / `.claude/settings.local.json` (project), splice-config clients; own-file for Codex/Copilot | Event keys by grim projection; handler element carries `com.grimoire.managed: hook-dispatcher` (`vendor.rs:180,201`) | `upsert_nested_handler` (`json_splice.rs:439`) | NOT recorded in state; derived, enumerate-and-reap `owned − desired` via `owned_nested_handlers` (`json_splice.rs:731`, `hook_registrar.rs:1-60`) | Marker constants frozen forever (`vendor.rs:150-205`) |
| Cline lock | `cline_mcp_settings.json` | n/a | grim joins Cline's directory lock so splice does not race Cline's own rewrite (`src/install/cline_lock.rs:1-30`) | n/a | only vendor-specific concurrency handling found |

Shared abstractions: `json_splice` (`split_pointer` at `json_splice.rs:28` admits only `/container/member`), `toml_splice`, `managed_config`, `json_config` parse helpers. They are format-level seams; the *ownership model* differs per write (recorded entry, derived-from-state, marker-element, own-file). There is no generic "arbitrary key at arbitrary path" primitive. No code writes `permissions`, `env`, `model` or similar keys; `claude_config.rs:59-62` notes `settings.json` also holds the user's `permissions` and `hooks` and that they survive untouched.
Not splice-capable: YAML (goose) and JSON5 (openclaw) are declined for MCP because the splicer handles JSON and TOML only (`mcp-servers.md:233-241`).
Open issues on these paths: #55 (dynamic settings.json/.jsonc selection can orphan a recorded MCP entry), #56 (MCP splice key not JSON-escaped, CWE-116/74), #53 (re-materialize managed context blocks).

## 3. Generic-to-vendor rendering today

- MCP: one canonical descriptor (`src/oci/mcp.rs`, `${VAR}` syntax) is rendered per vendor by `mcp_entry`; env refs translated by `src/install/mcp_config.rs:1-30`. Vendors differ in container key, entry shape, env-ref syntax, transports, and OAuth projection (emit matrix `mcp-servers.md:201-228`). Drops/declines: `ws` declined by OpenCode/Copilot/Codex/Gemini/Antigravity/Warp; OAuth "lossless-or-skip" (`adr_mcp_oauth_projection.md`; watchlist rows in `.claude/rules/vendor-capability-watchlist.md`); Gemini drops `timeout` with warning (`vendor_gemini.rs:206`). Surfaced as tracing warnings per client; no-registrable-client at all is an error (`installer.rs:2618-2627`).
- Skills/rules/agents: canonical file plus `<vendor>.<field>` keys in `metadata` lifted into native frontmatter (`adr_tool_namespaced_metadata_rendering.md`; agent projection and emit matrix `adr_agent_artifact_kind.md` decision 2; `append_lifted` in `src/install/render.rs`). This IS an existing per-vendor payload concept inside one artifact: vendor overrides ride in the single canonical document, and non-matching vendors ignore them. Object-valued fields (Claude `mcpServers`/`hooks`, OpenCode `permission`) are explicitly not projectable (`adr_agent_artifact_kind.md` decision 6).
- Hooks: `[[hooks]]` canonical entry plus `<vendor>.<field>` override tables and `<vendor>.event` native passthrough (`src/oci/hook.rs:31-33,377-385`).
- No existing artifact ships a separate per-vendor file set selected by client; one document is projected.

## 4. Trust boundary

- `adr_artifact_trust_model.md` (Accepted 2026-08-26): trust decision is the declaration gesture (`grim add`/edit/bundle); all kinds equally trusted (decision 2); integrity not consent (decision 4); explicitly out of scope: "Anything the harness owns: which tools an agent may run, sandboxing, approval prompts at execution time. grim installs config; it is not the execution policy layer." Amended once: hooks gated by workspace consent only because hooks were unreleased; "this is not an invitation to gate the next kind. A gate on a shipped kind stays prohibited."
- `adr_hook_workspace_consent.md`: consent record at `$GRIM_HOME/hooks/consent/<workspace-key>.json` keyed on absolute workspace path, kind-agnostic format; answers "may this checkout arm hooks"; env-unreachable by design (AGENTS.md env table "(no variable)" row).
- `arch-threat-model.md`: T1 malicious publisher, T3 untrusted cloned repo, T4 prompt injection driving config rewrites (PromptArmor example: a hook rewrote the permission store, `adr_hooks_support.md:649`); I1 nothing armable inside a repository; I4 default-deny for anything that executes; N1 insiders with commit access are a non-goal. Implication visible in the records: a project-scope artifact that wrote execution-relevant settings (permissions, env, endpoints) into a committed repo file would sit in I1/T3 territory; hook registration at project scope avoids this by using `.claude/settings.local.json` (`vendor_claude.rs:470-486`) and `$GRIM_HOME` payloads.
- What existing artifacts already reach: MCP descriptor names a `command`/`args` the client spawns or a URL (`src/oci/mcp.rs:132-133,354`) — recorded as installed with no gate and equal in reach to a skill shipping scripts (`adr_artifact_trust_model.md` Context). Credentials: `${VAR}` only, no literal secrets, no `client_secret` field. Permission-adjacent fields already pass through: Claude skill `allowed-tools`/`disallowed-tools` (`vendor_claude.rs:85-91`), Claude agent `permission-mode` (`vendor_claude.rs:137`), Junie `permission-mode` (`vendor_junie.rs:72`). Hooks (experimental) can allow/deny tool calls via grim's dispatcher.
- Class boundary: `adr_vendor_support_tiers.md` (Proposed): "grim renders artifacts. grim does not run at the client's runtime." Class 2 "compensating render" = a documented config key, deterministic, removed on uninstall, Tier 1 clients only (`claude, codex, opencode, copilot`, `:100,121`).

## 5. Prior decisions

- No ADR, plan, discussion or research file found that considered, deferred or rejected a publisher-authored settings/permissions/profile artifact (searched settings, permissions, config artifact/kind, preset, profile, `claudeMdExcludes`, marketplace across `.agents/**`; hits were only incidental). `.agents/discussions/config-artifact-type.md` is a 37-byte stub (`State: active · Updated: 2026-11-03`), untracked.
- `docs/src/content/docs/clients.md:665-679` (`{#gap-claude-kinds}`): grim declines Claude commands, output styles, LSP, themes, monitors, workflows; reasoning "an artifact kind is a publish, lock and render contract"; revisit "when a second client supports something equivalent, or when a user asks". Mirrored in `vendor-capability-watchlist.md` row "Component kinds beyond skills/rules/agents/MCP".
- Plugin/marketplace export (`adr_harness_plugin_export.md`, `adr_harness_marketplace_export.md`, `.agents/discussions/harness-native-marketplaces.md`): these render skills/rules/agents/MCP into harness plugin packages; grep found no settings/permissions payload there. Plugin env placeholders are an MCP amendment only (`adr_harness_plugin_export.md:581-600`). `research_plugin_schemes.md:102` records "OpenCode plugins are code, not config artifacts".
- `adr_vendor_support_tiers.md:32`: `claudeMdExcludes` registration was the motivating case for class 2 (issue #102, closed).
- `adr_managed_context_block.md:104,151,175`: "Publishable 'context artifact' kind: explicitly deferred, revisit post-1.0".
- `adr_hooks_support.md:891`: inline `settings.json` `hooks` key would require ownership tracking beyond the marker approach (context for why splice ownership is hard).

## 6. Compatibility under the freeze

- Principle 9 (AGENTS.md) and `stability.md` "Frozen at 1.0": OCI wire format artifact kinds, lock/config schema, state schema, JSON report shapes, published schema URLs are frozen; additive only. Vendor render layout is outside the semver contract (`adr_render_layout_stability.md` decision 1, `:112`), but renderer changes must prove self-heal (AGENTS.md Principle 9).
- A new kind is a recorded Principle-9 deviation each time: lock and state become unreadable by older grim via `deny_unknown_fields` (exit 78) once the kind is used (`plan_hooks_artifact_kind.md:2595`; `stability.md:441-476`); precedent: `agent`, `[options.vendors]`, `hook`. Mitigation pattern: emit the new key only when non-empty so config-free projects stay byte-identical (`adr_agent_artifact_kind.md` decision 4).
- Shipped grim meeting an unknown `com.grimoire.kind` value: `kind_from_manifest` returns `None` (`src/oci/annotations.rs:393`), so `grim add` fails with `NoKind` and asks for `--kind` (`add.rs:1419-1447`); the annotation `com.grimoire.requires-features` is ignored by grim 0.15.1 (`stability.md:497`). `adr_artifact_dependencies.md:234` rejects a new kind value as the forward-compat signal ("changes what `com.grimoire.kind` means for every index and catalog consumer").
- Features gate: `KNOWN_FEATURES = ["dependencies"]` (`src/oci/features.rs:18`); `check_required_features` runs on fetch, resolve and install (`src/fetch.rs:355`, `src/resolve/graph.rs:390`, `src/install/installer.rs:2402`) and refuses with exit 65 `unsupported-artifact-feature` telling the user to update grim. Membership is a "two-way door; the key is not" (`features.rs:17`). Only grim builds that include this feature check will refuse; 0.15.1 ignores the annotation. Layout rule F4 (`adr_artifact_dependencies.md:297-310`) refuses unknown layer shapes.
- Dependencies: every installable kind may declare `requires` (decision 6, `adr_artifact_dependencies.md:955-960`); for a new kind a source-side declaration form would have to be defined (frontmatter `grim.requires.*` for md kinds, `[requires.*]` tables in toml kinds).
- Bundle membership: member kinds are skill, rule, agent, mcp, hook (`src/oci/bundle.rs:39-45` comment; bundle source tables `project_config.rs:775-807`). Config as a member would inherit decision 1 transitivity (the `grim add` gesture covers everything a bundle pulls in, `adr_artifact_trust_model.md` decision 1), meaning a bundle could carry settings to every consumer; and an older grim with a lock containing the new kind exits 78.

## 7. GitHub context (gh worked)

Open issues possibly related: #55 (settings.json/.jsonc selection can orphan MCP entry), #56 (MCP splice key escaping, CWE-116/74), #53 (managed context blocks re-materialize), #266 "support envvars for mcp" (empty body), #264 (artifact dependencies closing-review follow-ups), #175 (move hook config validation off install layer), #171 (fail-closed runtime gate in `grim hook run`). Closed: #102 (claudeMdExcludes), #141 (Claude upstream refresh, source of the declined-kinds gap). No open issue or PR asks for a settings/config artifact. Open PRs are only dependency bumps (#258, #259, #260).

## Sources

`src/oci/artifact_kind.rs`, `src/install/vendor.rs`, `src/install/{json_splice,toml_splice,managed_config,claude_config,opencode_config,hook_registrar,installer,uninstall,install_state,cline_lock,mcp_config}.rs`, `src/oci/{features,mcp,hook,annotations}.rs`, `src/command/add.rs`, `.agents/adr/{adr_artifact_trust_model,adr_hook_workspace_consent,adr_artifact_dependencies,adr_agent_artifact_kind,adr_vendor_support_tiers,adr_managed_context_block,adr_render_layout_stability,adr_hooks_support,adr_harness_plugin_export}.md`, `.claude/rules/{arch-threat-model,vendor-capability-watchlist}.md`, `docs/src/content/docs/{stability,mcp-servers,clients}.md`, `.agents/plans/plan_hooks_artifact_kind.md`.

## Negative / contradicting evidence

- The discussion file is an empty stub; no prior record addresses this question directly, so "not previously considered" is an absence finding from grep, not a recorded rejection.
- `adr_artifact_trust_model.md` puts "which tools an agent may run, sandboxing, approval prompts" out of scope for grim, which can be read either as "grim should not manage permissions" or as "grim is not responsible for their effect"; the ADR does not say which.
- `arch-principles.md`, `subsystem-file-structure.md` were not read in full (914-line file); only grepped for relevant terms.
- Principle 9 constrains breaking changes but the repeated precedent (agent, hook kinds) shows a new kind is accepted as a recorded deviation, so "frozen" does not by itself forbid a kind.
- `ArtifactKind::Hook` is experimental and `adr_vendor_support_tiers.md` is still Proposed; both are weaker precedents than the MCP kind.
- Zero code hits for a deep-key or arbitrary-pointer splice; `split_pointer` rejects anything but two levels (`json_splice.rs:28-33`).

## Leads

- Upstream capability: per-harness settings schemas and layering (user/project/local/managed) and which keys each harness merges vs. overrides; lane: harness settings semantics.
- Security review: T3/T4 analysis of publisher-authored permissions/env/endpoints at project vs global scope; lane: threat-model.
- Splice engine: cost of a general nested-key merge with ownership tracking for JSON, JSONC and TOML (and YAML/JSON5 gaps); lane: file-structure engineering.
- Catalog/index and VS Code extension consumers of `com.grimoire.kind` for unknown values; lane: index consumers.
