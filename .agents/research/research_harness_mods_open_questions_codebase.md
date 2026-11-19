# Research: Harness extension artifact kind, open design questions (codebase)

## Metadata

**Date:** 2026-11-18
**Domain:** cli
**Triggered by:** discussion on a new artifact kind carrying native harness extension code (Claude "mod" wrapped in a generated plugin, OpenCode plugin file; one layer, one directory per harness, covered by the existing workspace consent record). Companion to `.agents/research/research_harness_mods_codebase_recon.md`.
**Expires:** 2027-02-18
**Lane:** codebase recon. Findings, per-option facts, `negative:` and `leads:`. No recommendation.

## Direct Answer

Seven questions, answered by section. Decisive facts: (1) a consent key is `binding@registry/repository` with no tag or digest, and `split_by_kind` today sends every non-`settings:` key to the hook half, so a third prefix needs that function generalised; (2) only grim itself touches the consent path (not the `grim-hook` launcher), and `grim hook revoke` deletes one file, which a permanent read-fallback would undo; (3) `[[plugin]]`, `[plugin]`, `[plugins.*]`, `--plugin` and `grim export plugin` are already taken, and "extension" is a documented term with a frozen anchor; (4) `admits()` is an exhaustive match, so a new kind cannot compile without an arm, and the exporter has no `hooks/hooks.json` writer; (5) hook and settings names come from the artifact (manifest `name` equals directory stem; settings file stem) while installed paths use the consumer binding; (6) `node_modules/` is a default ignore but `!node_modules/` in the root `.grimignore` re-includes it, and the shared walker silently skips symlinks; (7) there is no cross-artifact collision detector at render time, only the `untracked-destination` refusal.

## 1. Consent record keys

**Definition and writer.** `src/hook/consent.rs` (module doc `:4-69`, struct `:109-122`, `load` `:258`, `record` `:320`, `revoke` `:358`). Path helpers in `src/install/hook_dispatch.rs:191-217` (`consent_dir`, `consent_path`, `ensure_consent_dir`).

**On-disk shape** (`src/hook/consent.rs:109-122`, written by `serde_json::to_vec_pretty`, `:336`):

```json
{ "v": 1, "workspace": "<abs path>", "hooks": ["fmt@ghcr.io/acme/fmt", "settings:team@ghcr.io/acme/team"], "consented_at": "2026-08-28T10:00:00Z" }
```

`deny_unknown_fields` and no field defaults (`:104-110`), so a truncated file cannot deserialize into a grant. One file per workspace, filename `workspace_key(workspace).json`; the `workspace` field is the identity, filename only an index (`:46-48`, `:240-248`, byte comparison).

**Version handling.** `ConsentVersion` is a `serde_repr` `#[repr(u8)]` enum with one variant `V1 = 1` (`:95-100`). An unknown version fails deserialization, and every failure (absent, I/O, malformed, unknown field, unknown version) returns `None` at debug level, never a warning (`:252-275`). Unusable equals absent equals not consented.

**Key format.**
- Hook: `<binding>@<registry>/<repository>` (`consent_key`, `:176-179`). `None` for a source with no registry pin (path source).
- Settings: `settings:<binding>@<registry>/<repository>` (`SETTINGS_KEY_PREFIX` `:198`, `consent_key_for` `:203-214`, `declared_consent_key_for` `:184-194`). A hook binding that starts with `settings:` gets no key at all (`:187`, `:207`), so a repository cannot forge a settings key through a hook binding.
- Other kinds (skill, rule, agent, bundle, mcp) map to `None`; both `match` arms are exhaustive, so a new kind will not compile until it is added.

**What identifies an artifact in a key.** Binding name, registry, repository. Not tag, not digest, not version (`:164-166`, `:172-175`: the binding is part of the key so a second hook from an already-consented repository drifts).

**Version bump or content change.** Neither drifts. `evaluate` (`:240-250`) returns `Drifted(declared minus recorded)` only for a key not in the record. The module doc states the residual as T1 and I5, "evidence, not prevention" (`:58-69`): a hostile upstream owning a consented repository can publish a new version without a re-prompt; the digest pin in `grimoire.lock` is the control, and adding the digest to the key "would also re-prompt on every routine version bump". User docs say the same (`docs/src/content/docs/hooks.md:87-92`). `consent-drifted` is a status cause (`src/api/artifact_status.rs:157,484`), `gated` state, from a new binding or repository only (`.claude/rules/subsystem-cli-commands.md`, `grim status` row).

**Additive third kind, what the code needs.**
- Prefix: hook bindings parse as `SkillName` (`[a-z0-9]+([.-][a-z0-9]+)*`, `.claude/rules/subsystem-install-layout-hooks.md`), so no hook key can contain `:`; any `<prefix>:` is unambiguous against hook keys.
- `split_by_kind` (`src/hook/consent.rs:219-221`) is `partition(|key| !is_settings_key(key))`. A key with an unknown prefix lands in the hook half. `src/hook/policy.rs:501-530` feeds that half to `consent::evaluate` for hooks, so a declared third-kind key absent from the record would read as hook drift and disarm hooks (`:216-218` comment says exactly this hazard for settings). Facts only: the function and `is_settings_key` would need a third bucket.
- `consent_key_for` / `declared_consent_key_for` need a new arm; `HookPolicy` / `PolicySet` in `src/hook/policy.rs:316,501-545` carry one `kind` per policy (hooks and settings), so a third kind needs a third policy or a generalisation.
- `v` stays 1: a key is just a string in a `BTreeSet<String>`; no field is added.

**What a grim without the prefix does.** `docs/src/content/docs/stability.md:893`: a consent record holding `settings:` keys, "`grim hook allow` drops those keys, so a newer grim asks again". Mechanically (`src/command/hook/allow.rs:94`, `consent::record`): `allow` rewrites the whole set from what that grim declares. Reading is harmless: `evaluate` takes a set difference and unknown extra entries in the record are not in any declared set. `.agents/adr/adr_settings_artifact_kind.md:45` records this as the accepted outcome (row 10) and `:967-984` lists the tests.

**Per-option facts (key shape).**
- Prefixed key in the same `hooks` set (`mod:<binding>@<registry>/<repository>`): for, precedent is exactly the settings choice (`:198`), no `v` bump, no new file, old grims ignore it and re-ask after `allow`. Against: field is still named `hooks` (the report `HookConsentReport.hooks` carries settings keys too, ADR `:984`), `split_by_kind` must change, an old grim's `grim hook allow` silently drops the key.
- New record field or `v: 2`: for, an old grim reads an unknown version as absent and fails safe (not consented). Against: that discards hook and settings consent too, so every workspace re-asks on a downgrade; `deny_unknown_fields` means an old grim cannot read a new field at all.

## 2. Consent record path

**Every reader and writer of `$GRIM_HOME/hooks/consent/`.**
- Path owner: `src/install/hook_dispatch.rs:98` (`CONSENT_DIR = "consent"`), `:191-217`. It is also an entry in `RESERVED_ARTIFACT_NAMES` (`:187-190` comment; `src/oci/hook.rs:97-200`), guarded by the test `every_grim_owned_name_under_hooks_is_a_reserved_binding_name`.
- Read: `consent::load` called from `src/command/status.rs:2298,2569`, `src/command/hook_consent.rs:617,733`, `src/tui/app.rs:3967` (via `recorded_hooks`).
- Write: `consent::record` from `src/command/hook_consent.rs:759` (the seam for `grim add` and an accepted prompt), `src/command/hook/allow.rs:94`; TUI Install via `hook_consent` (`src/tui/app.rs`).
- Remove: `consent::revoke`, `src/command/hook/revoke.rs:64`, which deletes exactly `consent_path` (`src/hook/consent.rs:358-365`).
- Path displayed: `src/command/hook/allow.rs:111`, `src/command/hook_consent.rs:243`.
- The `grim-hook` launcher does not read it: no match for `consent` in `crates/` except an unrelated comment in `crates/grimoire-hook-core/src/audit.rs:150`; the launcher reads only `dispatch.json` beside itself (`.agents/adr/adr_hook_native_dispatcher.md`).
- Settings store reuses the filename shape only: `src/install/settings_store.rs:171,307` (test asserts `file_name` equals `consent_path`'s), real store is `$GRIM_HOME/settings/<workspace-key>.json` (`:15`).
- `grim-home-relative` / `grim-home-in-workspace`: `HookArmingCause` (`src/api/artifact_status.rs:113-117,477-478`) mapped from `ArmRefusal` (`src/install/hook_registrar.rs:135`, `src/command/status.rs:2934`). They test `$GRIM_HOME` as a whole, not the subdirectory, so a path move under `$GRIM_HOME` keeps both refusals intact. AGENTS.md env table states the same ("`$GRIM_HOME/hooks/`: ... the per-workspace consent records under `consent/`").
- Tests: `test/tests/test_hook_consent.py:87-107,1164` (`_consent_dir` hardcodes `hooks/consent`), `test_settings_consent.py`, `test_settings_gate.py`, `test_settings_status.py`, `test_hook_arming.py`, `test_hooks_flag_off.py`, `test_hooks_wp01_review.py`, `test_hooks_fixes_remove_disarm.py`, `test_remove_relock.py`, `test_golden_pre_settings.py`.
- Docs and catalog naming the path: `docs/src/content/docs/hooks.md:83`, `commands.md:2412`; `catalog/skills/grim-authoring/SKILL.md:164`, `catalog/hooks/README.md:144`, `catalog/skills/grim-usage/references/troubleshooting.md:383,415`, `catalog/skills/grim-usage/references/consume.md:860`.
- Rules and ADRs: `.claude/rules/subsystem-install-layout-hooks.md:105`, `.claude/rules/subsystem-cli-commands.md`, `.claude/rules/arch-threat-model.md:131`, `.claude/rules/arch-principles.md:96`, `.agents/adr/adr_hook_workspace_consent.md:169-200`.
- JSON schemas: none found for the record file itself (`grim schema --kind` covers config, publish, lock, mcp, hook, settings only).

**Is the path a stated contract?** User docs state it (`hooks.md:83`, `commands.md:2412`). `stability.md` exempts, for hooks, "the dispatch table's on-disk shape" (`:534`) and, for settings, "the file format grim keeps its settings records in" (`:598`); it does not list the consent record path as frozen or as exempt. `AGENTS.md:114` states the record as `$GRIM_HOME/hooks/consent/<workspace-key>.json` inside the no-env-form paragraph, and says no variable names or relocates it.

**Precedents for moving on-disk state.**
- Project install state: `$GRIM_HOME/state/projects/<sha>.json` to `<workspace>/.grimoire/state.json` (`.agents/adr/adr_install_state_portability.md:318-346`, `src/install/install_state.rs:1-20,870-874`). Convert on load, delete the old file after, `legacy_migration_lossy` flag skips the reap when conversion lost data (`:793-797,1070,1168`). The legacy path survives only as a discovery helper (`legacy_project_path`).
- Catalog cache: single `catalog.json` to per-source `catalog/<hash>.json`, legacy file reaped, no migration (`adr_multi_registry_mcp.md` row in `.claude/rules/arch-principles.md`).
- Render layout: `output_at_current_layout` plus `reap_moved_outputs` plus an upgrade fixture (`adr_vendor_config_and_selection.md` D3; fixtures `test/data/golden/pre_hooks_03e59b0/`, `pre_settings_47da12f/`, `pre_dependencies_cdeedcaa/`).
- AGENTS.md Principle 9: "Layout moves ship automatic state migration, an old-path reaper, and an upgrade fixture", written for released surfaces; hooks are experimental (`stability.md:525`).
- How they avoid re-importing stale state: deletion of the old file at migration time, lossy-guard to keep it when not fully converted, and new path read first. No precedent keeps a permanent fallback read of an old location for security-relevant state.

**Facts for each option.**
- (a) Keep the path. For: zero migration, tests, docs, launcher and four catalog pages unchanged; `consent` already reserved (`hook_dispatch.rs:187-190`). Against: kind-agnostic record sits under `hooks/`; the code comment justifies this as "everything that decides whether a hook arms lives in one directory" (`:177-186`), which a third kind does not alter.
- (b) Move to `$GRIM_HOME/consent/` with one-time import and delete. For: matches the install-state and catalog precedents; a record that existed is converted once. Against: touches the 9 files under `src/` named above, 10 test files, 8 docs and catalog pages, the threat-model and ADR text; `consent` stays a reserved binding name forever (the reserved list is append-only, `src/oci/hook.rs:113-117`); import must preserve `workspace`-field identity check; any downgrade grim reads the old path, finds nothing, and re-asks (fail-safe, inferred from `load` returning `None`); an old grim's `grim hook revoke` would not remove the new-path record (inferred from `revoke` deleting only its own `consent_path`). Whether `$GRIM_HOME/consent/` collides with another top-level `$GRIM_HOME` entry was not checked.
- (c) Move with permanent read-fallback. For: no import step, downgrade-safe both ways for reads. Against: `revoke` (`consent.rs:358-365`) removes one path only, so a fallback to the old file would resurrect consent a user revoked unless `revoke` deletes both; `record` writing to the new path leaves a stale old record that `load` could still read when the new one is absent; no precedent in the repo keeps a permanent fallback for an access-control record.

## 3. Kind literal name

**Naming pattern of existing kinds** (`src/oci/artifact_kind.rs:27,126-150`; `src/lock/grimoire_lock.rs:97-121`; `src/config/project_config.rs:789-821`; `src/config/declaration.rs:594-613`):

| kind literal | config table | lock array | `subdir` |
|---|---|---|---|
| skill | `[skills]` | `[[skill]]` | `skills` |
| rule | `[rules]` | `[[rule]]` | `rules` |
| agent | `[agents]` | `[[agent]]` | `agents` |
| bundle | `[bundles]` | `[[bundle]]` | `bundles` |
| mcp | `[mcp]` | `[[mcp]]` | `mcp` |
| hook | `[hooks]` | `[[hook]]` | `hooks` |
| settings | `[settings]` | `[[settings]]` | `settings` |

Singular literal, plural table (except `mcp` and `settings`, already plural or uncountable), singular lock array. The `--kind` value parsers list the literals in six places (`src/command/build.rs:48`, ADR `adr_settings_artifact_kind.md:169-186`). OCI wire: `com.grimoire.kind` annotation carries the literal; `application/vnd.grimoire.<literal>.v1` is the legacy `artifactType` (`artifact_kind.rs:176-186`).

**Per candidate.**
- `plugin`: heaviest collision. `[[plugin]]` already exists in `marketplace.lock` as a per-plugin declaration hash array (`src/lock/grimoire_lock.rs:119-122`, skipped from the published `grimoire.lock` schema); a `[plugin]` table exists in `grimoire.toml` (`docs/src/content/docs/configuration.md:1057-1108`, `config::plugin_meta::PluginMeta`, and `grim config unset plugin.logo`); `marketplace.toml` uses `[plugins.<name>]`; the CLI has `grim export plugin`, `--plugin`, `grim update --marketplace` `<plugin>:<member>` selectors, and a `plugin` JSON field on update rows (`.claude/rules/subsystem-cli-commands.md`); 951 hits of plugin/plugins in docs and catalog; generated files are `plugin.json`, `.claude-plugin/`. A kind named `plugin` would need `[plugins]` as its config table, colliding with `marketplace.toml`'s `[plugins.x]` vocabulary, and would be ambiguous with the Claude-plugin wrapper that is the very output.
- `extension`: "Vendor extensions" is a documented concept with a public anchor `docs/src/content/docs/artifacts.md#vendor-extensions` (`:179-186,779`; `catalog/skills/grim-authoring/references/skill-spec.md:186` links to `https://grimoire.rs/artifacts.html#vendor-extensions`, and `adr_docs_site_starlight.md` makes dropped URLs a CI failure). 52 hits of the word across docs and catalog, mostly "VS Code extension" (`browse.md`, `ratings.md`, `clients.md`) and "vendor extension". `clients.md:128` states "grim will never install a plugin, extension, or any other runtime code into your client". Code: Agent Plugins `plugin.json` has an `extensions` key (`src/export/family.rs:351,741`); `.extension()` (filename extension) appears 20 times in `src/` (not user-facing). No CLI subcommand, config key or lock key named `extension`. Plural table `[extensions]` and array `[[extension]]` are free.
- `mod`: no hits in docs or catalog; no `"mod"`/`"mods"` string literal in `src/`, `crates/`, `test/`. Rust keyword in source (irrelevant to a TOML/JSON literal). `[mods]` and `[[mod]]` are free. Only Claude uses the word (Claude Code builtin plugin-authoring skill), not OpenCode; the recon file records the Claude-only module API.
- `addon`: no hits anywhere in `src/`, `docs/src`, `catalog`. Free.

**Filename-extension use.** In code, `.extension()` 20 sites (path handling, not user text). In user-facing text, the filename sense is rare ("by extension" in `publishing.md:214`; "one extension per glob" in `catalog/skills/ai-config-authoring/references/rule-design.md:107` means a glob expansion). The user-facing meanings of "extension" are VS Code extension and vendor extension.

## 4. Export seam

**Today.** `src/export/family.rs:159-176` `admits(family, client, kind)`:
- `(Claude, Hook)` returns `Err(NotRepresentable)` ("A hook needs grim at run time", C-140).
- `(_, Rule | Bundle | Hook | Settings)` and `(AgentPlugins, Agent)` return `Err(NoFormatSurface)`.
- `(_, Skill)` and `(Claude, Agent | Mcp)` follow `kind_support`.
- The match is exhaustive over `ArtifactKind` (the `use` at `:160` names every variant), so a new kind needs an arm to compile.

`OmitReason` (`:47-56`) serializes `client-declined`, `no-format-surface`, `not-representable`. In `src/export/stage.rs:1370-1378` an `Err(reason)` pushes an `ExportOmission` and continues; `RenderOutcome::Empty` when nothing is emitted. Hooks and settings are skipped before fetch (`stage.rs:1294,1300`). Report: `omitted` array in the export JSON (`docs/src/content/docs/json-interface.md:98`), a line in the plugin's generated `README.md`, one stderr warning per member per plugin. Exit: omission is never an error, exit 0; an export whose every member is omitted exits 65 `EmptyPlugin` (`docs/src/content/docs/commands.md:3340-3385`, `src/error.rs:545-552`; hooks stated at `hooks.md:390-401`). Docs list per-client admission at `commands.md:3350-3360` and a hook/settings paragraph at `:3379-3385`.

**What the exporter writes.** Claude family: `.claude-plugin/plugin.json` (`family.rs:289-307`, Qoder at `.qoder-plugin/plugin.json`), `skills/<name>/` and `agents/<name>.md` (`stage.rs:1391-1395`), `.mcp.json` (`:1422`), root `README.md`, `assets/logo.<ext>`. Agent Plugins: `plugin.json`, `mcp.json`. No `hooks/hooks.json` is written anywhere; only the comment at `family.rs:163` mentions it. A `_ =>` arm in `render_members` (`stage.rs:1399-1407`) turns a kind admitted but unstaged into `MaterializeFailed`, so admitting a kind also needs a staging and render arm. Plugin version hashes the rendered tree (`family.rs:plugin_version`), so new files change plugin versions (grammar frozen, inputs not; `subsystem-cli-commands.md` export row).

**Per-option facts.**
- (a) Mark not-representable everywhere: for, smallest delta, one arm, mirrors hooks (`NotRepresentable` for Claude); docs and tests follow the hook pattern. Against: `not-representable` is described as "content cannot be expressed in the format", yet the Claude plugin format can carry a hooks module per the recon (`research_harness_mods_codebase_recon.md` section 7), so the token would be inaccurate for Claude; the Agent Plugins family would use `no-format-surface`.
- (b) Admit for Claude family only: for, the artifact is already a plugin in Claude's shape (`.claude-plugin/plugin.json`, `hooks/hooks.json` with `modules`, `hooks/register.tsx`). Against: needs a `hooks/hooks.json` writer, a tree stage, name-grammar checks, a per-client decision for the five Claude-family clients (`claude`, `droid`, `junie`, `openclaw`, `qoder`; `family.rs` `family_of`), `KindSupport` interplay, and README omission text; a mod is Claude Code specific so `droid` / `junie` / `qoder` cannot be assumed to load it (no evidence in the repo either way); the recon lists the export route as "not a ready path".

## 5. Descriptor and names

**Hook.** `hook.toml` carries `name` (`src/oci/hook.rs:402`, example `:38`). `HookManifest::validate` requires `name` equal to the directory stem (`:566-571`, error `NameMismatch` `:1044`) and runs the reserved/binding-grammar checks (`:556-565,679-687`). `grim build` packs under the manifest name (`src/command/build.rs:229-246`) and stamps it as `org.opencontainers.image.title` (`src/oci/annotations.rs:863`). Name lives in the artifact twice: in `hook.toml` inside the tar and in the title annotation. Not a config blob (empty OCI config).

**Settings.** The name is the source file stem `settings/<name>.toml`, validated as `SkillName` (`src/command/build.rs:681-697`), stamped as `org.opencontainers.image.title` (`annotations_for_settings`, `annotations.rs:792-800`); no in-document name field.

**Consumer binding.** The consumer's key in `[hooks]` / `[settings]` of `grimoire.toml` is the binding; `grim add --name` sets it, and bundles pick member bindings (`subsystem-install-layout-hooks.md`, SEC-1 paragraph). `src/oci/hook.rs:560-561` comment: "a consumer may bind such a hook under another name, so install must not drop it". Installed paths use the binding: `payload_dir(grim_home, root, &record.name)` (`src/install/hook_dispatch.rs:329`; install gate `installer::install_one` checks the binding, per `subsystem-install-layout-hooks.md`). Consent key uses the binding too (`consent.rs:176-179`). Settings records and lock entries are keyed by binding (`settings_store.rs`).

**Consequence for a default entry file named after the extension.** At `grim build` the artifact's own name is known (directory stem or manifest name) and is baked into the tar entry paths (`<name>/...`, `skill_package.rs:632-680`, `root_name` is the directory name). At install the binding may differ, and content is materialized under the binding directory (hook payload) without rewriting file names inside the payload. So an entry file named after the artifact name resolves at build and from the manifest at install; a file name derived from the binding would resolve only at install. Which one an extension kind would use is open. Skill precedent: agent export has `rebound_agent` (`src/export/stage.rs:1395-1397`) that rewrites an agent's name on rename, a precedent for rebinding names in content, none for hooks.

## 6. Ignore set and packing

**Default list** (`src/install/ignore_set.rs:21-41`): `__pycache__/`, `*.py[co]`, `.venv/`, `venv/`, `.mypy_cache/`, `.pytest_cache/`, `.ruff_cache/`, `*.egg-info/`, `node_modules/`, `.git/`, `.svn/`, `.hg/`, `.DS_Store`, `Thumbs.db`, `desktop.ini`, `*.swp`, `*.swo`, `*~`, `.idea/`, `.vscode/`. Never ignored: `SKILL.md`, `.grimignore` (`:46`), and `hook.toml` only for the Hook kind (`:49-54,159-161`). Growth of the list is "additive for new publishes only" (`:20`).

**Override.** The root's `.grimignore` (gitignore syntax, 64 KiB cap, only the walk root's counts, nested ones are ordinary files) is added after the defaults, so `!pattern` re-includes (`:4-12,196-215`). Test `negation_in_grimignore_reincludes_a_default` (`:281-283`) covers `!node_modules/`. A `!` line cannot reach a file inside an ignored directory (`:175-178`), so the directory itself has to be re-included (`!node_modules/`).

**Walker.** One walker for skills and hooks: `pack_tree_limited` / `collect_files` (`src/skill/skill_package.rs:187-262,632-690`), caps 512 MiB, 10,000 files, 50,000 nodes, depth 64 (`:497-513`). Rules and agents pack a single index file (`pack_rule_file` `:328`, `pack_agent_file` `:343`) and do not use this walker. Settings, mcp and bundles pack JSON documents. A new tar kind would call `pack_tree_limited` with its own `ArtifactKind` (the kind only changes the never-ignored manifest, `:159`). Hook builds already warn when `node_modules/`, `.venv/`, `venv/` is pruned (`warn_left_behind_dependencies`, `:238-258`), a precedent for a dependency-directory warning. The drift-hash walk (`src/install/content_hash.rs:188-216`) uses the same `IgnoreSet`, leniently.

**Symlinks.** `collect_files` uses `symlink_metadata` and takes only `meta.is_dir()` or `meta.is_file()` (`:677-698`); a symlink is neither, so it is silently dropped for skill and hook trees, with no warning. `content_hash.rs:216` also uses `symlink_metadata`. Rule and agent index files that are symlinks are rejected, not dropped (`reject_symlinked_index`, `:166-190`).

## 7. Name collisions on disk

No dedicated cross-artifact collision detector exists at render time. Per-destination gate: when a destination exists and no recorded output claims that `(anchor, relative)` path with matching content hash, install refuses `InstallErrorKind::UntrackedDestination` (`src/install/install_error.rs:148-155`; logic `src/install/installer.rs:1210-1250`), classified exit 65 (DataError) with reason `untracked-destination`, forceable (`src/error.rs:254,637-640`). Message: "destination '<path>' already exists for client <c> and was not created by grim; rerun with --force to overwrite". The check consults every output in the record, because the shared skill pool lets several clients own one directory (`installer.rs:1219-1224`); a destination owned by another record's output therefore does not raise it. Other related gates: dev-install versus declared binding collision (`src/command/add.rs:587-611`, exit 64); settings conflict among armed packages exits 78 (`subsystem-cli-commands.md`, `add` and `lock` rows); export `MemberConflict` exits 78 and `RenameCollision` 65 (`src/error.rs:545-556`). A binding exists once per kind table, so a skill `x` and an extension `x` differ by table; they collide on disk only if their per-client directories coincide, in which case the outcome above applies. Prune uses `shared_by_surviving_sibling` (`src/install/prune.rs:747`), record-only refcount, no filesystem fallback (`subsystem-install-layout-hooks.md`).

## negative:

- No JSON schema or `grim schema` output covers the consent record file.
- The `grim-hook` launcher does not read, write or know the consent path.
- No precedent keeps a permanent read-fallback to an old location for security-relevant state.
- `grim-home-relative` / `grim-home-in-workspace` do not depend on the `consent/` subdirectory.
- No hook-exporter or `hooks/hooks.json` writer exists in `src/export/`.
- No `mod`, `addon` string literal or doc occurrence anywhere in the repo.
- No existing symlink warning in the shared pack walker.
- Not checked: whether a new top-level `$GRIM_HOME/consent/` clashes with another `$GRIM_HOME` entry; whether `droid`/`junie`/`qoder` load Claude hook modules.

## leads:

- Generalise `split_by_kind` / `PolicySet` (`src/hook/consent.rs:219`, `src/hook/policy.rs:501`) before any third key prefix is chosen: it is the one place a new prefix silently reads as hook drift.
- `.agents/adr/adr_settings_artifact_kind.md:45,967-984` and `test/tests/test_old_grim_settings.py` hold the precedent tests for old-grim consent behavior; a third-kind ADR would extend them.
- `.agents/adr/adr_artifact_trust_model.md:97-113` and the recon note on "not an invitation to gate the next kind": a third gated kind needs an ADR amendment.
- The "VS Code extension" and "vendor extensions" uses sit in pages under URL-stability CI (`docs/check_urls.py`).
- Check `src/export/family.rs` `family_of` and `kind_support` for the five Claude-family clients before choosing export option (b).
- Verify `$GRIM_HOME` top-level names (`ls` of a live home, `src/store/paths.rs`) before option (b) or (c) on the consent path.
