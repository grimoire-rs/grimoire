# ADR: The experimental `mod` artifact kind

## Metadata

**Status:** Accepted
**Date:** 2026-11-18
**Deciders:** Michael Herwig (owner, rulings in `.agents/discussions/harness-mods-artifact-kind.md` § Decisions) + Claude (architect, `/hex-architect` xhigh)
**Beads Issue:** N/A
**Related PRD:** N/A (ratified discussion above)
**Blast radius:** external contract: kind literal and position, layer layout, `mod.toml` grammar, the `com.grimoire.mod.clients` annotation, `grimoire.toml` / `grimoire.lock`, the consent record (path, key, version), status causes, report keys, files grim writes into Claude and OpenCode folders. Internal: install pipeline, consent policy, export staging.
**Reversibility:** per decision, and collected in D14. The wire format, the lock row, report keys and literals freeze at first release, flag or not. The config table, the flag, the consent record, the mod store and the installed layout stay Unstable while the flag exists.
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (Rust 2024, Tokio, `serde_json`, `toml`, no new dependency)
**Domain Tags:** security | api | integration
**Supersedes:** N/A
**Extends:** `adr_artifact_trust_model.md` (decision 2, third scoped exception), `adr_hook_workspace_consent.md` (record path, key, version), `adr_settings_artifact_kind.md` (key prefix withdrawn, a reserved marketplace prefix), `adr_vendor_support_tiers.md` (class 3 wording), `adr_artifact_dependencies.md` (`mods` feature), `adr_harness_plugin_export.md` (one admitted kind)

## Context

Users write Claude Code mods: a plugin whose `hooks/hooks.json` names one JS or TS module that runs inside Claude Code (2.1.287 and later). OpenCode V2 loads local plugin code the same way. Grim cannot ship either behind its consent mechanism today. The owner ratified a kind named `mod` with harness directories, a grim-generated wrapper, project installs into the client's project folder, one workspace consent record and Claude export in the first version. This ADR turns those rulings into contracts and records where the evidence strains them.

The owner also ruled that a prerequisite fix to a shipped kind lands first, as its own bugfix. It concerns a recorded, unpublished finding about client-loaded code, and lands after one logged-in run confirms that finding. The ruling was made in the discussion on a recommendation from the discussion's orchestrator, before its conflict with the Accepted trust ADR was known, so it went back to the owner as open question 3. On 2026-11-19 the owner ruled "refuse, at publish and consume": the fix refuses the affected content in that shipped kind when an artifact is built or published and when it is installed. The owner authorises the Principle 9 exception and the trust ADR amendment this needs (D15). The mod kind works without the fix; the fix closes the route the finding names.

### Premise corrections

| # | Dossier statement | Fact | Effect |
|---|---|---|---|
| 1 | Append last "because bundle members sort by that order" | Inserting anywhere keeps every existing kind's relative order. The order fixes where a mod sorts inside *new* bundle and `requires` layers, whose bytes are digested (`research_mod_kind_old_grim_behaviour.md` Q3) | D11 states the real reason |
| 2 | A plain source list "keeps the rule" that a new source is a new decision | `adr_hook_workspace_consent.md` decision 2 keys on the binding, and `src/hook/consent.rs:172-175` names the attack the binding stops: a second hook from a consented repository | D6: option B, owner ruling 2026-11-18 |
| 3 | The watchlist decline of themes and output styles is reopened | The reopen condition covers output styles only (`.agents/upstream-watchlist.md:31`); the row calls workflows class 3 for being executable JS | Amendment 7 changes the class-3 wording only |
| 4 | Claude needs no I1 exception | The registration can stay local, but the executed payload in `.claude/mods/` is repo-resident, which I1 names explicitly | D5: accepted with P1 (§ Payload home) |

## Decision Drivers

1. The owner rulings set the scope. This ADR prices them against the smallest version that meets the need and names the difference (§ Scope and its cost).
2. Principle 9: the wire format is frozen at first release; a project without mods stays byte-identical apart from `serves: null` on reports, and older grims fail closed.
3. T1 and T3 (`.claude/rules/arch-threat-model.md`): a mod is unsandboxed publisher code with client-level reach (`research_mod_kind_security.md` §1), and grim has no runtime gate over it.
4. One consent mechanism, no look-alike, no environment form.
5. A small generated surface: every byte grim writes into a vendor format is churn grim pays for.

### Scope and its cost

The smallest version that meets the stated need is Claude only, project and global scope, the existing consent record with a `mod:` key prefix, and no export. The ruled scope adds four things over it. OpenCode brings a full I1 exception and a host declaration (D4, D5). The consent move asks a developer machine once again per workspace (D6). Claude export adds a staging arm and an output fence (D13). `serves` on describe and search adds frozen report keys (D14). Each is in scope by ruling; none is needed by another.

## Industry Context & Research

**Research artifacts:** `research_mod_kind_architecture_map.md`, `research_mod_kind_security.md`, `research_mod_kind_old_grim_behaviour.md`, `research_mod_kind_load_paths.md`, `research_harness_mods_codebase_recon.md`, `research_harness_mods_prior_art.md`, `research_harness_mods_vendor_distribution.md`, `research_harness_mods_community.md`, `research_harness_mods_open_questions_codebase.md`, `research_harness_mods_claude_wrapper_facts.md`, `research_harness_mods_opencode_plugin_facts.md`, and a recorded, unpublished finding.

**Prior art.** No vendor or third-party manager makes a mod a package type; each rides inside the vendor's plugin unit (vendor distribution § Direct Answer). Agent Plugins 1.0 carries per-client payloads as one top-level directory per namespace, ignored by other clients: the same shape as harness directories (prior art §3). Microsoft APM ships a Copilot-only executable kind behind an experimental flag and an executables approval (vendor distribution §2). rulesync's generated OpenCode plugin broke on OpenCode V2 (vendor distribution § Direct Answer), the maintenance risk class 3 names. Consent comparables: direnv keys approval on path plus content; VS Code and Claude gate auto-loaded project code per folder; pnpm and APM keep approvals in the repository they govern, the repeated failure mode (`research_mod_kind_security.md` §5).

**Key insight:** grim can gate what it *writes*, never what a client *loads*. Every control below is honest about that line.

## Considered Options

### Central choice: how a mod reaches the client

Criteria and weights: **S** fewest I1 exceptions and least T3 exposure (4), **C** compatibility and one-way-door cost (4), **M** maintenance against upstream churn (3), **U** user value: one tool, lock-driven updates, files the user can see (2), **K** simplicity (1). Scores 0 to 3, maximum 42.

| Option | S | C | M | U | K | Total | Risk | Reversibility |
|---|---|---|---|---|---|---|---|---|
| O1: harness directories, generated wrapper, payload in the client's project folder, registration in a per-developer surface | 1 | 2 | 2 | 3 | 2 | 26 | Two I1 exceptions | Layout outside the 1.0 contract; a move needs migration, reaper and fixture |
| O1h: O1, but the Claude project payload under `$GRIM_HOME` (hook payload precedent) | 2 | 2 | 2 | 2 | 1 | 27 | One I1 exception (OpenCode) | As O1 |
| O2: every payload under `$GRIM_HOME`, absolute registration | 3 | 2 | 2 | 1 | 1 | 29 | OpenCode project scope needs a tracked `opencode.json` edit, so it is not served | As O1 |
| O3: the artifact carries complete client plugin folders, copied verbatim | 1 | 1 | 3 | 2 | 3 | 24 | A publisher manifest can enable components no kind gates and pin old clients out (wrapper facts §2); the plugin name cannot follow the consumer binding | Irreversible once published |
| O4: export only; users install through the client | 3 | 3 | 2 | 1 | 2 | 34 | No lock-driven install; no OpenCode route, since OpenCode has no export family | Fully reversible |

**Reading.** Export only scores highest on these criteria. The installed kind is the owner's product ruling, which removes O4. Among installed options O2 scores highest but cannot serve OpenCode at project scope, and O1h beats O1 by one point. The owner's ruling for the client's project folder picks O1, which this ADR is written for. The owner confirmed it on 2026-11-18 with these scores in hand (§ Payload home).

### Layout of one artifact

| Option | Consequence |
|---|---|
| **One layer, one directory per harness (owner ruling)** | One version, one consent entry, one name for both clients. A fix for one harness re-versions the other's code too |
| The settings ADR's ratified "one document, vendor objects, one layer" | Fits data merged key by key. A mod payload is a file tree of code, which is not a document value, so the choice does not transfer |
| One artifact per harness, joined by a bundle | Each payload versions on its own host's churn. Costs two artifacts, two consent entries and a bundle to name the pair; demand for cross-harness mods is thin (`research_harness_mods_community.md`) |

The ruling stands. Its cost is shared versioning, named here.

### Payload home (owner ruling 2026-11-18: P1)

Both clients install a project mod into their own project config folder: Claude into `.claude/mods/<b>/`, OpenCode into `.opencode/plugins/<b>/`. The owner's reason: OpenCode has to install into the project anyway, so two homes for one kind would be misaligned, and each harness's own project config folder is the consistent place. The owner's example name `.opencode/extensions` means the folder OpenCode V2 scans for plugins, not a new name, because OpenCode V2 loads only from its scanned plugin folders.

| Option | Claude project payload | OpenCode | I1 cost | What it buys |
|---|---|---|---|---|
| **P1 (owner ruling)** | `.claude/mods/<b>/` | project scope, `.opencode/plugins/<b>/` | Claude payload exception plus OpenCode full exception | One home per harness for one kind; mod code visible in the checkout. No sharing, because the Claude registration is local and absolute |
| P2 | `$GRIM_HOME/mods/payload/<workspace-key>/claude/<b>/`, the hook payload precedent | as P1 | OpenCode full exception only | No Claude exception, no checkout overwrite of a Claude mod, no Claude `mod-files-*` causes; two homes for one kind |
| P3 | P1 or P2 | global scope only | the Claude choice only | No OpenCode exception; per-project OpenCode use unserved |

This ADR's draft recommended P2; the owner ruled P1. The two recorded I1 exceptions (D5) are the accepted cost of that ruling.

## Decision Outcome

### D1. Wire and declaration surface

| Surface | Contract |
|---|---|
| Kind | `ArtifactKind::Mod`, appended after `Settings`; `ALL` length 8; wire token `mod`; `subdir()` `mods`; legacy `artifact_type` `application/vnd.grimoire.mod.v1` (read tier only); `is_dir_artifact` true. The owner chose `mod` over `plugin` and `extension`, which collide with grim vocabulary; `addon` was also free |
| Layer | One content layer, default tar media type `application/vnd.grimoire.artifact.layer.v1.tar`; `is_tar_kind` true, so `--compression gzip` applies; plus the optional `requires` layer. Caps: 16 MiB decoded, 2,000 files, depth 16, `mod.toml` at most 64 KiB before parsing |
| Annotations | `com.grimoire.kind=mod`; `org.opencontainers.image.title=<name>`; **`com.grimoire.mod.clients`**: the harness directory names present, sorted, comma-joined, each `[a-z0-9]+`. Required on every mod manifest |
| Config | `[mods]` table, entry forms as `[hooks]`; in the declaration hash only when non-empty |
| Lock | `[[mod]]` array, emitted only when non-empty; every row carries `clients = [...]` copied from the annotation at resolve |
| Flag | `[options.experimental] mods = true`, `mods_enabled()`; config key appended last in `config_keys.rs`. Project-settable, no env form. The flag is not the gate; consent is |
| Host declaration | `[options.vendors.opencode] plugin_api = "v2"`, read from the resolved scope's config only: that scope's `[options.vendors]` table is used whole, with no cross-scope merge (`adr_vendor_config_and_selection.md:216-219`). Config key `options.vendors.<name>.plugin_api`, appended beside `shared_skills` in `src/command/config_keys.rs`. `VendorOptions` is shared by every client, so `validate_vendors` (`src/config/project_config.rs:706`) refuses `plugin_api` at load for any client but `opencode`, as it refuses `shared_skills = true` on a client that does not read the pool. Closed value set, appended later. Older grims: D12 |
| Schema | `grim schema --kind mod` emits `grim-mod.schema.json`; the literal joins every `--kind` value parser, appended |
| Build | `grim build` infers `mod` from a `mod.toml` only after `SKILL.md` and `hook.toml` were tested and absent; `--kind mod` is accepted |
| Publish | `[mods.<name>]` in `publish.toml`, conventional source `mods/<name>/`, after settings and before bundles |
| Bundles, `requires` | Yes, both directions, through the existing two-layer layout |
| Path sources | Declare and lock, never written at any scope (`not-armed`, `not-registered`), as for settings. Authors test through a local registry; the authoring docs say so |

### D2. Descriptor and layer layout

```toml
# mods/status-band/mod.toml
schema = 1
name = "status-band"            # SkillName grammar, equals the directory stem
description = "Build status above the prompt"
[claude]
entry = "register.tsx"          # optional at authoring
[opencode]                      # optional; no table means the default entry
[requires]                      # optional, as hook.toml
```

- The layer is rooted at `<name>/` and holds `mod.toml` plus one directory per harness, `claude/` and `opencode/` in the first version.
- A harness directory name names one client API line. `claude/` is the Claude Code mods API. `opencode/` is the OpenCode V2 plugin API, forever; a later incompatible OpenCode line gets a new directory name, never a new meaning for `opencode/`.
- A harness directory holds code files only: regular files with an extension from `.js .mjs .cjs .jsx .ts .mts .cts .tsx`, the module set Claude documents. An OpenCode entry is limited to `.ts` and `.js`, the extensions observed or documented for a directory entry; helpers may use any code extension.
- A harness directory with no table is valid and takes the default entry: the one top-level file whose stem equals `name`. A stem is the file name without its final code extension. None or several such files fail the build (65).
- `grim build` writes the resolved `entry` of every served harness into the *packed* `mod.toml`, serialised by the `toml` crate from the parsed struct in a fixed key order, so a re-release yields the same bytes; a golden pins them. The author's file on disk is untouched.
- **Two parse modes.** Build denies unknown keys and unknown tables. Install reads the packed file with unknown top-level *tables* ignored as harnesses this grim does not know, denies unknown keys inside each known table, and denies unknown top-level keys. Install reads `entry` only from the packed file and never resolves a default.
- **Forward compatibility.** An unknown harness directory gets containment, the segment rule and the caps at install, is never written, and costs one warning. A later key inside a known table ships with its own `com.grimoire.requires-features` token on the mod, so a first-version grim refuses it with "update grim" (65) before parsing. No `api` or `min` key is added now. Unit test: `unknown_harness_table_and_directory_install_known_clients_with_one_warning`.

### D3. Install-time re-validation (T1 control)

Build-time checks bind honest tooling only. Before any write, install re-checks the fetched layer and refuses the row with 65, reason `invalid-mod`, naming the rule. Other rows still converge.

1. Only regular files and directories; every path passes `path_segment_refusal` and `contain`; in a known harness directory each segment matches `[A-Za-z0-9._-]+`, does not start or end with `.`, is not a Windows device name (`con`, `prn`, `aux`, `nul`, `com1` to `com9`, `lpt1` to `lpt9`, any case, alone or before a dot), and is unique case-insensitively in its directory. An unknown harness directory gets this rule and containment only.
2. The known harness directories present equal the known names in `com.grimoire.mod.clients`, and the lock row's `clients` equal the annotation.
3. Every served harness has an `entry` in the packed `mod.toml` that passes rule 1, names an existing code file in that directory, carries an allowed entry extension (D2), and, for OpenCode, has a stem no other code file in its directory shares.
4. Every file in a known harness directory carries a code extension; the D1 caps hold.
5. **The binding gate runs on every mod row**, whatever declared it: a `grimoire.toml` key, a bundle member or a `requires` edge. It applies `SkillName`, then the device-name refusal of rule 1, before any key or path is minted, as the hook gate does at `src/install/installer.rs:961` (`binding_name_refusal`). Separately, grim declines the Claude client for a binding Claude's validator refuses (the reserved prefixes and names in `research_harness_mods_claude_wrapper_facts.md` §1). Claude itself loads such a plugin; the decline is grim's policy, with a warning.
6. Every generated JSON file is a `serde_json::Value` serialised by `serde_json`. The strings inside it (`./<binding>`, `<binding>@<marketplace>`, `../src/<entry>`, the anchor path) and the OpenCode specifier are composed only from a binding that passed rule 5, an entry that passed rules 1 and 3, fixed literals and the resolved anchor root, and `serde_json` escapes them. The OpenCode index is a fixed template plus that one serialised string. No other text is written.

Author files land under a grim-owned `src/` directory in every rendered tree, so no author file can sit where a client reads a component or a generated file, by construction, whatever component directories a client adds later. The same validator runs before `grim export` stages a mod (D13).

### D4. Generated files and installed locations

Each mod becomes one Claude plugin and one OpenCode plugin directory; the binding names both. Per scope, the written mods also feed one grim-owned Claude marketplace file and one marketplace entry (D7).

| Scope | Client | Files | Registration |
|---|---|---|---|
| project | claude | `.claude/mods/<b>/src/**`, `.claude/mods/<b>/.claude-plugin/plugin.json`, `.claude/mods/<b>/hooks/hooks.json`, shared `.claude/mods/.claude-plugin/marketplace.json` (anchor `workspace`) | `.claude/settings.local.json` entries |
| global | claude | `<claude-root>/mods/<b>/...`, shared `<claude-root>/mods/.claude-plugin/marketplace.json` (anchor `claude-root`, follows `CLAUDE_CONFIG_DIR`) | `<claude-root>/settings.json` entries |
| project | opencode | `.opencode/plugins/<b>/index.ts`, `.opencode/plugins/<b>/src/**` (anchor `workspace`) | none: presence loads it |
| global | opencode | `<open-code-root>/plugins/<b>/...` (anchor `open-code-root`: `$OPENCODE_CONFIG_DIR`, else the XDG `opencode` dir) | none |

No new `PathAnchor`. Illustrative content, from the shapes that loaded in `research_mod_kind_load_paths.md` §1 to §4. The goldens the serializer produces are the byte contract; `serde_json::Value` sorts object keys (no `preserve_order`, `Cargo.toml:45`) and writes pretty with a trailing newline.

```text
.claude/mods/.claude-plugin/marketplace.json
{"name": "grim-mods-3fa9c2d1e0b7", "owner": {"name": "grim"}, "plugins": [{"name": "status-band", "source": "./status-band"}]}

.claude/mods/status-band/.claude-plugin/plugin.json
{"name": "status-band"}           (plus "types": "./src/types/index.d.ts" when that file exists)

.claude/mods/status-band/hooks/hooks.json
{"modules": ["../src/register.tsx"]}

.opencode/plugins/status-band/index.ts
export { default } from "./src/status-band"
```

- **Marketplace names.** Project scope: `grim-mods-<first 12 hex of workspace_key>`. Global scope: `grim-mods-<12 hex>` from a random id written once to `$GRIM_HOME/mods/marketplace-id`, so no repository can guess or declare the user's global name. Every name matches `^grim-mods-[0-9a-f]{12}$`. A per-workspace name keeps two checkouts from sharing one entry in Claude's user-level `known_marketplaces.json`. What Claude does with one name declared by two projects, and whether it keys a marketplace by its settings key or its manifest `name`, were not checked (§ 10.3 of the design probes both).
- **Manifest.** `plugin.json` carries `name`, the documented minimum, plus `types` exactly when the author tree holds `types/index.d.ts` (Claude before 2.1.299 needs it for `$.state`, wrapper facts §2). A name-only manifest passes `claude plugin validate` (wrapper facts §1); the runtime load was observed with `name`, `version` and `description` (load paths §1). Name-only at runtime is unverified. Excluded by the generated manifest, so unavailable to a mod: `userConfig` (a module's `register(on, options)` then receives no options), `dependencies`, `defaultEnabled` and `engines`. The authoring docs say so.
- **Module.** `hooks.json` names one module, `../src/<entry>`: one module per plugin is Claude's limit, and a module in a plugin subfolder with sibling imports was observed loading (load paths §2).
- **OpenCode.** A subfolder needs an `index` file, and a loose top-level `.ts` or `.js` file loads as its own plugin (load paths §3, T1 and T7), so grim writes only inside `<b>/`. The extensionless specifier was observed for a `.ts` entry; D2 limits entries to `.ts` and `.js` and § 10.3 probes `.js`. OpenCode V1 globs top-level files only (opencode facts Q1), and grim cannot tell V1 from V2 without running a client binary, which grim never does. So the OpenCode client hosts a mod only when the host declaration says `plugin_api = "v2"`; otherwise it declines with one warning naming the key and writes nothing. Kilo declines through `kind_support`.
- **Paths.** Claude reads a registered absolute path in the first session and a relative one only from the second (load paths §1, observed in `settings.json`). Grim writes absolute paths. The first-session load from `settings.local.json` with an absolute path is unverified.
- **Write order**, for live reload (OpenCode V2 watches plugin directories, opencode facts Q5) and session start: OpenCode writes `src/**` first and `index.ts` last, and on an update deletes `index.ts` first; a folder with no `index` is skipped silently (load paths §3, T7). Claude writes `src/**`, then `plugin.json`, then `hooks/hooks.json`, then the marketplace file, then the settings entries. Disarm removes the `enabledPlugins` entry first, then `hooks/hooks.json`, then the rest of the plugin; the marketplace entry and file go last, with the scope's last mod. Each file is written to a temporary name in its own directory and renamed.
- Generated files are rendered output under `adr_render_layout_stability.md`: re-materialising leaves `status` not-modified, and they are recorded outputs hashed like the payload.

### D5. Invariant I1, per client

- **Claude registration** stays inside I1: `.claude/settings.local.json` at project scope, the recorded per-developer surface. Without that entry the directory marketplace does not load (load paths §1, case G). Claude's folder trust still gates the load; that was observed for `settings.json` (case I), and for the local file it is unverified.
- **Claude payload** is a recorded exception. Every file under `.claude/mods/` is repo-resident, the shared `marketplace.json` included, which maps a grim-enabled name to a source. A branch that tracks those paths replaces them on checkout, and the local `enabledPlugins` entry then enables what the branch put there (T3, pull or fork PR). The lock pins what grim writes, not what the client loads. `grim status` reads `modified` once run. This exception is the accepted cost of the P1 ruling (§ Payload home); P2 would have removed it.
- **OpenCode project scope** is a full recorded exception: `.opencode/plugins/` loads on start with no prompt (opencode facts Q4). Consent decides whether grim writes; it never decides whether OpenCode loads. A committed grim output runs for every cloner. Accepted with the P1 ruling (§ Payload home).
- **Records.** A mod record is armable input, since it decides "unchanged, register". Every mod record lives in the mod store under `$GRIM_HOME/mods/`: `<workspace-key>.json` per project and `global.json` for global scope, envelope `{v, workspace, records, shared}`, dir `0700`, file `0600`, the settings store's load and save rules. `.grimoire/state.json` and `$GRIM_HOME/state/global.json` never hold the kind, and a mod record read from either is dropped with a warning. `validate_grim_home` runs before the store or the consent record is read.
- **Global roots are fenced.** A global payload root or registration file passes the settings kind's repository fence (`root_refusal`, `src/install/settings_phase.rs:1176-1194`) before it is read, written or removed: a relative root or one inside the work tree the command runs in is refused, `not-armed`, exit 0, with the existing causes `settings-root-relative` and `settings-root-in-workspace` (T3 through a repository-set `CLAUDE_CONFIG_DIR` or `OPENCODE_CONFIG_DIR`).
- **No `.git/info/exclude` lines for mod paths (hygiene, not a T3 control).** Git treats an ignored file as expendable and overwrites it on checkout without a refusal, while an untracked, non-ignored file makes the checkout refuse. An exclude line would turn that refusal into a silent replacement, so grim writes none for mod outputs. The refusal holds only while no ignore rule matches the path. The repository's own `.gitignore` is T3-controlled and commonly ignores `.claude/` paths, and then checkout replaces the files silently (I1: a gitignore rule is not part of the control). The existing line for `.claude/settings.local.json` stays. Evidence instead: `mod-files-tracked` when a written mod path is tracked, and `mod-files-present` when a mod grim did not write, or disarmed, still has files at its expected paths. Both are advisory and never move the row.

Grim never reports a mod as armed: the install row's `armed` stays `null`, and `installed` means written and registered, not observed running.

### D6. Consent: one record, one path, no kind

The record moves to `$GRIM_HOME/consent/<workspace-key>.json` with `"v": 2`. It keeps `deny_unknown_fields`, no defaults, the `workspace` field as identity, no environment form, and the three write seams (`grim consent allow`, `grim add`, an accepted prompt). The key drops every kind prefix. **Entry: option B, decided by the owner on 2026-11-18**: `<binding>@<registry>/<repository>`, with no kind prefix. A and C stay in the table below as the considered alternatives. Example:

```json
{"v": 2, "workspace": "/abs/path/to/checkout", "sources": ["fmt@ghcr.io/acme/hooks/fmt", "status-band@ghcr.io/acme/mods/status-band"], "consented_at": "2026-11-18T10:00:00Z"}
```

| Option | Entry | What arms with no question (each T1, bounded to artifacts that repository published) |
|---|---|---|
| A (plain source list, considered) | `<registry>/<repository>` | (1) a new kind from a consented repository; (2) a new bundle member from it; (3) a `requires` edge into it; (4) a new or changed binding of it, reachable by a fork-PR edit of `grimoire.toml` (T3) |
| **B (owner ruling: the smallest shape with no kind)** | `<binding>@<registry>/<repository>` | (1') an artifact from the consented repository bound under a consented binding in another gated kind. The binding is attacker-selectable: a publisher names bundle members and `requires` edges, and a fork PR names `grimoire.toml` keys (for example, moving `b` from `[hooks]` to `[mods]`). (2), (3) and (4) re-ask unless they reuse a consented binding |
| C (kind prefixes, today) | `mod:<binding>@...` | none. It contradicts the "no kind in the record" ruling; its benefit over B is that (1') is closed |

- The declared set is the union over gated kinds whose flag is on. Each kind checks its own declared entries against the one set. Mods take a per-binding verdict, as settings do.
- Every changing run prints one stderr line for each mod first written under an entry this run's gesture did not add, naming its kind and source. That line is the only evidence of the (1') widening (I5).
- The prompt lists the kinds present, names each entry's kind from the declaration, and marks entries whose kind's flag is off.
- Old records are never honoured as consent. On every write and every revoke, grim deletes `$GRIM_HOME/hooks/consent/<key>.json` when its `workspace` field equals the resolved workspace, so a revoke reaches an older grim (it then asks again). Revocation is one-way across versions: an older grim's revoke never reaches the new record (D12).
- Each linked worktree and each moved or copied checkout consents separately: the key is the raw path, on purpose (`adr_hook_workspace_consent.md` decision 1).
- `consent` stays in `RESERVED_ARTIFACT_NAMES` (append-only list).
- Mods follow whatever the owner rules for the Proposed consent blocks under `adr_hook_workspace_consent.md` decision 4 (RD-3, RD3-6, RD4-3), as hooks do. The TUI's consent site stays hooks-only, so a mod declared there is asked about at the next `grim install`, as settings are.
- **Rejected by the ruling, with its cost:** read a v1 record once, convert its entries (strip `settings:`), write v2 and delete the old file. It costs one import path in security-relevant code. Its benefit is narrow: it would spare each developer machine one re-ask per workspace, and keep the grant on a persistent machine that runs grim non-interactively without `--trust-workspace`. CI gains nothing, since a fresh CI environment holds no stored record.

### D7. Ownership of the shared Claude files and keys

- **Shared marketplace file.** Grim writes or deletes `.claude/mods/.claude-plugin/marketplace.json` (and its global twin) only while its bytes equal the SHA-256 grim last wrote, the rule of the hook own-file record (`$GRIM_HOME/hooks/owned_files/`, `subsystem-install-layout-hooks.md`). For mods that hash lives in the store's `shared` slot for the scope. A file whose bytes equal the current render is adopted. Any other file refuses with `untracked-destination` (65, forceable). It is written while at least one mod of the scope is written, and deleted with the last one.
- **Marketplace entry.** `/extraKnownMarketplaces/<current name>` is grim's when its value equals what grim would write for this scope, and it is recorded in `shared` with a value hash. A different value grim did not record refuses with `untracked-destination` (65, forceable).
- **Plugin entries.** `/enabledPlugins/<binding>@<current name>` is one entry output per mod record.
- **The prefix is grim's.** In the file grim manages for a scope, every `/extraKnownMarketplaces/` key matching `^grim-mods-[0-9a-f]{12}$` and every `/enabledPlugins/` key ending in `@` plus such a name belongs to grim. At each converging command grim removes those that are not the current name. That clears a registration a moved or copied checkout carried over with its `settings.local.json`, including its stale absolute path. Residual: two `$GRIM_HOME` roots sharing one Claude root remove each other's global entry; the last writer wins.
- **The settings kind** keeps its `/extraKnownMarketplaces/<name>` row and refuses, at build and install, a `<name>` that starts with `grim-mods`, so the two kinds never fight over one key. `enabledPlugins` stays a key no settings row reaches (`src/install/vendor.rs:2385`).
- **Why not settings pieces.** A settings piece belongs to a settings package's typed document and record. The mod entries are derived from mod records and must disappear on a mod's disarm, revoke or flag-off, which the settings phase does not see. Routing them through it would tie a mod's load to the settings flag and verdict. Both kinds share `json_splice` and the settings writer's re-read-before-write rule (75, `locked`, on a race).
- **A user's `false`.** A recorded `enabledPlugins` entry set to `false` is kept, status adds the advisory cause `mod-disabled-in-client`, exit 0; `--force` restores it. Disarm and `grim uninstall` remove the entry whatever its value, since the plugin it names is gone; a `false` value is not drift. Where Claude's `/plugin` disable writes was not checked.
- Grim never writes `known_marketplaces.json` or the plugin cache, Claude's internal state (`adr_render_layout_stability.md` §3). A removed grim marketplace stays listed there, one stale row per checkout and worktree, until `claude plugin marketplace remove <name>` (load paths §5, R3). `grim uninstall` names that command when the last mod of a scope leaves.

| Option | Consequence |
|---|---|
| **Mod kind owns grim-named entries and the shared file, by recorded hash** | Two kinds share a file, never a pointer; a moved checkout's leftovers are cleared |
| Settings kind owns both keys | A mod's registration would need a settings package; consent and lifecycle split |
| Shared ownership | Two record stores could each authorise removing the other's value (CWE-345) |

### D8. Lifecycle without a runtime gate

**Converging commands.** `grim install`, bare `grim update`, named `grim update <names>`, `grim add` (its install half), `grim uninstall`, and the TUI's Install, Update and Delete as their CLI forms. Each runs the disarm half over every mod of the scope, on every client that holds its outputs. `--client` narrows writes, never the disarm half or the prefix rule (D7), as for hook convergence. Removal needs no network, runs when another row failed or the run is offline, and runs before the command's exit is decided, as hook convergence does in `delete_nodes`. Writes stop at the command's reach. A named `grim update` and `grim add` write only their closure's mods, prune only the previous closure (`SettingsRun::Named`, `src/install/installer.rs:166-173`, is the precedent), and carry every other record, file, entry and marketplace member unchanged, except that a gated mod outside the closure is disarmed and named on stderr. `status`, `lock`, `remove`, `context`, `consent allow|revoke|list`, `hook list`, `export`, `fetch` and the MCP tools never converge mods.

| Event | Written files | Claude registration | Status |
|---|---|---|---|
| Consent present, flag on, OpenCode declared V2 | written | written | `installed` (written and registered) |
| Not consented or drifted, non-interactive | none for that binding | none | `gated` (`workspace-not-consented` / `consent-drifted`); a mod already written defers both causes and reads `installed` until the next converging command disarms it (`src/command/status.rs`, written-output check) |
| `--no-trust-workspace` on a converging command | disarmed for this run | removed | `gated`; the next consented run writes them again |
| `grim consent revoke` | unchanged; stderr names each written mod and `grim install` | unchanged | `installed` until the next converging command, which disarms it; then `gated`. Revoke deletes the whole consent record and the mod store records no grant source, so "no record" after a revoke is the same data as after a `--trust-workspace` run; telling them apart needs a new persisted field. Pinned by `test_mod_status.py::test_revoked_mod_reads_installed_until_the_next_install` |
| Next converging command after revoke, drift or flag off | disarmed | entries removed; marketplace file and entry with the last mod | `gated`; kept files read `mod-files-present` |
| An edited output at disarm | the edited file is kept, every unmodified output is deleted, `index.ts` first; the warning names `grim uninstall --force mod <name>` | entry removed, so the Claude copy is inert | `gated`; an edited OpenCode `index.ts` keeps loading, and the warning says so |
| `grim remove mod <name>` | unchanged; stderr names `grim uninstall mod <name>` | unchanged | pruned by the next converging command |
| `grim uninstall mod <name>` | removed now | removed now; stderr names `claude plugin marketplace remove` with the last mod | absent |

**Disarm keeps ownership.** A record keeps every output it may still own, with path and original hash, until removal succeeds; an output that could not be removed stays recorded, so `grim uninstall --force` finds it. Before writing, grim saves the record with the planned outputs (write-ahead), so a crash leaves paths disarm knows. At disarm a file is grim's when it equals the recorded hash or a render of a pin the record or the lock names, rendered from the content store (`src/install/adoption.rs` precedent); any other file is kept and warned.

**Silent-inert causes.** `installed` means written and registered. The client can still skip the mod with no error grim can see: Claude managed settings (`allowManagedModsOnly`, `strictKnownMarketplaces` or `blockedMarketplaces` naming the directory source, a managed `enabledPlugins` value), `disableAllHooks`, `--safe-mode`, an untrusted folder; OpenCode managed `plugin.use` deny or a `-` entry in its `plugins` array (wrapper facts §5, opencode facts Q4). `disableSideloadFlags` covers `--plugin-dir` only, not this route. The docs list them next to `installed`; status never reads client state.

Disarm is removal because no dispatcher stops a loaded mod; between grim commands the client controls it. This departs from settings, where flag off and revoke leave values, for that reason.

### D9. Supported clients are read from the artifact

`client_hosts`, the per-artifact seam that the install loop, expected outputs and `status` drift share, gains the row's `clients`: a client hosts a mod when its vendor supports the kind at that scope, the row lists it, and (OpenCode) the host declaration says V2. `kind_support(Mod)` is `Native` for `claude` and `opencode`, `Declined` elsewhere. A gap is a per-artifact skip: one warning, zero files. Status, describe and search read the set from the lock row or the annotation, never the layer. `serves` is `null` on a mod row whose source did not publish the annotation (an index the indexer has not updated). `family::admits` takes the set the same way.

| Option | Consequence |
|---|---|
| **Annotation plus lock field** | No layer fetch on any read path; install checks the layer against both |
| Layer only | `status` and browse would fetch content to answer |
| Lock only | Browse and describe could not answer before a lock exists |

### D10. Class 3 and generated registration files

Generated registration files are a kind's render, not a gap response. The amended class 3 (amendment 5) covers behaviour grim authors and keeps alive against another product's API. Publisher code an artifact carries, and the registration files grim renders so the client loads it (a manifest, a module list, a marketplace list, a one-statement re-export of the publisher's entry), are not class 3. They break the way every render breaks when a vendor format moves, and that break is silent: `claude plugin list` does not show a hand-registered mod (load paths §1), OpenCode logs a failed plugin only to its log file (load paths §3), and status says `installed`. The silent clause stays in the class-3 test, and its answer is a **churn detector**: the design's § 10.3 no-login load probe, added as a job in `.github/workflows/verify-deep.yml`, which today has only a `workflow_dispatch` trigger. It runs against the latest `claude` and `@opencode/cli`, with `-m none/none` so OpenCode sends no model request. A red run is the signal; it gates no merge. It catches a break only when someone dispatches it. A `schedule:` trigger adds recurring CI spend, so it is deferred to the owner (§ Deferred).

Rejected: declaring the kind class 3 (no Claude or OpenCode route exists without a generated file), and an OpenCode `package.json` with `exports` in place of `index.ts` (declarative, read from source, never run; it can change how `.js` files are read).

### D11. Kind order

`Mod` sorts last. The rule is "pick a position once and never move it": a published bundle or `requires` layer that lists a mod digests its position. Last is the cheapest position to defend, because `ALL`, `all_index`, schema enums, `_KIND_ORDER` in `test/src/helpers.py`, every value parser and the publish order then only grow at the end.

### D12. Older grims

The contract is the run table of `research_mod_kind_old_grim_behaviour.md` § Final table (real 0.15.1 and 0.16.0 binaries), copied verbatim, plus five rows this design adds. Every row was run against the real binaries and is pinned by `test/tests/test_old_grim_mods.py`; the rows that differ from the first draft say so.

| Situation | 0.15.1 | 0.16.0 |
|---|---|---|
| `[mods]` table in `grimoire.toml`, any command | 78, `unknown field \`mods\``, untouched | 78, same |
| `[options.experimental] mods = true`, no table | 78, `unknown field \`mods\`, expected \`hooks\`` | 78, `expected \`hooks\` or \`settings\`` |
| lock with `[[mod]]`: `status`, `install` | 78, unknown field `mod` | 78, "the lock cannot be read; restore it from git" |
| lock with `[[mod]]`: `lock`, `update`, `add` | **0, silently rewritten, `[[mod]]` dropped** | 78, lock untouched |
| `grim add <mod>` | 65, could not infer the kind | 65, same |
| `grim add <mod> --kind mod` | 64, invalid value | 64, invalid value |
| `grim fetch <mod>` | 1, not a Grimoire artifact | 1, same |
| bundle with a mod member | 65, unknown variant `mod`; nothing changed | 65, same |
| skill with `requires` edge to a mod, no annotation | 65 after declare and lock (T and L changed, 2-layer refusal) | 65, unknown variant `mod`; nothing changed |
| same, `requires-features = mods` on the skill | same as no annotation (ignored) | 65, "requires features this grim does not support (mods); update grim" |
| plain skill with `requires-features = mods` | 0, installed (ignored) | 65, update grim |
| unknown layer media type, known kind | read as tar | read as tar |
| consent record only at `$GRIM_HOME/consent/`, `hooks/consent/` absent | hooks skipped (fail closed); new dir untouched | hooks skipped; new dir untouched |
| both consent locations present | reads `hooks/consent/` only; new dir untouched | same |
| *added:* a bundle manifest carrying `requires-features = mods` itself | 0, installed; the annotation is ignored | 65 "requires features this grim does not support (mods); update grim"; nothing changed. Run: `test_a_bundle_manifest_carrying_the_mods_feature_itself` |
| *added:* `$GRIM_HOME/state/global.json` after a global mod install | unchanged and readable (`status --global` exits 0), since the file never holds a mod record (D5). Run: `test_global_state_file_stays_readable_by_an_older_grim` | same |
| *added:* older `grim hook revoke` after a newer grim wrote a v2 grant | finds nothing to remove, because the newer grim's `consent allow` moved the old record into `consent/`; the v2 grant stays and a newer grim arms on it (by design). Run: `test_an_old_revoke_leaves_a_newer_grims_v2_grant_armed` | same |
| *added:* `[options.vendors.opencode] plugin_api` in a project config | 78, `plugin_api` named; `VendorOptions` denies unknown keys. Run: `test_a_project_config_naming_plugin_api_is_refused` | 78, same |
| *added:* the same key in the global config | project-scope commands ignore the unusable global options and exit 0, silently; only `--global` commands exit 78. Run: `test_a_global_config_naming_plugin_api_breaks_the_global_scope_only` | project-scope commands exit 0 with a warning; `--global` commands exit 78 |

- **No tolerated location exists for the host declaration.** Every options table of both config files denies unknown keys in 0.15.1 and 0.16.0 (`src/config/declaration.rs` at both tags). In a project config the key costs nothing extra, because the `mods` flag beside it already makes those grims exit 78. In the global config it makes every `--global` command of an older grim exit 78, while project-scope commands ignore it (0.16.0 warns, 0.15.1 is silent), so `upgrading.md` says to set it there only once no older grim runs `--global` commands. Alternative: a file under `$GRIM_HOME` that old grims never read; rejected because it adds a second config surface for one key.
- `requires-features = mods` goes on every manifest whose bundle members or `requires` edges name a mod, a mod included, and `KNOWN_FEATURES` gains `mods`. It never goes on a manifest that only coexists with mods, which 0.16.0 would refuse (the "plain skill" row). A mod with no mod edge never carries it: a pre-mod grim refuses the kind first. A mod whose `[requires]` names another mod carries it under the first rule, so one rule fixes the digested bytes. A later mod feature may put its own token on a mod (D2).
- Residual, unrepairable: where `grimoire.toml` has no `[mods]` table, so a mod is reached only through a `requires` edge or a stale row, grim 0.15.1's `lock`, `update` and `add` drop every `[[mod]]` row with exit 0. The mod's files stay and keep loading until a newer grim converges.
- Revocation is one-way across versions (last row). The docs tell users to revoke with the grim that granted.

### D13. Export

- `admits(Family::Claude, claude, Mod)` is `Ok` when the row serves `claude`. A row that does not serve `claude`, and every other Claude-family client (`droid`, `junie`, `openclaw`, `qoder`), get `client-declined`; Agent Plugins gets `no-format-surface`.
- The D3 validator runs before staging. The exported tree reuses the install render: author files under `src/`, the same `hooks/hooks.json` bytes. The plugin's own `.claude-plugin/plugin.json` stays the export's `ClaudeManifest` (`src/export/family.rs:59-76`), which gains an optional `types` field, appended, set under the D4 condition.
- A plugin with two or more admitted mods carries none of them: each is omitted as `not-representable`, and the README and one stderr line say a Claude plugin loads one hooks module and name the fix (one mod per plugin). Exit 0, unless every member is omitted (`EmptyPlugin`, 65, existing). Grim authors no combining module.
- **Output fence, mod plugins only.** Export arms nothing on this machine as long as it writes outside a client's load paths. So an export that stages an admitted mod refuses an output directory inside any client config root it knows (`<claude-root>`, `<workspace>/.claude`, `<workspace>/.opencode`, the OpenCode config root). It exits 64 through the existing `ExportError::Usage` (`src/error.rs:538`), after member admission and before any write. Every export that stages no mod keeps its current behaviour and exit code: a plugin of skills exported into `.claude/` still exits 0. That scoping is a Principle 9 guard, because `stability.md:33` freezes export arguments and documented exit codes.
- MCP `grim_render` declines the kind, because rendering into a client load path would arm a mod with no consent (T4).

| Option for several mods | Consequence |
|---|---|
| **Omit every mod** | One stable rule; adding a second mod never swaps which one ships |
| Keep the first by binding | A renamed or added mod silently changes the published plugin |
| Refuse with 65 | Needs a new `ExportError` variant, which `c021_exit_codes_and_reasons_equal_main` pins |

### D14. Stability

**Frozen at first release, not exempted by the experimental flag:** the `mod` token and its position; the layer layout (root `<name>/`, `mod.toml`, the harness directory names and the API line each names); `mod.toml` `schema = 1` and `grim schema --kind mod`; the `com.grimoire.mod.clients` name and value grammar; the `mods` feature token; `--kind mod`; the `[[mod]]` row fields, `clients` included; and the literals added to otherwise-frozen reports: `serves` on every kind's `status`, `describe` and `search` items, the causes `mod-files-tracked`, `mod-files-present` and `mod-disabled-in-client`, the reason `invalid-mod`, the `mod` kind token wherever a report carries a kind, `armed: null` on mod rows, and the export output fence for a plugin that stages a mod (64, D13). These follow `stability.md` § Additive fields, like the settings literals (`stability.md:596-603`).

**Unstable while the flag exists:** the `[mods]` table of `grimoire.toml`, the `[mods]` table of a bundle source manifest and the `[mods.<name>]` entries of `publish.toml` (as the hooks entry of `stability.md` § Unstable lists theirs), the `[options.experimental] mods` key, the `[options.vendors.opencode] plugin_api` key, the consent record's path and format, the mod store format, the installed layout and generated file bytes (render rules), the marketplace naming scheme, and plain-text output. The naming scheme is sticky in practice, because Claude records a name grim never cleans.

### D15. Prerequisite fix to a shipped kind (owner ruling 2026-11-19)

**Ruling: "refuse, at publish and consume".** The fix refuses the affected content in one shipped kind at both ends: when an artifact is built or published, and when it is installed. Install refuses too, because a build-only check binds honest tooling only (D3's argument). The fix lands first, as its own bugfix, after one logged-in run confirms the finding (Migration and rollout item 1). Its design belongs to that bugfix's own plan, not to this ADR, which records the class only: a shipped kind stops accepting content that reaches client-loaded code.

**An owner-authorised breaking change under Principle 9.** Published artifacts that carry the affected content stop building and installing. The precedent is `adr_rule_support_dir_removal.md`. Unlike that precedent, which kept already-published packages installing with a warning, this ruling refuses at install too. The exception obliges the bugfix to ship:

- a named refusal reason, its literal and exit code chosen by the bugfix's plan;
- an `upgrading.md` entry and a changelog entry naming the refusal and what an affected publisher does instead;
- an upgrade fixture that pins the refusal for content an older grim accepted;
- the old-binary behaviour, stated in `upgrading.md` and `stability.md`: an older grim still builds and installs the affected content, and the fix cannot reach it;
- the trust ADR amendment (amendment 1).

| Option | Consequence |
|---|---|
| **(b) Refuse at build, publish and install (ruled)** | The route the finding names closes in new grims. Costs a Principle 9 exception and amendments to trust ADR decision 2, its two boxes and its Consequences line |
| (a) Drop the prerequisite (not chosen) | The shipped kind stays an ungated route to the same client capability. Mod consent is then a release-history gate, like the hook and settings exceptions, not a capability boundary, and the docs must not call it one |
| (c) Disclosure only (not chosen) | Grim warns at build and install and documents the route. No refusal, no amendment, no Principle 9 break; the route stays open but visible (I5) |

### Boundary and convention deviations, named

1. Two recorded I1 exceptions: the Claude payload and OpenCode project scope (D5).
2. The settings ADR's prefixed consent key is reversed; one binding from one repository is one entry across kinds (D6, option B).
3. Flag off and revoke remove files, where settings leave values (D8).
4. `HookConsentReport.hooks`, `HookPolicy` and `HookArmingCause` serve mods under their old names; `grim consent list` still lists hooks only, which the docs say.
5. The global fence reuses the causes `settings-root-relative` and `settings-root-in-workspace` for mod rows.
6. `consent` stays a reserved binding name though its directory moved (append-only list).
7. The mod packer refuses a symlink; the skill and hook walk drops one silently.
8. `grim remove mod` leaves a loading mod until the next converging command, because `remove`'s files-stay contract is frozen.
9. Mod records leave both `state.json` and `global.json`; the settings kind moved project records only.
10. The settings kind refuses marketplace names that start with `grim-mods`.
11. Mod outputs get no `.git/info/exclude` line, unlike `.claude/settings.local.json`.
12. One shipped kind refuses content it used to accept (D15), an owner-authorised Principle 9 exception.

## Component contracts (summary; full shapes in `.agents/specs/design_mod_artifact_kind.md` § 6)

| Contract | Testable statement |
|---|---|
| Status tokens | Reused: `installed`, `outdated`, `modified`, `missing`, `stale`, `gated`, `not-armed`; causes `feature-flag-off`, `workspace-not-consented`, `consent-drifted`, `insecure-transport` (a non-loopback plain-HTTP pin is never written, as for hooks), `grim-home-relative`, `grim-home-in-workspace`, `grim-home-shared`, `not-registered`, `settings-root-relative`, `settings-root-in-workspace`. New advisory causes, never moving the row: `mod-files-tracked`, `mod-files-present`, `mod-disabled-in-client` |
| Refusals | `invalid-mod` (65, data, not forceable, per row); `untracked-destination` and `modified` (65, forceable); a race on a settings file 75 `locked`; an export output inside a client config root, for a plugin that stages a mod only, 64 (`ExportError::Usage`). No new exit code |
| Reports | Always-present `serves` (client names for a mod, `null` for another kind or an unknown set) on `status` items, `describe` and `search` items; install `armed` is `null` on mod rows; `HookConsentReport.hooks` holds entries of every kind |
| CLI | `--kind mod` appended wherever `--kind` exists; `grim uninstall mod`, `grim remove mod`, `grim why mod`; no new subcommand (26 stays) |

## NFR coverage

| Quality | Coverage |
|---|---|
| Security | T1: default-off flag (I4), consent before any write, D3 re-validation including the binding gate, serializer-only JSON, digest pin, transport gate. T3: local Claude registration, records under `$GRIM_HOME`, the global root fence, the prefix rule for moved checkouts, recorded exceptions (D5); no exclude lines, as hygiene only. The prerequisite fix closes the route the recorded finding names, in new grims only (D15). Named residuals: no runtime gate; a version bump of a consented entry does not re-ask (T1, I5); the option-B widening (1'); OpenCode loads committed files with no grim |
| Compatibility and stability | D14. Older grims per D12; a project without mods is byte-identical apart from `serves: null` (upgrade fixture, p9 gate) |
| Operability | `grim status` is the inventory, since `claude plugin list` does not show a hand-registered mod; `serves`, three advisory causes, stderr lines on revoke, remove, uninstall and first write under an existing entry; the silent-inert causes are documented, not observed |
| Maintenance against upstream churn | Grim owns four small JSON shapes and one re-export line; the dispatched load probe (D10) and dated watchlist rows catch a break; publisher code breaking on a client release is the publisher's, as for skills. Claude's manifest and `hooks.json` shape held from 2.1.287 to 2.1.327 by docs and changelog (wrapper facts §3); only 2.1.327 was run |

## Consequences

**Positive:** one tool installs, updates, pins and removes mods for two harnesses; the wrapper cannot carry components another kind gates; consent is one record per workspace; status answers what `claude plugin list` cannot.

**Negative:**
- The record moves and its version changes, so a grant stored by an older grim is not honoured, hooks and settings included (a hook entry is otherwise unchanged). A developer machine is asked once again at its next interactive `grim install` or `grim update`. A persistent machine that runs grim non-interactively without `--trust-workspace` loses its grant at that run, and a guard hook such as the catalog's `command-guard` stays disarmed there until someone consents. CI is unaffected: a CI environment is built fresh, holds no stored record, and consents through `--trust-workspace` on the command line. `upgrading.md` says so first.
- Published artifacts of one shipped kind that carry the affected content stop building and installing (D15, an owner-authorised Principle 9 exception); an older grim still accepts them.
- Teams cannot share a Claude mod by committing it (local registration; § Deferred).
- A revoke takes effect at the next converging command.
- Two I1 exceptions are recorded (D5).
- OpenCode users set one host declaration before an OpenCode mod is written.

**Risks:**

| Risk | Mitigation |
|---|---|
| Claude or OpenCode change the wrapper contract | Load probe (D10), watchlist rows, experimental flag, renderer goldens |
| Copilot reads `.claude/settings*.json` `extraKnownMarketplaces` (`adr_settings_artifact_kind.md` decision 9); its handling of a mod plugin is unchecked | Watchlist row, revisit trigger |
| Name collision across origins: Claude loads one hooks module per plugin name across marketplaces, `--plugin-dir`, the user's other plugins and managed settings (https://code.claude.com/docs/en/plugins/loading.md, § Name conflicts, fetched 2026-11-18), so a same-named plugin silences the grim copy with only a `/plugin` note | Docs line; an advisory cause is deferred |
| Two mods with one OpenCode plugin `id`: one is dropped silently, first by scan order (load paths §3, T4) | Authoring docs; grim does not parse code |
| A moved or copied checkout | New workspace key: consent asks again, the store is empty, existing mod files are untracked destinations (65 unless `--force`, which adopts files equal to a render of the pin), and the prefix rule removes the old registration |

## Migration and rollout

1. **Order.** The prerequisite fix (D15) lands first, as its own bugfix with its own plan, after one logged-in run confirms the finding. It refuses the affected content at build and publish and at install. Then the consent move (D6) lands as its own commit, then the kind.
2. **Flag.** Ships off. With it off, `grim add` declares and locks, nothing is written, status reads `gated`.
3. **Consent move.** No import, by ruling. The old file is deleted per workspace on write or revoke. `stability.md` and `upgrading.md` state the one re-ask on a developer machine and the grant a persistent non-interactive machine loses without `--trust-workspace`. The one golden record, `test/data/golden/pre_settings_47da12f/golden/consent.project.json`, then asserts the re-ask: `test_an_old_consent_record_still_arms_its_hook` and `test_an_old_consent_record_still_gates_a_grown_declaration` in `test/tests/test_golden_pre_settings.py` change.
4. **Older grims.** `test/tests/test_old_grim_mods.py` under `GRIM_OLD_BIN`, pinned to 0.15.1 and run against 0.16.0, asserts every D12 row, the to-be-run rows included; a `pre_mods_<sha>` upgrade fixture covers a project without mods.
5. **Churn detector.** The load probe (D10) as a `workflow_dispatch` job in `verify-deep.yml`; a `schedule:` trigger only if the owner approves it (§ Deferred).
6. **Docs** (one pass after the last code fix): new `mods.md` reference with a page declaration, the silent-inert causes, excluded manifest features, the revoke-with-the-same-grim rule and the host declaration; `artifacts.md`, `concepts.md`, `commands.md`, `json-interface.md`, `publishing.md`, `configuration.md`, `clients.md` (matrix column in the same commit as the `Vendor` change), `hooks.md` and `commands.md` consent path, `settings.md` (reserved prefix), `stability.md` (D14, D12), `upgrading.md`, `guides/dependencies.md`, `changelog.md`.
7. **Catalog drift.** `grim-usage` (consume, troubleshooting), `grim-authoring` (`SKILL.md` consent path, new `references/mod-spec.md`), `catalog/hooks/README.md`; `task catalog:verify`.
8. **AI config.** New `.claude/rules/subsystem-install-layout-mods.md` with a catalog row and glob; `AGENTS.md`, `product-context.md`, `arch-principles.md`, `subsystem-install-layout-hooks.md` and `subsystem-cli-commands.md` lines (§ Amendments).
9. **Watchlist rows** (dated 2026-11-18): Claude directory-marketplace registration and wrapper (2.1.327); OpenCode V2 local plugin directory (2.3.2); Copilot reading `.claude/settings*.json` marketplaces (unchecked); Kilo V2 lineage (pre-release).
10. **Cross-repo.** Both consumers follow the settings-kind precedent: kind admitted, coloured, filterable, flag and consent prompts, and a per-harness view of the package.
    - **VS Code extension** (needs grim's JSON, so it lands after grim): admit the kind and its icon; read `serves` from `status`, `describe` and `search`; reuse the hook and settings flag and consent prompts for the `mods` flag; treat `gated` and the three advisory causes as it treats the hook ones. The details view reuses the harness chips the settings kind has since 0.4.0 (grimoire-vscode commit `3cfd2ea`): one chip per harness directory, showing that harness's entry and file list alone, plus a Raw chip for `mod.toml`. As there, nothing is keyed on a known harness name, so a mod from a newer grim with an unknown harness directory renders the same way (D2). Recorded grim output pins the contract, as for settings.
    - **Indexer** (needs only the annotation, so it can land before grim's install work): admit the `mod` kind in validation and marketplace members; copy `com.grimoire.mod.clients` into the package record; show the served harnesses as badges on the card and the package page; offer a harness filter beside the kind filter; give the package page the same per-harness switch, with the layer's file list grouped by harness directory. The indexer's settings page shows the whole document today (indexer commit `4675950`); its existing harness switcher covers install commands only. A record without the annotation shows no badges, matching `serves: null` (D9).
    - `serves` is present on every kind's report and `null` outside mods. A later release may fill it for settings from the document's harness tables, additively, so list and filter badges can serve both kinds.

## Amendments to existing records (applied on acceptance, not by this ADR)

1. `adr_artifact_trust_model.md`, after the settings box: add "AMENDED: decision 2 carries a third scoped exception, **mods**. The reason is release history again: the kind has never shipped. One record, one prompt, one answer; global scope is consented." The hook box's "it stores `<binding>@<registry>/<repository>` entries with no kind field" stays (option B). Decision 2 also gains "AMENDED (2026-11-19): one owner-authorised exception to equal trust. A shipped kind refuses content that reaches client-loaded code, at build and publish and at install (`adr_mod_artifact_kind.md` D15). It is a Principle 9 exception, as `adr_rule_support_dir_removal.md` was; client-loaded code ships as the consent-gated `mod` kind." (`:62`). Both boxes' statement that a gate on a shipped kind stays prohibited (`:85`, `:109`) gains "except the refusal of `adr_mod_artifact_kind.md` D15". The Consequences line rejecting "kind X is executable, therefore gate it" (`:168`) gains "D15 is an owner-authorised refusal, recorded as such, and no precedent for gating a kind because it is executable".
2. `adr_hook_workspace_consent.md` decision 1: "The record is `$GRIM_HOME/hooks/consent/<workspace-key>.json`" becomes "The record is `$GRIM_HOME/consent/<workspace-key>.json`". Its JSON example (`"v": 1`, `"hooks": [...]`) becomes the D6 example. "`consent` joins `RESERVED_ARTIFACT_NAMES`, because a hook bound to a name grim writes under `$GRIM_HOME/hooks/` lands a directory on that path." gains "It stays reserved after the record moved, because the list is append-only." Decision 2, its first two sentences from "The consented set is `<binding>@<registry>/<repository>`" through "or one from a new repository is **drift**", become: "The consented set is `<binding>@<registry>/<repository>` for every gated kind, with no kind prefix, no tag, no digest and no tier. A new binding, a rebinding, or one from a new repository is drift; the same binding from the same repository under another gated kind is not."
3. `adr_settings_artifact_kind.md` decision 7: "**Key.** `settings:<binding>@<registry>/<repository>`, built by `consent_key_for(kind, binding, source)`." becomes "**Key.** The unprefixed entry of `adr_mod_artifact_kind.md` D6, shared by every gated kind." Appendix A's `/extraKnownMarketplaces/<name>` row gains "a `<name>` that starts with `grim-mods` is refused at build and install; the mod kind owns that namespace".
4. `.claude/rules/arch-threat-model.md` I1: after the settings box add "**Mods carry two recorded exceptions.** A Claude mod's payload is repo-resident while its registration stays in `.claude/settings.local.json`; an OpenCode project mod is repo-resident and loads on presence. Consent decides what grim writes, never what the client loads (T3). Mod records live under `$GRIM_HOME/mods/`, and global mod roots take the settings root fence." In the I2 box, "`$GRIM_HOME/hooks/consent/<workspace-key>.json`" becomes "`$GRIM_HOME/consent/<workspace-key>.json`", and "Its bindings sit in the same consent record under a `settings:` prefix," becomes "Its bindings sit in the same consent record as unprefixed entries (`adr_mod_artifact_kind.md` D6),". In Applying it, "as extended to the `settings` kind" becomes "as extended to the `settings` and `mod` kinds, and the mod exceptions under I1". In What grim is, append "As of the mod kind it also writes publisher code a client loads into its own process."
5. `adr_vendor_support_tiers.md` (Proposed), class 3 row: "Anything grim must keep alive against someone else's API: plugin, extension, wrapper process, injected script." becomes "Behaviour grim authors and must keep alive against someone else's API: plugin, extension, wrapper process, injected script. Publisher code an artifact carries, and the registration files grim renders so a client loads it, are a kind's render, not a gap response; a load probe checks them for silent breaks (`adr_mod_artifact_kind.md` D10)." The test sentence "The test is whether a client release can break grim's contribution without grim being touched, and whether that break is silent." stays. Section 3: "specifically that grim will never install runtime code into their client" becomes "specifically that grim will never add runtime code of its own to their client".
6. `docs/src/content/docs/clients.md` compensation bullet: "**grim will never install a plugin, extension, or any other runtime code into your client.**" becomes "**grim will never add runtime code of its own to your client to close a gap.** A hook or a mod is written only after you consent to the workspace." The bullet claims no more than that: no audit checked that D15 closes every route by which package content reaches client-loaded code, so a general sentence would be unverified. The workflows bullet's "grim never installs code into a client" becomes "grim adds no code of its own to a client; a mod is the kind built to carry client-loaded code".
7. `.agents/upstream-watchlist.md:31` last cell: "workflows are class 3" becomes "workflows stay declined as a component kind; client-loaded code ships only as the `mod` kind (`adr_mod_artifact_kind.md`)".
8. `docs/src/content/docs/stability.md:25`: "Three exceptions sit inside these rows" becomes "Four exceptions", naming the mod surface; the Unstable list gains the D14 Unstable block, the not-exempted list gains the D14 frozen block, and the old-grim section gains the D12 table.
9. `adr_artifact_dependencies.md` Decision 2: `KNOWN_FEATURES = ["dependencies"]` gains `mods`, with the writer rule of D12.
10. `adr_harness_plugin_export.md`, member admission: add "A mod is admitted for `claude` when the artifact serves it; other Claude-family clients `client-declined`; Agent Plugins `no-format-surface`; a plugin with two or more mods carries none of them (`not-representable`); an export that stages a mod refuses an output inside a client config root (64), and every other export is unchanged."
11. `AGENTS.md` environment table: "`$GRIM_HOME/hooks/consent/<workspace-key>.json`, one record for both kinds: settings consent sits in it as `settings:<binding>@<registry>/<repository>` keys beside the hook keys" becomes "`$GRIM_HOME/consent/<workspace-key>.json`, one record for every gated kind, as unprefixed entries"; the `GRIM_HOME` row's "the per-workspace consent records under `consent/`" moves out of the `$GRIM_HOME/hooks/` list into its own `$GRIM_HOME/consent/` clause, and the row gains the mod store `$GRIM_HOME/mods/`; the "no variable" row names mods beside hooks and settings.
12. `.claude/rules/arch-principles.md:96`, `.claude/rules/subsystem-install-layout-hooks.md:105`, `.claude/rules/subsystem-cli-commands.md:47` and `catalog/skills/grim-authoring/SKILL.md:164`: every `$GRIM_HOME/hooks/consent/` becomes `$GRIM_HOME/consent/`; the hooks layout row's record shape `{v, workspace, hooks, consented_at}` becomes `{v, workspace, sources, consented_at}` and moves to the new mods layout rule. In `subsystem-cli-commands.md:47`, "`allow` also records the declared settings packages as `settings:<binding>@<registry>/<repository>` in the same array" becomes "`allow` also records the declared settings and mod packages as unprefixed entries in the same array". In `.claude/rules/subsystem-install-layout-settings.md:104`, "holds `settings:<binding>@<registry>/<repository>`" becomes "holds unprefixed entries (`adr_mod_artifact_kind.md` D6)".

## Revisit triggers

- Claude changes `plugin.json`, `hooks.json` or directory-marketplace loading, scans `.claude/mods/` natively, or documents a local plugin location that needs no settings entry. The load probe goes red.
- OpenCode V2 adds a trust gate for project plugins (narrow D5), a verified index-free entry (drop `index.ts`), or a way to tell V1 from V2 without running it (drop the host declaration).
- Kilo's V2 lineage reaches `latest`; a third harness is requested.
- Copilot is shown to list or load the grim marketplace from `.claude/settings*.json`.
- Signatures arrive (`adr_artifact_trust_model.md` § When signatures arrive).
- 0.17.0 ships or `KNOWN_FEATURES` changes: re-run the old-grim table.

## Deferred

- A committed Claude registration in `.claude/settings.json` for team rollout by commit. Out of scope for the first version; `grim export marketplace` covers sharing a mod as a Claude plugin, and a committed route can be added without a format change.
- A `schedule:` trigger for the load probe in `verify-deep.yml`. No workflow has one today, and it adds recurring CI spend, so it is an owner decision.
- Kind and flag state per entry in `HookConsentReport` (the prompt already shows them).
- Keeping a user's `enabledPlugins: false` across a revoke and re-consent; today the re-consented run writes `true`.
- An advisory cause for a same-named plugin from another origin.
- Reading client logs or client state for status.
- OpenCode TUI and GUI plugin parts; only the server entry is served.
- Garbage collection of stores and consent records for deleted workspaces (inert, as consent records already are).

## Open questions

All three are answered by the owner; none remains open.

1. Consent entry, 2026-11-18: option B (D6).
2. Payload home, 2026-11-18: P1, both clients in their own project folder (§ Payload home).
3. Prerequisite fix, 2026-11-19: option (b), "refuse, at publish and consume", with the Principle 9 exception and trust ADR amendment authorised (D15).

## Links

- `.agents/discussions/harness-mods-artifact-kind.md` (rulings), `.agents/specs/design_mod_artifact_kind.md` (design)
- `adr_artifact_trust_model.md`, `adr_hook_workspace_consent.md`, `adr_settings_artifact_kind.md`, `adr_vendor_support_tiers.md`, `adr_render_layout_stability.md`, `adr_artifact_dependencies.md`, `adr_harness_plugin_export.md`, `adr_harness_marketplace_export.md`, `adr_rule_support_dir_removal.md`, `adr_hook_native_dispatcher.md` (no dispatcher here, so its launcher contracts do not apply)
- `.claude/rules/arch-threat-model.md`, `docs/src/content/docs/stability.md`, `docs/src/content/docs/clients.md`

---

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-11-18 | Claude (architect, `/hex-architect` xhigh) | Initial draft, **Proposed**. |
| 2026-11-18 | Claude (architect, fix pass) | Review round applied: forward-compatible parse, shared-file ownership, disarm ownership, moved checkouts, global fence, binding gate, export `types` and output fence, named-update reach, D14 stability, prerequisite as open question 3, matrix without the rulings criterion, layout table, consent costs and widenings, verbatim old-grim table, OpenCode host declaration, churn detector. |
| 2026-11-18 | Claude (architect, fix pass 2) | Re-validation applied: export fence scoped to mod plugins, dispatch-only probe, `enabledPlugins: false` at uninstall, disarm ignores `--client`, host declaration per resolved scope with D12 rows, prerequisite order under option (b), amendment 6 scoped to hooks and mods, exclude lines as hygiene, eleven wording fixes. |
| 2026-11-18 | Claude (architect, owner rulings) | Owner ruled consent option B and payload P1 (both clients in their own project folder); questions 1 and 2 closed, § Payload home added; consent-move cost narrowed to developer and persistent non-interactive machines, CI unaffected. |
| 2026-11-19 | Claude (architect, owner ruling) | Owner ruled question 3 as option (b), "refuse, at publish and consume": new D15 with the Principle 9 exception, its obligations and the not-chosen options; Context, rollout 1, deviation 12, NFR, Consequences and amendments 1 and 6 updated; no question open. Rollout item 10 (cross-repo) was expanded by the orchestrator on 2026-11-19 and is unreviewed by the architect. |
| 2026-11-19 | Claude (records pass) | **Accepted**, with its implementation, on the owner's goal-loop start of 2026-11-19. Amendments 1 to 12 applied: 1 `adr_artifact_trust_model.md`, 2 `adr_hook_workspace_consent.md`, 3 `adr_settings_artifact_kind.md`, 4 `.claude/rules/arch-threat-model.md`, 5 `adr_vendor_support_tiers.md`, 6 `docs/src/content/docs/clients.md`, 7 `.agents/upstream-watchlist.md`, 8 `docs/src/content/docs/stability.md`, 9 `adr_artifact_dependencies.md`, 10 `adr_harness_plugin_export.md`, 11 `AGENTS.md`, 12 the four rule and catalog files it names. D12 table: every row run, the global-config `plugin_api` row corrected (it breaks `--global` commands only). D8 event table: the revoke row reads `installed` until the next converging command. New § Implementation (mod kind). |

## Implementation (D15 fix)

The prerequisite fix landed first, as its own bugfix. This block records the rulings its plan took where D15 left the mechanism open. Class only: it names what is refused, not how a client treats the content.

| Decision | Ruling |
|---|---|
| Refused set | A skill whose root carries a Claude plugin manifest entry, `.claude-plugin`, of any entry type. The name is folded before the comparison: cut at the first `:`, trailing dots and spaces removed, Unicode uppercase. Root only; the same name below the root is allowed. |
| Build seam | The packed, post-ignore entry list, checked in `pack_skill_dir` and in a `grim publish` pre-pass that runs before any push. An entry `.grimignore` excludes is not refused. |
| Install seam | The staged directory after `materialize`, read back as the filesystem stored it (`stage_locked_artifact_within`), and the same check in `grim_render` and in export staging. |
| Installed by an older grim | On the `AlreadyInstalled` path, the record's output paths (else the output root) are checked. A manifest name fails the run with 65; files, install record and lock stay. Named residuals in `upgrading.md` and `stability.md`: the files remain until `grim update` or `grim uninstall`, and `status` still reads `installed`. |
| Reason | New slug `skill-plugin-manifest`, exit 65, `data`, not forceable, not retryable, appended at index 10 of the frozen reason list. |

Open questions answered, taking the recommendations: the refusal covers the root only, because a deeper refusal would also refuse skills that ship plugin templates and widening later is a second breaking change; and grim keeps the files it wrote for an already-installed skill and refuses every install-type run, because the ruling grants a refusal, not a deletion, and an older grim would restore removed files.

Obligations met: the upgrade note and changelog entry, the stability promise `{#promise-skill-plugin-manifest}` with the old-binary behaviour, the upgrade fixture (a real older-grim acceptance module), and the trust ADR amendment (`adr_artifact_trust_model.md`).

D15 reach, found after the first fix landed (class only). The refusal of a skill whose root carries `.claude-plugin` also fires for an installed record whose output sits under a client outside `--client`, because every recorded output is checked. `grim install` and `grim update` also refuse (exit 65, reason `skill-plugin-manifest`) while a dev install (`grim install ./path`) carries the refused entry; `grim status` keeps exit 0 with its stderr line, and `grim uninstall skill <name>` succeeds. Evidence: `installer::refuse_installed_dev_plugin_manifest`, `test_skill_plugin_manifest.py::test_installed_dev_skill_with_entry_refuses_install_and_update`. The remedy for a refused skill is the same: ship client-loaded code as a `mod`. Residual: the check sits in `install` and `update`, so a dev install of a different directory (`grim install ./<other path>`), `grim add` and a TUI action on another row are not blocked by a stale dev record elsewhere. The registry-sourced refusal and this one run after the consent prompt, so an accepted prompt is recorded before the 65; the record holds the user's answer, which stays valid. The TUI's Install reaches the installer's refusal (unit `tui_install_of_a_skill_with_a_plugin_manifest_is_refused`).

## Implementation (mod kind)

Rulings the mod kind's implementation took where this ADR or the design left the mechanism open, or where the code differs from them. Each is checked against the code at acceptance.

| Decision | Ruling |
|---|---|
| Revoke reads `installed` until the next converging command | D8's event table said `gated` at once. The code defers the two consent causes (`workspace-not-consented`, `consent-drifted`) to a written output (`src/command/status.rs:3253`), because a `--trust-workspace` run leaves no consent record and its written rows would otherwise read `gated`. `grim consent revoke` deletes the whole record and the mod store records no grant source, so the two cases are the same data; telling them apart needs a new persisted field. Hooks read the same way today. Kept. Pinned by `test_mod_status.py::test_revoked_mod_reads_installed_until_the_next_install` |
| `adoption.rs` `provable(Mod)` is not added | `provable` (`src/install/adoption.rs:388`) admits a kind to a proof that reads a `state.json` record and stages the node through the kind's materializer; a mod has neither. Disarm proves a file is grim's by the recorded hash, or a render of the record's pin or the lock's pin from the content store (`render_proofs`, `src/install/mod_phase.rs:1103`), computed before `delete_nodes` and passed in. `DeleteEnv` gained `locked_mods` (`src/command/uninstall.rs:596`, `src/tui/app.rs`). A moved or copied checkout's existing mod files are untracked destinations: 65 unless `--force`, which adopts files equal to a render of the pin. Pinned by `test_mod_lifecycle.py::test_moved_checkout_keeps_only_its_own_marketplace_key` |
| A hand-edited shared Claude file reads `modified` | Status hashes each shared slot read-only: the marketplace file by its bytes, the `extraKnownMarketplaces` entry by its value (`shared_output_drifted`, `src/command/status.rs:3189`). A changed, missing or unparseable file or entry is drift and moves every installed Claude mod row of the scope to `modified`, since both slots list every Claude mod of the scope and the store keeps no per-mod slot. A gate, `stale` and `missing` keep precedence. No new JSON key. Not covered at global scope (same code path). Pinned by `test_mod_status.py::test_hand_edited_marketplace_file_reads_modified` and `::test_hand_edited_marketplace_entry_reads_modified` |
| A user's `enabledPlugins` `false` is not drift | The store records no `enabledPlugins` slot, so the value is no one's to prove. It reads `mod-disabled-in-client`. Pinned by `test_mod_status.py::test_false_entry_reads_mod_disabled_in_client` and `::test_disabled_plugin_value_is_not_shared_drift` |
| A mod with files but no write reads `mod-files-present` | Only reachable past the consent gate. Pinned by `test_mod_status.py::test_unwritten_mod_with_files_reads_mod_files_present` |
| A refused shared output lands on the run's first Claude mod | The marketplace file and the settings entry are shared by every Claude mod of the scope, so one refusal (an untracked destination) has no single owner. It is reported on the first Claude row (`src/install/mod_phase.rs:1525`); with no Claude row it is a warning. Retiring fails the same way |
| MCP tools do not serve mods | `grim_fetch` answers "mods cannot be fetched yet" (`src/fetch.rs:591`) and `grim_render` "mods render no files; use grim install" (`src/mcp/render.rs:134`). No MCP tool converges mods. Pinned by `test_mod_lifecycle.py::test_non_converging_commands_leave_mod_files_unchanged` |
| First-write disclosure counts the pre-gesture record | D6's stderr line names a mod first written under an entry this run's gesture did not add. `ConsentPolicies::note_added` (`src/hook/policy.rs:614`) takes the record as it stood before the gesture and keeps only the keys it lacked, so an entry recorded before the run is disclosed. Pinned by `test_mod_consent.py::test_add_under_an_entry_recorded_before_the_run_is_disclosed` |
| Every built mod installs | The valid mod directories the build test packs are each released, locked and installed with exit 0, with the entry written for every served client. Pinned by `test_mod_install.py::test_every_built_mod_passes_install_validation` |
| Export validator refusal | A mod layer the validator refuses during export is mapped through `ModError`, so it exits 65 with reason `invalid-mod` (`src/export/stage.rs:1469`); an earlier draft exited 65 without the reason. Recorded so the divergence is not rediscovered |
| First-version file list | `test/data/golden/pre_settings_47da12f/tools/generate.py` is unchanged, because it drives the old `47da12f` binary, which writes the v1 consent path; moving it would break the generator. `test_old_grim_settings.py` is unchanged for the same reason |
| Client load probe | The kind is experimental, behind `[options.experimental] mods` (off by default). The probe (`test/tests/test_mod_load_probe.py`, CI job `mod-load-probe` in `.github/workflows/verify-deep.yml`) is opt-in, runs on `workflow_dispatch` only and gates no merge, as D10 and Deferred say |

