# System Design: the experimental `mod` artifact kind

<!--
System Design Document (Technical Architecture)
Filename: .agents/specs/design_mod_artifact_kind.md
Owner: Architect (/hex-architect)
Handoff to: /hex-plan
-->

## Metadata

**Status:** Draft
**Author:** Claude (architect, `/hex-architect` xhigh)
**Date:** 2026-11-18
**Beads Issue:** N/A
**Related PRD:** N/A (`.agents/discussions/harness-mods-artifact-kind.md`)
**Related ADRs:** `.agents/adr/adr_mod_artifact_kind.md` (decisions D1 to D15; this file holds the detail), `adr_settings_artifact_kind.md`, `adr_hook_workspace_consent.md`, `adr_artifact_trust_model.md`, `adr_render_layout_stability.md`, `adr_harness_plugin_export.md`

**Tech Strategy Alignment:**
- [x] Rust 2024, Tokio, `serde_json`, `toml`; no new dependency
- [x] No deviation; nothing here needs a database, infrastructure tier or OTel change

## Executive Summary

A `mod` artifact carries publisher code a harness loads into its own process: a Claude Code mod and an OpenCode V2 plugin. Grim packs one tar layer with a `claude/` and an `opencode/` directory. At install it re-validates the layer and the binding, writes the code under a grim-owned `src/` directory, generates each client's wrapper files with a serializer, and registers the result where the client loads it, only after workspace consent. Grim gates what it writes; it never claims to gate what the client loads. It follows the owner's rulings of 2026-11-18: consent option B, and payload option P1, both clients in their own project folder. On 2026-11-19 the owner ruled the prerequisite fix (ADR D15), so no decision point is open (§ 13).

---

## 1. Context (C4 Level 1)

```text
 publisher ── grim build / release ──► OCI registry ◄── grim add / install ── developer
                                             │                                   │
                                             ▼                                   ▼
                                        grim (CLI) ── writes files, settings entries, records
                                             │
          ┌──────────────────────────────────┼─────────────────────────────────┐
          ▼                                  ▼                                 ▼
   $GRIM_HOME (consent record,        git work tree (.claude/mods/,     Claude / OpenCode config
   mod store, content store)          .opencode/plugins/, .claude/      roots (global scope),
                                      settings.local.json)              Claude settings.json
          ▲                                  ▲
          │                                  │ clone, pull, fork-PR checkout (T3)
     only grim reads it               Claude Code / OpenCode V2 load the code at session start
```

| Actor | Role | Threat-model class |
|---|---|---|
| Publisher | Builds and pushes a mod; names bundle members and `requires` edges | T1 when hostile |
| Developer | Declares, consents, installs | The user; N4 when bypassing a gate they saw |
| Teammate, cloner, fork contributor | Changes the repository or its environment | N1 (teammate), T3 (cloner, fork) |
| Claude Code 2.1.287+, OpenCode V2 | Load the written code with no per-mod prompt | N3 out of scope |

## 2. Containers (C4 Level 2)

| Container | Holds | New for this kind |
|---|---|---|
| Registry | Manifest (`com.grimoire.kind=mod`, `com.grimoire.mod.clients`), content layer, optional requires layer | annotation |
| `grimoire.toml` / `grimoire.lock` | `[mods]`, `[[mod]]` rows with `clients`; `[options.vendors.opencode] plugin_api` | table, array, key |
| `$GRIM_HOME/consent/<key>.json` | One consent record per workspace, `v: 2` | path, key, version |
| `$GRIM_HOME/mods/<key>.json`, `$GRIM_HOME/mods/global.json` | Every mod record, plus each scope's shared outputs | new store |
| `$GRIM_HOME/mods/marketplace-id` | Random id behind the global marketplace name | new file |
| Work tree | Project payload and wrappers, `.claude/settings.local.json` entries | written outputs |
| Client roots | Global payload, `<claude-root>/settings.json` entries | written outputs |
| Claude internal state (`known_marketplaces.json`, plugin cache) | Claude's records | never written by grim |

`.grimoire/state.json` and `$GRIM_HOME/state/global.json` never hold a mod record.

## 3. Components (C4 Level 3)

| Component | Responsibility | Home |
|---|---|---|
| `ModManifest` | Parse `mod.toml` in two modes (build strict, install tolerant of unknown harness tables), 64 KiB cap | new `src/oci/mod_manifest.rs` |
| Mod packer | Walk the source dir, refuse symlinks and non-code files, resolve default entries, write the packed `mod.toml` with the `toml` serializer, stamp annotations | `src/skill/skill_package.rs`, `src/command/build.rs`, `src/oci/annotations.rs` |
| Layer validator | The ADR D3 rules on a fetched layer, before any write or export staging | beside `ModManifest` |
| Binding gate | `SkillName` plus the device-name refusal on every mod row, before any key or path is minted | `src/install/installer.rs` (beside the hook gate at `:961`) |
| Mod renderer | Author tree under `src/` plus `plugin.json`, `hooks.json` or `index.ts`, per client | new `src/install/mod_render.rs`, called from `ClientTarget::materialize` and export staging |
| Mod convergence | Per scope and client: write in order, disarm, the shared marketplace file and entry, `enabledPlugins` entries, the prefix rule, advisory causes | new `src/install/mod_phase.rs`, `json_splice.rs` |
| Mod store | `{v, workspace, records, shared}` under `$GRIM_HOME/mods/`, write-ahead save, merge on load, split on save | `src/install/settings_store.rs` generalised by store directory; the settings store's own envelope stays unchanged (§ 6.5) |
| Root fence | The settings kind's `root_refusal` applied to global mod roots | `src/install/settings_phase.rs` (shared) |
| Consent | Record v2 at the new path, unprefixed entries, union over gated kinds, `ConsentPolicies { hooks, settings, mods }` | `src/hook/consent.rs`, `src/hook/policy.rs`, `src/install/hook_dispatch.rs` |
| Support predicate | `client_hosts` with the row's `clients` and the OpenCode `plugin_api` opt-out | `src/install/installer.rs` |
| Export | `admits` with the served set, the one-mod-per-plugin rule, the output fence for mod plugins, the `types` field | `src/export/family.rs`, `src/export/stage.rs` |
| Churn probe | No-login load probe against the latest clients | a `workflow_dispatch` job in `.github/workflows/verify-deep.yml` |

## 4. Key Design Decisions

All recorded in the ADR: D1 surface, D2 descriptor, D3 re-validation, D4 files, D5 I1, D6 consent, D7 shared-file and key ownership, D8 lifecycle, D9 served clients, D10 class 3 and the churn detector, D11 order, D12 older grims, D13 export, D14 stability, D15 prerequisite fix, plus § Payload home. No owner decision point stays open there.

---

## 5. Flows

### 5.1 Build and release

1. `grim build <dir>`: `SKILL.md` and `hook.toml` are tested first and absent; `mod.toml` is present, so the kind is `mod` (or `--kind mod`).
2. Read `mod.toml` (at most 64 KiB) in build mode: unknown keys and tables refuse (65). `name` passes `SkillName` and equals the directory stem.
3. Walk with `pack_tree_limited` and `MOD_NEVER_IGNORED = ["mod.toml"]`. A symlink is a build error (65), not a silent skip, since a dropped helper breaks the module at load.
4. The top level holds `mod.toml` and at least one of `claude/`, `opencode/`; anything else refuses (65). Each file under a harness dir has a code extension.
5. For each served harness with no `entry`, find the one top-level file whose stem is `name`; none or several refuse (65). An OpenCode entry ends in `.ts` or `.js`, and its stem is unique among code files in its directory.
6. Write the tar rooted at `<name>/` with the packed `mod.toml`, serialised by `toml` from the parsed struct (resolved entries filled in). Caps: 16 MiB, 2,000 files, depth 16.
7. Annotations: kind, title, `com.grimoire.mod.clients`. A manifest whose requires layer or bundle members name a mod, a mod included, sets `com.grimoire.requires-features` with `mods`.
8. `grim release` and `grim publish` follow the tar-kind path; publish order puts mods after settings, before bundles.

### 5.2 Converging commands (install, update, add, uninstall)

1. Resolve scope, workspace and clients. `validate_grim_home` runs first; a refusal reads `not-armed` (`grim-home-*`), exit 0, and nothing is read or written.
2. Global scope: the root fence runs on each client's payload root and registration file; a refusal reads `not-armed` (`settings-root-relative` or `settings-root-in-workspace`) for that client, exit 0.
3. Load the mod store. Read the lock; each `[[mod]]` row brings `clients`.
4. Binding gate on every mod row (config key, bundle member, `requires` edge): a failure refuses that row (`invalid-mod`, 65) before any key or path exists.
5. Flag off: every mod is `gated` (`feature-flag-off`); go to step 11.
6. Consent: one read of `$GRIM_HOME/consent/<key>.json`; per gated kind, its declared entries are checked against the record; the flag pair overrides in both directions; the transport gate refuses a non-loopback plain-HTTP pin; an interactive run prompts once if any kind needs an answer (the prompt names each entry's kind and marks kinds whose flag is off), and accepting records the union. `grim add` records its gesture before installing.
7. Hosting clients per granted mod: `selected ∩ clients ∩ client_hosts`. OpenCode hosts unless `plugin_api = "v1"` (the resolved scope's config only, no cross-scope merge; unset means V2, owner ruling 2026-11-23); with `"v1"` the client declines with one warning naming the key. Claude declines a binding its validator refuses, with one warning.
8. Reach: a bare `install` or `update` writes every granted mod; `add` and a named `update` write only their closure.
9. For each mod to write: fetch the layer (store first; a missing layer offline makes the run exit 81 at the end, after step 11), verify the pinned digest, run the D3 validator (`invalid-mod`, 65), render in memory, run the integrity gate against the store records (unchanged skips; a modified output or an untracked destination refuses, 65, forceable).
10. Write-ahead: save the store with the planned outputs. Then write in order. OpenCode: on an update delete `index.ts` first; write `src/**`; write `index.ts` last. Claude: `src/**`, `plugin.json`, `hooks/hooks.json`. Each file goes to a temporary name in its own directory and is renamed. Save the store with the final hashes.
11. **Disarm half, over every mod of the scope and every client that holds its outputs, whatever the reach and whatever `--client` selects**, as hook convergence ignores `--client`. It needs no network and runs even when an earlier row failed. A mod is disarmed when it is gated, pruned (bare runs: any dropped row; named runs and `add`: the previous closure only) or uninstalled. Order: Claude `enabledPlugins` entry, then `hooks/hooks.json`, then the rest; OpenCode `index.ts`, then the rest. A file is deleted when it equals the recorded hash or a render of a pin the record or lock names (content store); any other file is kept, stays recorded with its original hash, and is warned. A gated mod outside a named closure is named on stderr.
12. Shared outputs per scope, for every client that holds a mod output, from the store after steps 10 and 11 (so the prefix rule also ignores `--client`): re-render `marketplace.json` and write it, or delete it with the last mod, only under the D7 ownership rule; upsert or remove the `extraKnownMarketplaces` entry for the current name; upsert one `enabledPlugins` entry per written mod, except that a recorded entry whose value is `false` is left as it is (ADR D7; `--force` writes `true`); remove every non-current `grim-mods-…` key in that file (§ 5.8). Settings files are spliced by `json_splice` with the re-read-before-write rule (75 `locked` on a race).
13. Advisory causes: `is_tracked` on written mod paths (`mod-files-tracked`); existence of expected paths for a mod grim did not write or kept after disarm (`mod-files-present`); a recorded `enabledPlugins` value of `false` (`mod-disabled-in-client`).
14. Report: one install row per mod (`armed` is `null`), one stderr line per mod written or removed, one stderr line per mod first written under an entry this run's gesture did not add, and on `uninstall` of a scope's last mod one line naming `claude plugin marketplace remove <name>`.

Global scope differs in roots (§ 6.5), store file (`global.json`) and consent (always granted, no record).

### 5.3 Consent record

1. `grim consent allow` (alias of `grim hook allow`) records every gated artifact the workspace declares, any flag state, as unprefixed entries.
2. Write: `ensure_consent_dir` creates `$GRIM_HOME/consent/` with `0700`, file `0600`, atomic write. Then, if `$GRIM_HOME/hooks/consent/<key>.json` exists and its `workspace` field equals the resolved workspace, delete it. The old file is read for that field only and never honoured as consent.
3. Read: absent, I/O error, parse error, unknown field, unknown `v` and a foreign `workspace` all read as absent (debug log), never an error.
4. `--global` exits 64 for `allow` and returns `Absent` for `revoke`, unchanged.

### 5.4 Revoke

1. `grim consent revoke` deletes the record and, under the step 2 rule above, the old-path record.
2. It prints one stderr line per written mod: the client keeps loading it until a converging command disarms it.
3. The next converging command runs § 5.2 with consent absent: the mods are `gated` and step 11 disarms them.

### 5.5 Uninstall and remove

1. `grim uninstall mod <name>`: the target is disarmed now (§ 5.2 step 11), a drifted settings entry refuses (65, forceable) as for MCP entries, then step 12 runs. A recorded `enabledPlugins` entry set to `false` is not drift: uninstall removes it like a `true` one. Claude's `known_marketplaces.json` keeps a stale row (load paths § 5, R3); the stderr line names its removal command.
2. `grim remove mod <name>`: config and lock only (frozen contract); one stderr line names `grim uninstall mod <name>`; the next converging command prunes the orphan.

### 5.6 Status

1. Load records from the mod store; prove outputs, shared ones included, as for other kinds.
2. Row state from the shared ladder; `arming` names each client's cause; `serves` lists the row's `clients`, or `null` when the set is unknown.
3. Advisory causes as in § 5.2 step 13, never moving the row.
4. Status never reads a client log or client state and never runs a client binary.

### 5.7 Export (Claude family)

1. Resolve members; for each mod member, `admits(Claude, client, Mod, served)`: `claude` and served is `Ok`; not served, or another Claude-family client, is `client-declined`; Agent Plugins is `no-format-surface`.
2. Count admitted mods per plugin and client: one is staged; two or more are all omitted as `not-representable`, with one README line and one stderr line each.
3. Only when a plugin stages a mod: refuse an output directory inside a known client config root (`<claude-root>`, `<workspace>/.claude`, `<workspace>/.opencode`, the OpenCode config root) with `ExportError::Usage` (64), before any write. An export that stages no mod skips this check and keeps its current exit code (Principle 9, `stability.md:33`).
4. Run the D3 validator on the single mod, then stage: author tree under `<plugin>/src/`, `<plugin>/hooks/hooks.json` with the install bytes, and `types` in the export's `ClaudeManifest` when `src/types/index.d.ts` exists.
5. The plugin version hash covers the new files, so a wrapper change moves every exported mod plugin's version (grammar frozen, inputs not).

### 5.8 Moved or copied checkout

1. The workspace key changes with the path. Consent asks again; the new store is empty; the current marketplace name changes.
2. Existing `.claude/mods/<b>/` or `.opencode/plugins/<b>/` files are untracked destinations: install refuses them (65) unless `--force`, which adopts a file equal to a render of the pin and replaces any other.
3. The copied `settings.local.json` still names the old `grim-mods-<h12>` entries with the old absolute path. § 5.2 step 12 removes every non-current `grim-mods-…` key and writes the current ones.
4. The old workspace's store and consent files stay under `$GRIM_HOME`, inert, as consent records already do.

---

## 6. Data shapes

### 6.1 `mod.toml` (`grim schema --kind mod`)

| Key | Type | Build mode | Install mode |
|---|---|---|---|
| `schema` | integer | `1` | `1` |
| `name` | string | `SkillName`, equals the directory stem | as packed |
| `description` | string | required | as packed |
| `[claude]`, `[opencode]` | table | optional; one per harness directory | unknown keys inside refuse |
| unknown top-level table | table | refuse | ignored, as a harness this grim does not know |
| unknown top-level key | any | refuse | refuse |
| `<harness>.entry` | string | optional; resolved and packed | required for every served harness |
| `[requires]` | table | as `hook.toml` | as `hook.toml` |

A later key inside a known table ships with its own `com.grimoire.requires-features` token on the mod, so a first-version grim answers "update grim" (65) before it parses.

### 6.2 Layer tree

```text
status-band/mod.toml
status-band/claude/register.tsx
status-band/claude/lib/format.ts
status-band/claude/types/index.d.ts
status-band/opencode/status-band.ts
status-band/opencode/helper.ts
```

### 6.3 Manifest, config, lock

```text
annotations:
  com.grimoire.kind               = mod
  org.opencontainers.image.title  = status-band
  com.grimoire.mod.clients        = claude,opencode
```

```toml
# grimoire.toml
[options.experimental]
mods = true
[mods]
status-band = "ghcr.io/acme/mods/status-band:1"

# grimoire.lock (fields other than clients as for every kind)
[[mod]]
name = "status-band"
pinned = "ghcr.io/acme/mods/status-band:1.2.0@sha256:..."
clients = ["claude", "opencode"]
```

`clients` is required on `[[mod]]` and absent elsewhere. A name this grim does not know stays in the row and is ignored.

### 6.4 Consent record v2

```json
{
  "v": 2,
  "workspace": "/home/dev/src/app",
  "sources": [
    "fmt@ghcr.io/acme/hooks/fmt",
    "status-band@ghcr.io/acme/mods/status-band",
    "team@ghcr.io/acme/settings/team"
  ],
  "consented_at": "2026-11-18T10:00:00Z"
}
```

Entries follow option B, the owner's ruling. Every field is required, unknown fields are denied, the array is a sorted set.

### 6.5 Mod store

```json
{
  "v": 1,
  "workspace": "/home/dev/src/app",
  "records": ["<InstallRecord>, as in state.json, with planned outputs saved before the write"],
  "shared": [
    {"client": "claude", "anchor": "workspace", "relative": ".claude/mods/.claude-plugin/marketplace.json", "content_hash": "<sha256>"},
    {"client": "claude", "anchor": "workspace", "relative": ".claude/settings.local.json", "entry": "/extraKnownMarketplaces/grim-mods-3fa9c2d1e0b7", "content_hash": "<value hash>"}
  ]
}
```

`$GRIM_HOME/mods/<key>.json` per project; `$GRIM_HOME/mods/global.json` for global scope, where `workspace` is `null`. Dir `0700`, file `0600`, `deny_unknown_fields`. An unusable file reads as absent and keeps its bytes at `<file>.unusable` on the next save; no records and no shared outputs removes the file. These are the settings store's rules. The `shared` slot exists in the mod store only. The settings store keeps its `{v, workspace, records}` envelope byte for byte, because 0.16.0 reads an unknown field as unusable and moves the file aside, which would drop its settings records.

### 6.6 Installed locations

| Scope | Client | Anchor | Relative paths |
|---|---|---|---|
| project | claude | `workspace` | `.claude/mods/<b>/src/**`, `.claude/mods/<b>/.claude-plugin/plugin.json`, `.claude/mods/<b>/hooks/hooks.json`, `.claude/mods/.claude-plugin/marketplace.json` (shared) |
| project | claude | `workspace` | `.claude/settings.local.json` entries `/extraKnownMarketplaces/grim-mods-<h12>` (shared), `/enabledPlugins/<b>@grim-mods-<h12>` |
| global | claude | `claude-root` | `mods/<b>/...`, `mods/.claude-plugin/marketplace.json`, `settings.json` entries with marketplace `grim-mods-<id12>` |
| project | opencode | `workspace` | `.opencode/plugins/<b>/index.ts`, `.opencode/plugins/<b>/src/**` |
| global | opencode | `open-code-root` | `plugins/<b>/index.ts`, `plugins/<b>/src/**` |

`<b>` is the consumer binding. `<h12>` is the first 12 hex characters of `hook_dispatch::workspace_key`. `<id12>` comes from `$GRIM_HOME/mods/marketplace-id`. Observed: Claude project registration in `settings.json` and `settings.local.json` (load paths §1), OpenCode project and global (§3, §4). Not observed: a Claude global session load through a hand-written user `settings.json` (wrapper facts §4b ran `claude plugin install` and `plugin list`, no session), and the first-session load of an absolute path from `settings.local.json`.

### 6.7 Generated files (illustrative; the serializer goldens are the byte contract)

```text
.claude/mods/.claude-plugin/marketplace.json
{
  "name": "grim-mods-3fa9c2d1e0b7",
  "owner": {"name": "grim"},
  "plugins": [{"name": "status-band", "source": "./status-band"}]
}

.claude/mods/status-band/.claude-plugin/plugin.json
{"name": "status-band", "types": "./src/types/index.d.ts"}     ("types" only when that file exists)

.claude/mods/status-band/hooks/hooks.json
{"modules": ["../src/register.tsx"]}

.claude/settings.local.json (members grim owns; json_splice keeps every other byte)
{
  "enabledPlugins": {"status-band@grim-mods-3fa9c2d1e0b7": true},
  "extraKnownMarketplaces": {
    "grim-mods-3fa9c2d1e0b7": {"source": {"path": "/home/dev/src/app/.claude/mods", "source": "directory"}}
  }
}

.opencode/plugins/status-band/index.ts
export { default } from "./src/status-band"
```

Rules: each JSON file is a `serde_json::Value` written pretty with a trailing newline, keys sorted (no `preserve_order`). `owner.name` is the literal `grim`, required by `claude plugin validate` (load paths §1). `path` is the absolute root of the marketplace directory. The OpenCode line is the fixed bytes `export { default } from `, then `serde_json::to_string("./src/" + entry without its extension)`, then a newline.

### 6.8 Reports and errors

```json
{"kind": "mod", "name": "status-band", "state": "installed", "serves": ["claude", "opencode"],
 "arming": [{"client": "claude", "cause": "mod-files-tracked", "message": "...", "transient": false}]}
```

```json
{"error": {"message": "mod 'status-band': entry 'claude/../x.ts' leaves its harness directory", "reason": "invalid-mod", "retryable": false, "forceable": false}}
```

`serves` is always present on `status` items, the `describe` report and `search` items: an array on a mod row, `null` on another kind or when the set is unknown.

---

## 7. Security Architecture

Scoped to `.claude/rules/arch-threat-model.md`. One control per line, with its attacker.

| Attacker | Control | Residual (named, not engineered against) |
|---|---|---|
| T1 hostile publisher | Flag default off (I4); consent before any write; D3 re-validation and the binding gate; author files under `src/`; serializer-only JSON; digest pin; transport gate; caps | A new version of a consented entry runs without a re-ask (I5: lock diff, install rows). The code is unsandboxed in the client |
| T1 via a consented repository | Option B (owner ruling) re-asks on a new binding, member or edge | (1'): a publisher names a member or edge with a consented binding, or a fork PR rebinds across kinds. The first-write disclosure line is the evidence |
| T2 tag swap | Digest pin; a plain-HTTP non-loopback pin is never written | As for hooks |
| T3 cloner, fork PR | Claude registration local; every record under `$GRIM_HOME`; prefix rule removes a carried-over registration; global root fence against a repository-set `CLAUDE_CONFIG_DIR` or `OPENCODE_CONFIG_DIR`; `install` writes nothing without consent; `mod-files-present` on a gated row | A branch that tracks `.claude/mods/**` replaces the copy on checkout and runs at the next session; any committed `.opencode/plugins/` file loads with no grim. Grim writes no exclude lines, as hygiene, not a control: checkout refuses to overwrite an untracked mod file only while no ignore rule matches it, and the repository's own `.gitignore` can match it (I1, I5) |
| T4 injected agent | `grim add` is a gesture (as today); MCP `grim_render` declines the kind; an export that stages a mod refuses a client config root | An agent that can run `grim add` can consent; the install rows and lock diff are the evidence |
| T5 local process | Store and record files `0600` in `0700` dirs | None new |

What grim states in docs and status: the lock pins what grim writes, not what the client loads; `installed` means written and registered, and the silent-inert causes of ADR D8 can still skip the mod; between grim commands the client controls it; revoke with the grim that granted.

## 8. Non-Functional Requirements

Only the qualities this kind moves.

| Quality | Budget or rule |
|---|---|
| Compatibility | A project without mods: lock, `state.json`, `global.json`, config hash and every report byte-identical apart from `serves: null` (p9 gate). Older grims per ADR D12 |
| Stability | ADR D14. Re-materialising leaves `status` not-modified for payload, generated files and shared outputs |
| Operability | One install row and one stderr line per write or removal; `grim status` lists every mod per client, which `claude plugin list` does not |
| Maintenance | Four JSON shapes and one re-export line (§ 6.7); a vendor change is a red probe run, a watchlist row, a renderer edit and a golden update |
| Performance | One layer read per mod that changes; disarm reads no layer when hashes match the record; one settings file per scope and client |

## 9. Touch list by area

From `research_mod_kind_architecture_map.md` § 2, checked against HEAD `c16330cd`.

| Area | Files |
|---|---|
| Kind identity | `src/oci/artifact_kind.rs` (variant, `ALL`, `all_index`, `kind_str`, `subdir`, `artifact_type`, `config_media_type`, `is_dir_artifact`, the appended-last test), `src/oci/annotations.rs`, `src/oci/layer.rs` (`is_tar_kind`), `src/oci/manifest.rs` (layout test list), `src/oci/features.rs` (`KNOWN_FEATURES`), `src/oci/bundle.rs`, `src/oci/requires_decl.rs`, `test/src/helpers.py` (`_KIND_ORDER`) |
| Authoring | new `src/oci/mod_manifest.rs`, `src/command/build.rs`, `src/skill/skill_package.rs`, `src/install/ignore_set.rs` (`MOD_NEVER_IGNORED`), `src/skill/local_pack.rs`, `src/command/release.rs`, `src/command/publish.rs`, `src/command/schema.rs` |
| Config and lock | `src/config/declaration.rs` (`ExperimentalOptions.mods`, `VendorOptions.plugin_api`), `src/config/project_config.rs`, `src/config/hash.rs`, `src/config/toml_merge.rs`, `src/command/config_keys.rs`, `src/lock/{grimoire_lock,lock_io,effective_set,edit,closure,prior}.rs`, `src/lock/testdata/lock.schema.json`, `src/resolve/resolver.rs`, `src/resolve/graph.rs`, `src/resolve/pin_agreement_tests.rs` |
| Consent | `src/hook/consent.rs` (record v2, unprefixed key; `split_by_kind`, `is_settings_key` and `SETTINGS_KEY_PREFIX` go), `src/install/hook_dispatch.rs` (`consent_dir`, `ensure_consent_dir`, old-path delete, the reserved-name test), `src/hook/policy.rs`, `src/hook/trust.rs`, `src/command/hook_consent.rs`, `src/command/hook/{allow,revoke}.rs`, `src/command/add.rs`, `src/tui/app.rs` |
| Install | `src/install/installer.rs` (binding gate beside `:961`, `client_hosts`, `kind_is_permanently_declined`, `name_fits`), `src/install/vendor.rs`, `vendor_claude.rs`, `vendor_opencode.rs`, `vendor_kilo.rs`, `src/install/client_target.rs` (materialize arm, docs-parity test), new `src/install/mod_render.rs` and `mod_phase.rs`, `src/install/json_splice.rs`, `src/install/materializer.rs`, `src/install/expected_outputs.rs`, `src/install/install_state.rs` (split the kind out of both state files), `src/install/settings_store.rs` (generalise the store directory; `shared` in the mod store only), `src/install/settings_phase.rs` (share `root_refusal`), `src/install/adoption.rs` (render proof at disarm), `src/install/path_anchor.rs` (`candidate_anchors` arms), `src/install/hook_registrar.rs` (`is_tracked` users), `src/install/{prune,uninstall,status_badge}.rs` |
| Settings kind | `src/oci/settings.rs` (refuse `grim-mods*` marketplace names) |
| Reports and surfaces | `src/api/artifact_status.rs` (three causes), `src/api/status_report.rs`, describe and search reports (`serves`), `src/catalog/registry_catalog.rs`, `src/command/{status,describe,search,add,uninstall,remove,why}.rs`, `src/tui/{app,render,detail,state,tree}.rs`, `src/mcp/{render,server,tool_args}.rs` (`grim_render` declines), `src/telemetry/*`, `src/fetch.rs`, `src/error.rs` (`invalid-mod`) |
| Export | `src/export/family.rs` (`admits` with the served set, `ClaudeManifest.types`), `src/export/stage.rs` (`part.mods`, the one-mod rule, validator, staging), `src/command/export.rs` (output fence for mod plugins), `src/export/resolve.rs`, `src/export/marketplace_export.rs` |
| CI | `.github/workflows/verify-deep.yml` (load probe job, `workflow_dispatch`), `test/manual/README.md` |
| Docs, catalog, config | ADR § Migration and rollout items 6 to 9, § Amendments |

## 10. Test strategy

### 10.1 Unit

- Kind enum: `ALL` completeness and injectivity, `Mod` last, round trips, `from_kind_str("mod")`.
- `mod.toml`: build mode refuses an unknown key and an unknown table; install mode ignores an unknown table and refuses an unknown key inside `[claude]`; the 64 KiB cap; `name` not the stem.
- `unknown_harness_table_and_directory_install_known_clients_with_one_warning`: a packed `mod.toml` with an `[amp]` table plus an `amp/` directory holding a non-code file installs claude and opencode, writes nothing for `amp`, and warns once.
- Default entry table: none, one, two candidates of different extensions, a candidate only in a subfolder, an OpenCode `.mjs` entry (refused), `x.d.ts` beside `x.ts` (different stems).
- Packed `mod.toml` golden, byte-identical across two builds.
- D3 hostile corpus, one case per rule: `..`, absolute, drive prefix, NUL, backslash, leading or trailing dot, each device-name form, a case-insensitive duplicate, a symlink entry, a hardlink entry, a non-code file, annotation and directory set disagreeing, lock `clients` disagreeing, a missing entry, an entry outside its harness dir. Each refuses before any byte is written.
- Binding gate: a committed key with uppercase, `@`, `/` or a newline; `nul` and `con.x`; the same names as a bundle member binding and as a `requires` edge name. Each refuses the row before a key or path is minted.
- Generated files: goldens for § 6.7 with `types` present and absent, plugins sorted, parsed back and compared; the OpenCode line parsed back for its one specifier.
- Consent: the `evaluate` table under v2 (absent, granted, drifted, foreign workspace, unknown `v`, truncated, unknown field); a hook entry and a mod entry with one binding and repository (no drift); union over gated kinds with one flag off; old-path delete only on a matching `workspace` field.
- `client_hosts` with a served set and with `plugin_api` unset, `"v2"` and `"v1"`; `plugin_api` under `[options.vendors.claude]` refused at load; `admits` rows for every Claude-family client, a row not serving `claude`, and Agent Plugins; marketplace names; the prefix rule over a file holding current, non-current and user keys; the settings validator refusing `grim-mods*`.

### 10.2 Acceptance (`test/tests/test_mod_*.py`, real zot registry)

- Install one artifact with both directories into a workspace with both clients and `plugin_api` unset: each code file byte-identical under `src/`, generated files and shared outputs equal to goldens, settings entries present.
- With `plugin_api = "v1"`: OpenCode declines with one warning and zero files.
- Build with no declared entry: the packed `mod.toml` names the default.
- Re-run `grim install`: `status` reads `installed`, nothing is written (not-modified proof, shared outputs included).
- A hand-edited `marketplace.json` refuses (65) and `--force` restores it; a foreign value at the grim marketplace entry refuses (65).
- Only one client present: the other declines with zero files and one warning.
- Nothing is written before consent; `--no-trust-workspace` disarms for the run; `--trust-workspace` writes with no record; after `grim install` no file exists under `$GRIM_HOME/consent/` (write-seam allowlist).
- The same through a bundle member and through a `requires` edge, with the first-write disclosure line.
- Revoke, then install: outputs and entries removed, row `gated`. Flag off, then install: the same. An edited `src/` file is kept, `index.ts` is gone, and `mod-files-present` shows. An edited `index.ts` is kept with the "keeps loading" warning.
- Disarm while another row fails, and with `--offline`: the mods are still removed.
- Crash between the write-ahead save and the final save (fault injection): the next disarm removes every file that matches a render of the pin.
- Named `grim update <skill>` after a revoke: unrelated written mods are disarmed and named on stderr; nothing else outside the closure changes.
- Uninstall removes payload, wrappers, both entries and the marketplace file with the last mod, and names `claude plugin marketplace remove`.
- A user `enabledPlugins` value of `false` stays across `grim install`, reads `mod-disabled-in-client`, and goes at `grim uninstall` with no refusal; a tracked output reads `mod-files-tracked`.
- Revoke, then `grim install --client claude`: the OpenCode copy is disarmed too.
- Hostile artifacts pushed by hand (one per D3 class): exit 65, `reason: invalid-mod`, nothing written.
- An untracked `.opencode/plugins/<b>/`: 65, forceable.
- Moved checkout: copy a workspace, run `grim install --force` in the copy; the copy's `settings.local.json` holds only its own `grim-mods-<h12>` keys.
- Global scope with `CLAUDE_CONFIG_DIR` and `OPENCODE_CONFIG_DIR` set outside the repository, and with neither; with either set inside the repository, the client reads `not-armed` (`settings-root-in-workspace`).
- Consent move: an old-path record is ignored and deleted by the next `grim consent allow`; every suite that hard-codes `hooks/consent` moves to the new path (`test_hook_consent.py` `_consent_dir` first; the list is in `research_harness_mods_open_questions_codebase.md` §2 and `research_mod_kind_architecture_map.md` § 1.2 item 3).
- Export: one mod staged with the install `hooks.json` bytes and `types`; two mods both omitted, exit 0; a mod plugin with `-o` inside `.claude` exits 64, and a skills-only plugin with the same `-o` exits 0 as before; MCP `grim_render` declines.

### 10.3 Real clients: probe and churn detector

Scripted from `research_mod_kind_load_paths.md`: a probe mod writes a marker on `session.start`. Claude runs as `claude -p hi --debug-file <f>` in an isolated `CLAUDE_CONFIG_DIR` with trust pre-recorded; OpenCode as `opencode run --standalone --print-logs --log-level debug -m none/none x`, which sends no model request. It runs locally on demand and as a `workflow_dispatch` job in `verify-deep.yml` against the latest `claude` and `@opencode/cli`; a `schedule:` trigger is deferred to the owner (ADR § Deferred). A red run is the churn signal; it gates no merge. It must answer the facts the research did not observe:

1. A name-only `plugin.json` loads.
2. An absolute path in `settings.local.json` loads in the first session.
3. Folder trust gates a registration in `settings.local.json`.
4. A hand-written user-scope registration loads in a global session.
5. Whether Claude keys a directory marketplace by its settings key or by the manifest `name` (a key-versus-name mismatch case).
6. Whether one marketplace name declared by two projects crosses over.
7. The OpenCode extensionless re-export of a `.js` entry.
8. Whether `types` is needed at runtime for a mod using `$.state`.
9. Where Claude's `/plugin` disable writes `enabledPlugins`.
10. Whether Copilot lists or loads the grim marketplace from `.claude/settings*.json`.

### 10.4 Old-binary goldens

- `test/tests/test_old_grim_mods.py` under `GRIM_OLD_BIN`, pinned 0.15.1, also run against 0.16.0, asserting every row of ADR D12, the to-be-run rows included.
- The mixed-consent scenario: an older grim allows, a newer grim writes v2, the older grim revokes, then a newer grim installs; the test pins that the newer grim still arms, which the docs state.
- A `test/data/golden/pre_mods_<sha>/` upgrade fixture: a project without mods keeps lock, state and config byte-identical.
- The one golden record, `test/data/golden/pre_settings_47da12f/golden/consent.project.json`, now asserts a re-ask, not a grant: `test_an_old_consent_record_still_arms_its_hook` and `test_an_old_consent_record_still_gates_a_grown_declaration` in `test/tests/test_golden_pre_settings.py` change.

### 10.5 Additive-keys gate

`test/tools/p9_diff.py` `ADDITIVE_STDOUT_KEYS["status"]` gains `"serves": lambda v: v is None`. Describe and search are outside the gate today; unit tests cover their `serves`. The install row's `armed` stays `null`, so it needs no entry.

## 11. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Claude changes `hooks.json`, `plugin.json` or directory marketplaces | Load probe, watchlist row, experimental flag, renderer goldens |
| Claude adds native scanning of `.claude/mods/` | Revisit trigger; Claude's one-plugin-per-name rule leaves one copy loading |
| A same-named plugin from another origin silences the grim copy | Docs line; advisory cause deferred |
| Copilot reads the grim marketplace from `.claude/settings*.json` | Probe item 10; watchlist row |
| An OpenCode plugin `id` shared by two mods | Authoring docs; grim does not parse code |
| Two `$GRIM_HOME` roots share one Claude root | Each removes the other's global entry; named in ADR D7 |
| The consent move drops grants stored by an older grim | CI is unaffected: it is built fresh, holds no record and passes `--trust-workspace`. A developer machine is asked once at its next interactive `grim install` or `grim update`; a persistent machine that runs grim non-interactively without the flag loses its grant at that run. `upgrading.md` says so first |

## 12. Implementation phases

0. The owner's prerequisite fix (ADR D15), as its own bugfix with its own plan, after one logged-in run confirms the finding. It refuses the affected content at build and publish and at install, and ships the obligations D15 lists.
1. Consent move and key change, with every hook and settings test moved to the new path.
2. Kind identity, descriptor, build, config, lock, `requires-features`.
3. Install: binding gate, validator, renderer, mod store with write-ahead and `shared`, Claude convergence, OpenCode with the host declaration, disarm, status.
4. Export, the output fence and the MCP decline.
5. Old-grim goldens, upgrade fixture, p9 gate, the load probe job.
6. One docs and records pass after the last code fix: docs, catalog, AI config, watchlist rows, the ADR amendments once accepted.

## 13. Open Questions

None open. The owner ruled consent option B and payload option P1 on 2026-11-18, and the prerequisite fix as option (b), "refuse, at publish and consume", on 2026-11-19. This design carries no option deltas; the considered alternatives stay in ADR D6, § Payload home and D15.

---

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-11-18 | Claude (architect) | Initial draft |
| 2026-11-18 | Claude (architect, fix pass) | Review round applied; flows, data shapes, security, tests and option deltas aligned with the ADR fix pass |
| 2026-11-18 | Claude (architect, fix pass 2) | Re-validation applied: export fence after admission for mod plugins only, dispatch-only probe, `enabledPlugins: false` kept and removed at uninstall, disarm ignores `--client`, resolved-scope host declaration, phase 0 under option (b), exclude lines as hygiene, settings store envelope unchanged |
| 2026-11-18 | Claude (architect, owner rulings) | Consent option B and payload P1 recorded as owner rulings; option forks and § 13 deltas for them removed; consent-move risk narrowed, CI unaffected |
| 2026-11-19 | Claude (architect, owner ruling) | Prerequisite fix ruled (ADR D15): phase 0 unconditional, § 13 has no open point, D1 to D15 |
