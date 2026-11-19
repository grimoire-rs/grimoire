---
paths:
  - src/install/mod_*.rs
---

# Install Layout: Mods

Where a mod lands per client and what grim records. Index: [subsystem-file-structure.md](./subsystem-file-structure.md). Decision: `adr_mod_artifact_kind.md`; user contract: `docs/src/content/docs/mods.md`.

### Mods {#install-layout-mods}

A **mod** (experimental, behind `[options.experimental] mods`) is publisher code a client loads into its own process. grim writes the code and the registration; consent decides what grim writes, never what the client loads (T3, `arch-threat-model.md` I1).

| Path | What | Source |
|---|---|---|
| `$GRIM_HOME/mods/<workspace-key>.json`, `$GRIM_HOME/mods/global.json` | The **mod store**: one file per project and one for global scope, kept apart from install state so a repository cannot supply it. Envelope = records plus `shared` (outputs several mods rely on) and `disarmed` (`(mod, client)` pairs a disarm left kept files for) | `mod_store.rs` (`STORE_DIR`, `GLOBAL_FILE`) |
| `$GRIM_HOME/mods/marketplace-id` | Machine-local identity (32 random bytes, mode 0600) that names the global marketplace | `mod_store.rs` (`ID_FILE`) |
| `$GRIM_HOME/consent/<workspace-key>.json` | The **workspace consent record**, shared by every gated kind: `{v, workspace, sources, consented_at}` with `v` = 2, `deny_unknown_fields`, every field required; `sources` holds unprefixed `<binding>@<registry>/<repository>` entries; an unusable record reads as absent; `grim hook revoke` deletes it and also deletes an older grim's record under `$GRIM_HOME/hooks/consent/`, which nothing reads as consent | `hook/consent.rs` |
| `.claude/mods/<binding>/` (project), `mods/<binding>/` under the Claude root (global) | Claude plugin tree: `src/**`, `.claude-plugin/plugin.json`, `hooks/hooks.json` (written last) | `plan_claude` |
| `.claude/mods/.claude-plugin/marketplace.json` | The scope's **shared** Claude marketplace, listing every Claude mod | `render_marketplace`, `write_shared` |
| `.claude/settings.local.json` (project) | Shared entries: `/extraKnownMarketplaces/<marketplace>` and `/enabledPlugins/<binding>@<marketplace>` | `settings_entries` |
| `.opencode/plugins/<binding>/` (project), `plugins/` under the OpenCode root (global) | OpenCode plugin: `src/**` plus the generated `index.ts` (written last), only when the scope declares `plugin_api = "v2"` | `plan_opencode`, `host_declared` |

**Marketplace name.** `grim-mods-` plus 12 lowercase hex characters of the scope id (`marketplace_name`). grim owns every `extraKnownMarketplaces` key with that prefix: `write_shared` removes each other `grim-mods-…` key (prefix rule). `claude-mods` and the prefixes `claude-`, `anthropic-`, `anthropics-`, `cc-plugin-` are Claude-reserved binding names (`claude_binding_refusal`): Claude alone declines them.

**Write-ahead and disarm ownership.** The write half records both generations in the store before the first byte moves (`write_ahead_outputs`); a failed row never stops another. The disarm half runs whatever the write half did, before the exit is decided, for a gated, pruned or no-longer-hosted mod or a client `[options].clients` no longer names, and from the uninstall paths (`disarm`). Shared Claude outputs are written from the store after every row, never per row, each only while it is grim's; a refused shared output is reported on the run's first Claude mod row. Revoking consent deletes the record but disarms nothing by itself: a written mod reads `installed` until the next converging command.

**`ModReach`.** `Full` (bare `install`/`update`, TUI Install) writes and prunes over the whole lock; `Closure {install, prune}` (a named `update`, TUI Update, `add`) writes only `install` and prunes only `prune`; `Skip` (dev install or refresh) neither writes nor disarms, since a mod has no path source.

**Status reads the store.** `grim status` judges a mod from the store record, never from install state. A `disarmed` pair counts as unwritten, so its gate cause shows and its kept files read `mod-files-present`. Advisory causes `mod-files-tracked`, `mod-files-present` and `mod-disabled-in-client` exit 0 (`api/artifact_status.rs`); a hand edit of the shared marketplace file or its settings entry moves every installed Claude mod row of the scope to `modified`.

**Invariants.** A relative or workspace-nested `$GRIM_HOME` refuses (`validate_grim_home`) and the slot reads `Refused`: none read, none written. Global mod roots take the settings root fence (`root_refusal`). Every output is anchor-contained (`AnchoredPath`). A mod store that cannot be read warns and is not rewritten.

See also: [subsystem-install-layout-hooks.md](./subsystem-install-layout-hooks.md) (dispatch table, launcher), [subsystem-install-layout-settings.md](./subsystem-install-layout-settings.md) (the settings store this envelope extends), [subsystem-path-anchors.md](./subsystem-path-anchors.md).
