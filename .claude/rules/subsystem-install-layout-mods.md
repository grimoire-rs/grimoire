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
| `.claude/mods/.claude-plugin/marketplace.json` | The scope's **shared** Claude marketplace, listing every Claude mod | `render_marketplace`, `write_marketplace` |
| `.claude/settings.local.json` (project) | Shared entries: `/extraKnownMarketplaces/<marketplace>` and `/enabledPlugins/<binding>@<marketplace>` | `settings_entries` |
| `.opencode/plugins/<binding>/` (project), `plugins/` under the OpenCode root (global) | OpenCode plugin: `src/**` plus the generated `index.ts` (written last), unless the scope sets `plugin_api = "v1"` (unset or `"v2"` hosts) | `plan_opencode`, `host_declared`, `opencode_hosts` |

**Marketplace name.** `grim-mods-` plus 12 lowercase hex characters of the scope id (`marketplace_name`). grim owns every `extraKnownMarketplaces` key with that prefix: `apply_settings` removes each other `grim-mods-…` key (prefix rule). `claude-mods` and the prefixes `claude-`, `anthropic-`, `anthropics-`, `cc-plugin-` are Claude-reserved binding names (`claude_binding_refusal`): Claude alone declines them.

**Write-ahead and disarm ownership.** The write half records both generations in the store before the first byte moves (`write_ahead_outputs`); a failed row never stops another. The disarm half runs whatever the write half did, before the exit is decided, for a gated, pruned or no-longer-hosted mod or a client `[options].clients` no longer names, and from the uninstall paths (`disarm`). Shared Claude outputs are written from the store after every row, never per row, each only while it is grim's; a refused shared output lands on the first Claude row with no failure of its own (`shared_home`), never over a row's own refusal; if every Claude row failed, the step still runs and its report is logged as a warning. `mod_dir` and `mod_fence` live once in `mod_phase.rs`; status reuses them. Revoking consent deletes the record but disarms nothing by itself: a written mod reads `installed` until the next converging command.

**`ModReach`.** `Full` (bare `install`/`update`, TUI Install) writes and prunes over the whole lock; `Closure {install, prune}` (a named `update`, TUI Update, `add`) writes only `install` and prunes only `prune`; `Skip` (dev install or refresh) neither writes nor disarms, since a mod has no path source.

**Status reads the store.** `grim status` judges a mod from the store record, never from install state. A `disarmed` pair counts as unwritten, so its gate cause shows and its kept files read `mod-files-present`. Advisory causes `mod-files-tracked`, `mod-files-present` and `mod-disabled-in-client` exit 0 (`api/artifact_status.rs`), as do the three that name a served client the mod is not written for (`mod-plugin-api-unsupported`, `mod-client-unsupported`, `mod-name-reserved`, one per `ModHosting` decline, derived in `mod_arming`); the TUI leads its status line with the same decline text (`HookNote::declined`, `declined_warnings`); a hand edit of the shared marketplace file or its settings entry moves every installed Claude mod row of the scope to `modified`.

**Invariants.** A relative or workspace-nested `$GRIM_HOME` refuses (`validate_grim_home`) and the slot reads `Refused`: none read, none written. Global mod roots take the settings root fence (`root_refusal`). Every output is anchor-contained (`AnchoredPath`). A mod store that cannot be read warns and is not rewritten.

See also: [subsystem-install-layout-hooks.md](./subsystem-install-layout-hooks.md) (dispatch table, launcher), [subsystem-install-layout-settings.md](./subsystem-install-layout-settings.md) (the settings store this envelope extends), [subsystem-path-anchors.md](./subsystem-path-anchors.md).

**TUI consent.** A TUI Install that declares a mod row, or newly brings one through a bundle, records its consent as `grim add` does, at project scope with the mods flag on (`consent_site` carries the flag; settings stay off). Update and a repair Install record nothing. The status line names a consented mod, and a mod the policy refused with its refusal text. A single row that wrote nothing prints that note alone, without `installed (1 ok)`. The title row cuts the status line, so the full text of an unwritten or declined mod also opens in a `Mods` notice (`show_mod_notes`), or joins an open dialog or panel.

**Browse reads the store.** `grim search` and the TUI add the store's records to the in-memory install state they read for badges (`mod_store::overlay_records`), so an installed mod reads `installed` and a bundle with a mod member does not read `pending`. Nothing is written; an absent or unreadable store adds nothing. A mod row reads `pending` (`status_badge::mod_client_unwritten`, shared by the badge and the TUI row state) when the target names a served client with no recorded output and `mod_hosting` answers `Hosted`; `NotServed`, `Unsupported`, `PluginApiV1` and `ReservedByClaude` never do, since `pending_outputs` ignores mods.

**Build cap.** `grim build` counts the packed files (`pack_mod_dir`), the same bytes the install validator counts. The install validator bounds untrusted input; keep the two counts equal.
