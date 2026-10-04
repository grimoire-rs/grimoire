# Codex CLI hook-registration surface — D7a axis (plugin vs. hooks.json vs. inline `[hooks]`)

## Metadata

- **Date**: 2026-08-14
- **Domain**: Codex CLI (`openai/codex`) native hook system — third-party installer surface choice for grim's Codex hook dispatcher registration
- **Triggered by**: hex-architect tier-high research fan-out, axis `codex-hook-surface`, blocking design decision D7a. Builds directly on `.agents/research/hooks_vendor_reports/codex.md` (same research date) — that file's §2/§3/§9/§10/§11 are treated as established and **not** re-derived here except where this pass found more precise or corrected detail.
- **Expires**: Codex hooks are explicitly a fast-moving target (10+ merged hook PRs and a dozen+ open hook issues in the two weeks before this research date alone). Re-verify against `codex-rs/hooks/schema/generated/*.schema.json` and the specific source files cited below before shipping a hard dependency. Treat anything here as stale after **2026-09-14** (one month) or after the next Codex stable release past `rust-v0.147.0`, whichever comes first.

Repo state for this pass: shallow clone (`--depth 1`) of `openai/codex` `main`, HEAD `8630bb3c` (2026-08-14T09:27:41Z) — one commit later than the prior report's HEAD, same day.

## Direct Answer

**RECOMMENDATION: Surface 2 — splice `[hooks]` into `config.toml`, using the array-of-matcher-groups shape grim already knows how to splice for MCP servers, at global (`~/.codex/config.toml`) scope by default.**

Confidence: **medium-high**. The plugin route (Surface 3) is real and viable as *code* — a plain directory with `.codex-plugin/plugin.json` works, cross-compatible with Claude Code's and Cursor's manifest paths — but it buys grim **zero trust-friction relief** (plugin hooks go through the identical, non-managed trust gate as file/TOML hooks) and it **adds** a second, heavier config surface grim must also own (`[marketplaces.<name>]` + `[plugins."<name>@<marketplace>"]` in the same `config.toml`, plus, if grim uses the vendor's own install flow, a Codex-managed cache copy under `marketplace_install_root(codex_home)`). Since grim ends up editing `config.toml` either way, the plugin route's only genuine win — dictionary-keyed identity instead of positional identity — is available more cheaply by going straight to Surface 2, because `[marketplaces."grim"]` / `[plugins."grim-hooks@grim"]` are themselves dictionary keys of the same shape grim already splices for `mcp_servers`. The one place a bare `[[hooks.PreToolUse]]` array-of-tables is genuinely positional is *within* the event's own array — see the fix below.

**The fix for D7a's core risk (index-shift trust invalidation) is not "own the file" vs. "use TOML" — it is emit exactly one matcher group per event, always as the *only* member of that event's array, guarded by a comment banner asking humans not to add sibling groups.** Source-verified: the trust hash is computed over `(event_name, matcher, single normalized handler)` — reformatting or moving the same content between `hooks.json` and inline TOML does **not** change the hash. But the **persisted trust record is keyed positionally** (`source_path:event_snake:group_index:handler_index`), and that key is looked up, not searched-by-hash. If a human (or another tool) inserts a new matcher group *above* grim's in the same event array, grim's group's `group_index` shifts, the old trust record silently stops matching, and grim's hook reverts to `Untrusted` (silently skipped, not blocked) even though its content hash is unchanged. Keeping grim to one group per event, and documenting "don't hand-edit above this block," minimizes — cannot fully eliminate — this exposure under either Surface 1, 2, or 3, since the position-vs-hash split is a property of the trust *state* mechanism (`codex-rs/hooks/src/engine/config_rules.rs`), not of the file format.

**Fallback if Surface 2 turns out wrong**: Surface 1 (own `hooks.json`) — same trust exposure, marginally better isolation (a whole-file diff makes tampering obvious), but keeps the collision risk stated in the brief (fixed filename, exit-65 refusal on an existing user file) with no directory-glob (`hooks.d/`) escape hatch — confirmed **not to exist** (source-checked). Reach for Surface 3 (plugin) only if a future design wants grim's hook to travel *inside* a grim-authored plugin bundle alongside skills/MCP servers under one namespaced identity — not for trust-friction reasons.

---

## 1. Plugin viability

**A Codex "plugin" is a directory carrying a manifest at one of three discoverable relative paths** (`codex-rs/exec-server-protocol/src/protocol.rs:46`):

```rust
pub const DISCOVERABLE_PLUGIN_MANIFEST_PATHS: &[&str] = &[
    ".codex-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
];
```

i.e. Codex accepts a **Claude Code plugin manifest path verbatim** (`.claude-plugin/plugin.json`) as one of its own discoverable forms — deliberate cross-vendor compatibility, not an accident (see `PLUGIN_ROOT`/`CLAUDE_PLUGIN_ROOT` env-var aliasing below, and §11 of the prior report on `ClaudeHooksEngine`).

**Manifest fields — all optional, `name` derived if empty.** `codex-rs/core-plugins/src/manifest.rs:45-68`, `RawPluginManifest` — every field is `#[serde(default)]`; there is no `deny_unknown_fields` and no required field at the JSON level. If `name` is blank, it falls back to `plugin_root.basename()` (`manifest.rs:306-309`). A **minimal valid manifest is `{}`**. Fields relevant to hooks:

```rust
struct RawPluginManifest {
    name: String,                              // defaults to "", then to dir basename
    version: Option<String>,
    description: Option<String>,
    keywords: Vec<String>,
    skills: Option<RawPluginManifestPaths>,
    mcp_servers: Option<RawPluginManifestMcpServers>,
    apps: Option<String>,
    hooks: Option<RawPluginManifestHooks>,       // Path | Paths | Inline(HooksFile) | InlineList
    interface: Option<RawPluginManifestInterface>,
}
```

`hooks` accepts a path string, an array of path strings, an inline `HooksFile` object, or an array of inline `HooksFile` objects (`codex-rs/core-plugins/src/manifest.rs:145-153`) — the *same* `HookEventsToml` schema documented in the prior report's §3, so nothing new to learn about the hook shape itself once you're inside a plugin.

**Install/discovery path is two-tier, and a bare directory drop is NOT enough on its own — it must also be *registered*:**

1. **Marketplace registration** — `[marketplaces.<name>]` in `config.toml`, written by `codex-rs/config/src/marketplace_edit.rs::upsert_marketplace` (verbatim, tested): `source_type`, `source`, `last_updated`, optional `last_revision`/`ref`/`sparse_paths`. `source_type = "local"` + `source = "<path>"` is a fully local, offline, synchronous flow (`codex-rs/core-plugins/src/marketplace_add.rs`, `source.rs` — `MarketplaceSource::Local`, no network call).
2. **Plugin enablement** — `[plugins."<name>@<marketplace>"]` in `config.toml`, written by `codex-rs/config/src/plugin_edit.rs::set_user_plugin_enabled` (verbatim, tested): only `enabled` (bool) and `mcp_servers` (per-server policy overlay) are real schema fields on `PluginConfig` (`codex-rs/config/src/types.rs:844-849` — no `source` field on the type itself, `deny_unknown_fields` is set here, unlike the plugin manifest).
3. Only *then* does `configured_plugins_from_stack` (`codex-rs/core-plugins/src/marketplace_policy.rs`, consumed by `codex-rs/core-plugins/src/loader.rs`) surface the plugin's hooks into `PluginHookSource` for the hook engine to discover (`codex-rs/core-plugins/src/loader.rs:1174-1266` builds `PluginHookSource` only for configured/enabled plugins).

**So: a plain directory with a manifest is discoverable by the generic `DISCOVERABLE_PLUGIN_MANIFEST_PATHS` scan used by *other* subsystems (executor/connector plugin resolution, `codex-rs/core-plugins/src/provider.rs:174`, skills loader `codex-rs/ext/skills/src/loader/discovery.rs:135`) — but the **hooks-specific pipeline requires the two `config.toml` registrations above**, not just a file on disk.** This means: **third-party install by writing files only is possible, but the files include `config.toml` edits, not just a plugin directory** — the "own config.toml with no config.toml edits" framing in the brief's option 3 does not hold; it trades one TOML edit (a `[[hooks.EVENT]]` array-of-tables) for two TOML *table* edits (`[marketplaces.grim]`, `[plugins."grim-hooks@grim"]`) plus a directory.

`codex plugin add`/`codex plugin marketplace add` (`codex-rs/cli/src/plugin_cmd.rs`) are the vendor's own non-interactive CLI verbs for this — they write exactly the two config tables above and (per `install_plugin`, `codex-rs/core-plugins/src/manager.rs:1412-1466`) additionally **copy** the plugin into `marketplace_install_root(codex_home)`, i.e. `codex plugin add` does not run the plugin in place from its declared `source` path; it stages a private, Codex-owned cache copy (`PluginInstallOutcome.installed_path`). Grim could either shell out to `codex plugin add` non-interactively (adds a hard `codex` binary dependency to grim's install path) or hand-write the same `config.toml` tables directly and skip the CLI (cheaper, matches grim's existing model, but means grim must track how `find_installable_marketplace_plugin` and `resolve_configured_marketplace_root` resolve a `source_type = "local"` marketplace — not independently proven here to *skip* the install-root copy step for local sources; treat "does a local-source plugin run in place vs. require a `codex plugin add` copy" as **NOT FULLY CONFIRMED**, medium confidence it still needs the copy since `configured_plugins_from_stack` gates on the *plugin* being installed/enabled, not merely the marketplace being registered).

**Trust clearing for plugin hooks is NOT separate from, and NOT lighter than, ordinary hook trust** — see §2. `append_plugin_hook_sources` (`codex-rs/hooks/src/engine/discovery.rs:238-283`) passes `is_managed: false` for every plugin-sourced hook, unconditionally. There is no plugin-level "installed = trusted" shortcut.

**Confirmed env vars for plugin-sourced hooks** (source-verified, upgraded from the prior report's "medium confidence"): `PLUGIN_ROOT`, `PLUGIN_DATA`, and — new in this pass — **Claude-compatible aliases `CLAUDE_PLUGIN_ROOT` and `CLAUDE_PLUGIN_DATA`** are set on every plugin hook's process env, explicitly "for OOTB compat with existing plugins that use this env var" (`discovery.rs:262-267`). This is more cross-vendor-compatibility evidence for the plugin route specifically, but it is a portability nicety, not a trust or identity win.

## 2. Trust identity mechanics — exact

Ground truth: `codex-rs/hooks/src/engine/discovery.rs:619-654` and `:711-753`.

**What is hashed** — `hook_hash()` (`discovery.rs:711-728`), verbatim:

```rust
/// Hash a normalized, config-derived identity instead of source text so equivalent
/// hooks from config TOML and hooks.json converge on the same trust identity.
#[derive(Serialize)]
struct NormalizedHookIdentity {
    event_name: &'static str,
    #[serde(flatten)]
    group: MatcherGroup,
}

fn hook_hash(
    event_name: HookEventName,
    matcher: Option<&str>,
    group: &MatcherGroup,
    normalized_handler: HookHandlerConfig,
) -> String {
    let mut group = group.clone();
    group.matcher = matcher.map(ToOwned::to_owned);
    group.hooks = vec![normalized_handler];   // <-- synthetic single-handler group
    let identity = NormalizedHookIdentity { event_name: ..., group };
    let Ok(value) = TomlValue::try_from(identity) else { unreachable!(...) };
    version_for_toml(&value)
}
```

So the hash covers exactly: **the event name, the resolved matcher string, and the one normalized handler config** (command, timeout, async flag, status message, additionalContextLimit — post-normalization, e.g. Windows-vs-POSIX command already resolved, default timeout already filled in). It does **not** include: the source file path, the other hooks in the same matcher group, any sibling groups, the whole file, or the positional indices. Two byte-for-byte-different files (`hooks.json` vs. inline `[hooks]` TOML) encoding the same logical hook produce the **same hash** — this is the literal, doc-commented purpose of normalizing before hashing.

**What is NOT hashed but IS load-bearing: the storage key.** `hook_key()` (`codex-rs/hooks/src/lib.rs:105-115`):

```rust
pub fn hook_key(key_source: &str, event_name: HookEventName, group_index: usize, handler_index: usize) -> String {
    format!("{key_source}:{}:{group_index}:{handler_index}", hook_event_key_label(event_name))
}
```

`key_source` is the **source file's own path string** (`source_path.display().to_string()` — `discovery.rs:132,172,227`) for config/hooks.json layers, or `crate::declarations::plugin_hook_key_source(plugin_id, source_relative_path)` for plugins. This key is used purely to **look up** a prior trust record — `hook_trust_status()` (`discovery.rs:730-742`):

```rust
fn hook_trust_status(is_managed: bool, current_hash: &str, trusted_hash: Option<&str>) -> HookTrustStatus {
    if is_managed { HookTrustStatus::Managed }
    else { match trusted_hash {
        Some(trusted_hash) if trusted_hash == current_hash => HookTrustStatus::Trusted,
        Some(_) => HookTrustStatus::Modified,
        None => HookTrustStatus::Untrusted,
    }}
}
```

`trusted_hash` comes from `source.hook_states.get(&key)` — an **exact string-key lookup**, no fallback search by hash across other keys.

**Direct answer to the brief's concrete question: yes.** If a user inserts a new matcher group *above* grim's group in the same event's array (in the same file), grim's group's `group_index` increases by however many groups were inserted before it. The next discovery pass computes `key = "<file>:<event>:<NEW_index>:0"`, which has **no entry** in `hook_states` (the old entry sits under the old, now-orphaned key). `trusted_hash` is therefore `None` → `HookTrustStatus::Untrusted` — even though `current_hash` (the content hash) is byte-identical to before. The hook is then **silently excluded from execution** (see the gating logic below), not blocked with an error, and not auto-repaired: there is no hash-based fallback that would let Codex notice "this exact hook was already trusted under a different index" and re-associate the state.

Inserting a hook *inside the same group*, before grim's handler (shifting `handler_index`), has the identical effect for that one handler.

**Does a `managed` source skip trust entirely? Yes, unconditionally.** `is_managed` is computed once per config layer (`hook_metadata_for_config_layer_source`, `discovery.rs:755-770`):

| Layer | `is_managed` |
|---|---|
| `PackagedDefaults` | false (also `HookSource::Unknown`) |
| `System` | **true** |
| `User` | false |
| `Project` | false |
| `Mdm` | **true** |
| `EnterpriseManaged` | **true** |
| `SessionFlags` | false |
| `LegacyManagedConfigTomlFromFile`/`FromMdm` | **true** |
| Plugin hooks (`append_plugin_hook_sources`) | **false**, always, regardless of which layer registered the plugin |

`hook_trust_status` short-circuits to `Managed` before even comparing hashes when `is_managed` is true — those hooks never enter the untrusted/modified branch at all. Grim, as a normal per-user install, has **no path to `is_managed = true`** — that tier is reserved for `requirements.toml`/MDM/enterprise-managed layers, an admin-only channel (per the prior report's §2/§9).

**A real, load-bearing, sourced consequence not previously in the record — where trust state itself is read from:** `codex-rs/hooks/src/config_rules.rs::hook_states_from_stack`, verbatim comment: *"This intentionally reads only user and session flag layers... Project, managed, and plugin layers can discover hooks, but they do not get to write user hook state."* Concretely:

```rust
for layer in config_layer_stack.all_layers_low_to_high() {
    if !matches!(layer.name, ConfigLayerSource::User { .. } | ConfigLayerSource::SessionFlags) {
        continue;
    }
    let Some(state_value) = layer.config.get("hooks").and_then(|hooks| hooks.get("state")) else { continue };
    ...
}
```

**Trust records for a hook of *any* scope or source — project, plugin, user — live only under `[hooks.state."<key>"]` in the single per-user `~/.codex/config.toml`.** A project's own `.codex/config.toml` or `.codex/hooks.json`, and a plugin's manifest/hooks file, can never carry their own trust bookkeeping — Codex will not look there. This is why the interactive `/hooks` review flow works uniformly regardless of scope: it always writes the approval into the user's own config. It is also the **exact mechanism openai/codex#21615** (still open) documents third-party integrators already exploiting: *"the trust hash is reproducible from the open-source implementation, so the practical alternative for integrators today is to write Codex's internal trust state directly with `[hooks.state."<key>"] trusted_hash = "..."` entries in `~/.codex/config.toml`. A supported API would be better than having each integrator ship its own copy of that private logic."* (openai/codex#21615, opened by a "local IDE/wrapper" integrator, unresolved as of 2026-08-14.) This is unofficial and explicitly called out by the vendor's own issue tracker as a workaround rather than a sanctioned path — but it is real, reproducible from open source, and **scope-independent**: grim could, at global-scope install time, write both the hook definition (wherever) and the matching `trusted_hash` into the user's `~/.codex/config.toml` in the same operation, closing the review gap entirely without `--dangerously-bypass-hook-trust`. Doing so silently would go against the vendor's stated intent (human review via `/hooks`); if grim ever adopts this, it should be an explicit, disclosed opt-in, not a default.

## 3. File-vs-inline precedence

**Not "one wins" — both are loaded and merged (union), with only a warning.** `codex-rs/hooks/src/engine/discovery.rs:144-172`, verbatim:

```rust
let json_hooks = match layer.hooks_config_folder() { ... => load_hooks_json(...), _ => None };
let toml_hooks = load_toml_hooks_from_layer(layer, &mut warnings);

if let (Some((json_source_path, json_events)), Some((toml_source_path, toml_events))) =
    (&json_hooks, &toml_hooks)
    && !json_events.is_empty() && !toml_events.is_empty()
{
    warnings.push(format!(
        "loading hooks from both {} and {}; prefer a single representation for this layer",
        json_source_path.display(), toml_source_path.display()
    ));
}

for (source_path, hook_events) in [json_hooks, toml_hooks].into_iter().flatten() {
    append_hook_events(..., hook_events, policy);
}
```

Both are appended into the same handler list at the same layer — this is consistent with the general "sources append, they never override" merge model documented in the prior report's §2 (`append_hook_events`). The only user-visible effect of having both is a load-time warning string; nothing is dropped or shadowed. Each format still gets its **own** trust key (`key_source` is the literal file path, so `hooks.json` and `config.toml` produce different keys even for equivalent content) — the earlier "converge on the same trust identity" language (§9 of the prior report, and the `hook_hash` doc comment quoted in §2 above) refers strictly to the **hash matching** across formats, not to a shared **storage key**. Moving a hook from `hooks.json` to inline TOML at the same scope therefore still requires a fresh trust approval under the new key, even though its hash would compare equal to the old, now-orphaned record if anyone bothered to look.

## 4. Own-the-file collision reality

**No `hooks.d/`-style directory-glob exists.** Source-checked exhaustively: `codex-rs/hooks/src/engine/discovery.rs::load_hooks_json` reads exactly one fixed path per layer — `config_folder.join("hooks.json")` (line 341) — with no wildcard, no secondary file, no "additional hook files" list. `layer.hooks_config_folder()` (`codex-rs/config/src/state.rs:236`) resolves to a single override-or-default folder per layer (project `.codex/`, user `~/.codex/`), not a directory that's globbed for multiple files. This confirms, more strongly than the prior report's phrasing, that **grim genuinely has no way to own "its own file" alongside a user's pre-existing `hooks.json` at the same scope** — it is one file, full ownership or none, exactly as the brief's option 1 risk states. `managed_dir`/`windows_managed_dir` (on `ManagedHooksRequirementsToml`, per the prior report's §2/§9) are reached only via `requirements.toml`, which is itself an admin/MDM channel — a non-admin grim install cannot redirect them to a grim-owned directory; this was already established and is not contradicted by anything found in this pass.

**How likely is a pre-existing `hooks.json`, empirically:** growing, not hypothetical. Hooks shipped as a supported feature only in **March 2026** (openai/codex#2109 closed 2026-03-27), yet by this research date there is already at least one documented third-party wrapper vendor writing to `~/.codex/hooks.json` at install time (openai/codex#21615, opened well before this research date, explicitly describing "a local IDE/wrapper integration that runs Codex CLI and installs Codex hooks into `~/.codex/hooks.json`"), plus routine hand-authored project hooks appearing across many of the bug reports surveyed in this pass (`#26383`, `#30835`, `#24211`, `#35306`, `#38295` all show real `.codex/hooks.json` or `~/.codex/hooks.json` content already in place). The collision surface is real today and will only grow as more tools adopt the same install pattern grim is considering.

## 5. Headless/CI

**An untrusted (or index-shift-invalidated) hook is silently skipped, not a blocking error.** Confirmed directly from the gating logic in `append_matcher_groups` (`discovery.rs:648-666`):

```rust
if enabled
    && (source.bypass_hook_trust
        || matches!(trust_status, HookTrustStatus::Managed | HookTrustStatus::Trusted))
{
    handlers.push(ConfiguredHandler { ... });   // only path into the runtime handler list
}
```

An `Untrusted`/`Modified` hook still gets a `HookListEntry` (visible in `/hooks` or `hooks/list`) but never enters `handlers`, i.e. it never runs, with no warning surfaced to the turn itself — consistent with the vendor's own prose ("skipped until trusted") and with issue titles like `#35306` ("...SessionStart hooks are silently skipped").

**State of openai/codex#32491**: still **open** (last activity 2026-08-09), but its own most recent comment (2026-08-09, from a different reporter than the original) reports a **negative result on the current stable, 0.147.0**: with trust persisted and no `--dangerously-bypass-hook-trust`, `codex exec` correctly dispatched both registered project hooks. The original repro was filed against `codex-cli 0.144.1`. Read together: the headless-trust-skip bug most likely **regressed and was independently fixed somewhere in the 0.144.1→0.147.0 range**, but the issue has not been formally closed by a maintainer as of this research date — treat as **medium-high confidence resolved on current stable, not yet confirmed by the vendor**. A related, more severe historical report, **openai/codex#26383** ("`codex exec` ... does not appear to dispatch repo hooks ... even when `--dangerously-bypass-hook-trust` is supplied", filed against 0.137.0), is also still open with no confirming re-test comment — treat its current status as **unknown**, plausibly also stale given the pace of fixes in this subsystem, but not independently re-tested in this pass.

**Non-interactive trust-clearing paths other than `--dangerously-bypass-hook-trust`:** none *officially supported*. `#21615` (open) is the vendor-tracked ask for exactly this, unresolved. The one real technical alternative — pre-writing `[hooks.state."<key>"] trusted_hash = "<computed hash>"` directly into the user's `~/.codex/config.toml` (§2 above) — is unofficial, vendor-unendorsed, and explicitly named as a stopgap by the same open issue, not a documented feature. There is no CLI verb (`codex hooks trust <key>` or similar) found in `codex-rs/cli/src/*` for scripted trust approval outside of `--dangerously-bypass-hook-trust` and the interactive `/hooks` TUI.

## 6. Recommendation

**Surface 2 (`[hooks]` splice in `config.toml`) at global scope, one matcher group per event, fallback to Surface 1 (own `hooks.json`) if project experience shows the TOML splice collides with hand-authored `[hooks]` tables more than expected.**

Trade-off named plainly: none of the three surfaces reduces the fundamental exposure that Codex's hook trust is (a) positional-key-based rather than content-addressed for its *persisted* state, and (b) always recorded in the user's own `config.toml` regardless of where the hook itself lives. The plugin route's real, verified advantages — dictionary-keyed marketplace/plugin identity, Claude-plugin-manifest compatibility, `CLAUDE_PLUGIN_ROOT`-style env aliasing — do not offset the extra machinery it requires (a marketplace registration *and* a plugin registration, both still `config.toml` edits, plus a probable Codex-owned cache copy step whose "runs in place vs. copied" behavior for local sources is not fully confirmed here). Going straight to a named `[hooks]`-adjacent config table gets grim the same dictionary-keyed identity at lower design and maintenance cost, using exactly the span-preserving TOML editor grim already has for MCP servers.

If Surface 2 proves wrong in practice (e.g., real-world `[hooks]` collisions are worse than modeled, or Codex ships a stable per-hook `id` field that changes the calculus — watch openai/codex#31469 and #25293, both open asks for exactly that), fall back to Surface 1: full-file ownership is simpler to reason about and to diff for tampering, at the cost of the brief's stated exit-65 collision risk, which is real but rarer than a `[hooks]` table collision inside an otherwise-hand-authored `config.toml`.

---

## Sources

| URL | What it establishes | Fetched |
|---|---|---|
| `openai/codex` git clone, HEAD `8630bb3c`, 2026-08-14T09:27:41Z | All source citations below, live-read from a local shallow clone (not WebFetch-truncated) | 2026-08-14 |
| `codex-rs/exec-server-protocol/src/protocol.rs:46-50` | `DISCOVERABLE_PLUGIN_MANIFEST_PATHS` — the three cross-vendor manifest paths (`.codex-plugin`, `.claude-plugin`, `.cursor-plugin`) | 2026-08-14 |
| `codex-rs/plugin/src/manifest.rs`, `codex-rs/core-plugins/src/manifest.rs` | `PluginManifest`/`RawPluginManifest` full field list; all fields optional, `deny_unknown_fields` absent, name falls back to directory basename | 2026-08-14 |
| `codex-rs/utils/plugins/src/plugin_namespace.rs` | `AGENT_PLUGIN_MANIFEST_RELATIVE_PATH = "plugin.json"`, legacy `.codex-plugin/plugin.json` fallback resolution | 2026-08-14 |
| `codex-rs/config/src/marketplace_edit.rs` | `[marketplaces.<name>]` TOML shape (`source_type`, `source`, `last_updated`, `ref`, `sparse_paths`), verbatim edit/test code | 2026-08-14 |
| `codex-rs/config/src/plugin_edit.rs` | `[plugins."<name>@<marketplace>"]` TOML shape (`enabled`, `mcp_servers`), verbatim edit/test code, symlink-following write path | 2026-08-14 |
| `codex-rs/config/src/types.rs:844-849` | `PluginConfig` real schema fields (`enabled`, `mcp_servers` only; `deny_unknown_fields` set here, unlike the manifest) | 2026-08-14 |
| `codex-rs/cli/src/plugin_cmd.rs` | `codex plugin add/list/remove/marketplace` CLI surface; confirms plugin install is a distinct, config-table-plus-cache-copy operation | 2026-08-14 |
| `codex-rs/core-plugins/src/manager.rs:1412-1466` | `install_plugin` resolves + validates + copies into an install root; distinct from merely registering a marketplace | 2026-08-14 |
| `codex-rs/core-plugins/src/marketplace_add.rs`, `source.rs` | Local marketplace add is synchronous/offline (`MarketplaceSource::Local`, no network) | 2026-08-14 |
| `codex-rs/core-plugins/src/loader.rs:1174-1266` | `PluginHookSource` construction only for configured/enabled plugins (`configured_plugins_from_stack` gate) | 2026-08-14 |
| `codex-rs/hooks/src/engine/discovery.rs:60-283` | Full discovery pipeline: `HookHandlerSource`, `append_managed_requirement_handlers`, `append_plugin_hook_sources` (env vars `PLUGIN_ROOT`/`PLUGIN_DATA`/`CLAUDE_PLUGIN_ROOT`/`CLAUDE_PLUGIN_DATA`, `is_managed: false` for plugins), `hook_metadata_for_config_layer_source` (managed-vs-not table) | 2026-08-14 |
| `codex-rs/hooks/src/engine/discovery.rs:337-403` | `load_hooks_json` (single fixed `hooks.json` path, no glob), `load_toml_hooks_from_layer`, both-loaded-with-warning precedence logic | 2026-08-14 |
| `codex-rs/hooks/src/engine/discovery.rs:450-666` | `append_hook_events`/`append_matcher_groups`: per-handler `hook_hash`/`hook_key` computation, the exact gating condition (`enabled && (bypass || Managed/Trusted)`) that makes untrusted hooks silently non-executing | 2026-08-14 |
| `codex-rs/hooks/src/engine/discovery.rs:711-753` | `hook_hash` (content-normalized, format-independent), `hook_trust_status`, `hook_trusted_hash` — the hash-vs-key split at the center of the index-shift finding | 2026-08-14 |
| `codex-rs/hooks/src/lib.rs:105-115` | `hook_key()` exact format string (`"{key_source}:{event}:{group_index}:{handler_index}"`) | 2026-08-14 |
| `codex-rs/hooks/src/config_rules.rs` | `hook_states_from_stack` — trust state read ONLY from `User`/`SessionFlags` config layers, never from Project/Managed/Plugin layers, with verbatim doc comment | 2026-08-14 |
| `codex-rs/config/src/state.rs:236` | `hooks_config_folder()` — one folder per layer, no multi-file convention | 2026-08-14 |
| https://github.com/openai/codex/issues/21615 | Third-party wrapper vendor confirms the `[hooks.state."<key>"] trusted_hash = "..."` self-trust workaround is real, reproducible, and already in use; asks (unresolved) for a supported API | 2026-08-14 |
| https://github.com/openai/codex/issues/32491 | Headless `codex exec` + persisted trust gap; original repro on 0.144.1; latest comment (2026-08-09) reports non-reproduction on 0.147.0 | 2026-08-14 |
| https://github.com/openai/codex/issues/26383 | Earlier, more severe report: hooks not dispatched under `codex exec` even with the bypass flag (0.137.0); status of fix not independently re-tested here | 2026-08-14 |
| https://github.com/openai/codex/issues/24211 | User-level `hooks.json` `PostToolUse` historically ignored while plugin `PostToolUse` ran (0.133.0) — corroborates the plugin-key format (`name@marketplace:hooks/hooks.json:post_tool_use:0:0`) live, and shows the subsystem's churn | 2026-08-14 |
| https://github.com/openai/codex/issues/38295 | Windows: Claude-marketplace plugin hooks resolve `bash` to the WSL shim and fail; a plugin-route-specific reliability hazard for shell-script hook commands (not for a direct-binary command like grim's) | 2026-08-14 |
| https://github.com/openai/codex/issues/31469, /25293 | Still-open asks for a per-hook name/description field — corroborates no stable non-positional identity exists yet | 2026-08-14 |
| https://github.com/openai/codex/issues/21753 | Umbrella parity tracker, still open, last updated 2026-07-30, 29 comments — confirms the surface is still actively contested/expanding | 2026-08-14 |
| `.agents/research/hooks_vendor_reports/codex.md` (this repo, 2026-08-14) | Baseline facts not re-derived: event catalogue, output/response contract, `--dangerously-bypass-hook-trust` exact flag, `requirements.toml`/`managed_dir` admin channel, `ClaudeHooksEngine` naming, overall trampoline viability for `type: command` | 2026-08-14 |
