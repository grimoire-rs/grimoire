# ADR: The experimental `settings` artifact kind

## Metadata

**Status:** Accepted
**Date:** 2026-11-04
**Deciders:** Michael Herwig (owner, ratified [`config-artifact-type.md`](../discussions/config-artifact-type.md) on 2026-11-03) + Claude (architect, goal loop)
**Beads Issue:** N/A
**Related PRD:** N/A (ratified discussion: `.agents/discussions/config-artifact-type.md`)
**Blast radius:** external contract: OCI media type and annotation, `grimoire.lock`, `grimoire.toml`, install state, consent record contents, `--format json` reports and error document, status tokens, CLI flags. Internal: install pipeline, six vendor files, splice engines.
**Reversibility:** mixed, labelled per decision. Wire names, document grammar, record shape, key prefix and the flag pair are one-way doors by choice: they are Unstable while the feature flag exists (§ Migration and rollout), and this design keeps them additive anyway. The key list and the size cap stay two-way while the flag exists.
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (Rust 2024, Tokio, no new dependency)
**Domain Tags:** security | data | api | integration
**Supersedes:** N/A
**Extends:** [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md) (decision 2, second scoped exception), [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md) (kind-neutral record, flag and prompt), [`adr_artifact_dependencies.md`](./adr_artifact_dependencies.md) (`requires` and bundle membership gain the kind)
**Superseded By:** N/A

## Context

Grim installs skills, rules, agents, MCP servers and hooks, but no harness configuration. The
ratified discussion decides that it should, as one new kind, for six harnesses, with public packages
in scope. Its Decisions are frozen and its Threads are carried here. This ADR turns them into
contracts, closes the eight open questions, and records where a code fact corrected a premise.

The kind differs from every other in one way: it writes **pieces** of files the user and the harness
also edit. So the design has one hard centre: a per-piece rule engine that reads the file first, and
refuses every overwrite together.

### Premise corrections

Citations are at HEAD `25b722a`. Each row corrects a source, and none reopens a Decision.

| # | Source claim | Fact | Effect |
|---|---|---|---|
| 1 | `gated` is documented at `artifact_status.rs:46` (discussion :109) | The "Hook only." doc for `Gated` is at `src/api/artifact_status.rs:36`; `:47` is `NotArmed` | Widen `:36`, `:47` docs |
| 2 | `uninstall.rs:106` (`research_config_artifact_codebase.md:39`) | `uninstall` starts at `src/install/uninstall.rs:154` | Cite only |
| 3 | `AlreadyTracked` "stays a `grim status` note" (discussion :200) | It is a `tracing::warn!` at `src/install/hook_registrar.rs:1049`; status has no such note | Decision 8 adds the note |
| 4 | `.codex/config.toml` is why Codex is global-only, citing `vendor_codex.rs:226` (discussion :194) | `:226` is about `hooks.json`. Grim already writes `.codex/config.toml` at project scope for MCP (`vendor_codex.rs:278`) | Global-only stands, because the file is tracked; the citation changes |
| 5 | "A repo's `.claude/settings.json` is never written" (discussion :191) | Grim already writes `claudeMdExcludes` there (`src/install/claude_config.rs:100`, `:258`) | True for this kind only: the typed list cannot reach that file |
| 6 | A scalar conflict is "computable from the lock alone" (discussion :167) | The lock holds pins, not values (`src/lock/locked_artifact.rs:41`) | Decision 5 reads the layers |
| 7 | `mcp_entry_vendor_owned_keys` looked like a canonicaliser (Discover) | It carries over client-owned keys (`src/install/vendor.rs:1159`) | Not reused; `settings_value_hash` is new |
| 8 | "Older grims refuse with exit 78" also covers state (`stability.md:448-450`) | A state read fails through `state_io` as `TargetIo`, exit 74 (`src/command/install.rs:389`, `src/error.rs:636`) | Decision 6 states it; the docs line is corrected |
| 9 | "A new kind … exit 78" (discussion :260) | A bundle member or `requires` edge of this kind on an older grim is 65 | Old-grim matrix |
| 10 | "Older grims ignore" settings keys in the consent record (discussion :226) | An older `grim hook allow` replaces the record and drops them (`src/command/hook/allow.rs`) | Fail-safe re-prompt, named |
| 11 | The error follows MCP's `RefusedUntracked` (discussion :89) | That outcome is single-item and per artifact; `finish` keeps the first error and drops the report (`src/command/install.rs:506`) | Decision 5 is a cross-package pass |
| 12 | "Must target the main checkout" (field list § 8) | Overruled in Decision 9 | Worktree's own file |
| 13 | Claude's file is JSONC (Discover) or strict JSON (field list § 1.1) | Strict for Claude; grim's tolerant reader is harmless | Grim never introduces a comment |
| 14 | Other harnesses are "global-only" because no project file is touched (reading of discussion :194) | `.qoder/settings.json` is written at project scope for MCP (`vendor_qoder.rs:91`) | Global-only is this kind's rule, not a statement about those files |
| 15 | `git_info_dir` targets the right file in a linked worktree (doc comment, `hook_registrar.rs:2054-2061`) | Probed on git 2.43.0: a rule in the per-worktree `info/exclude` is ignored; one in the common dir is honoured | Existing defect, class: ineffective ignore rule. Separate `fix:` commit |
| 16 | Hook convergence owns the exclude rule alone (`hook_registrar.rs:1035`) | It drops the rule when no hook remains, even if settings still use the file | Decision 9 checks both users |
| 17 | The phase can use its `lock` argument | Six seats pass a projection or a synthetic lock: `add.rs:718`, `install.rs:365`, `update.rs:600`, `tui/app.rs:2893,3030,3135` | Decision 5 adds `SettingsRun` |
| 18 | "Canonical JSON" for MCP (discussion :133) | `to_layer_bytes` is pretty JSON plus a newline | Settings copies that, with sorted keys |
| 19 | "Claude 16 keys", "Codex 13 rows" (field list § Recommendation) | Both include rows with unresolved values | Appendix A decides per row |
| 20 | `consent::record` guards `GRIM_HOME` (reading of discussion :232) | Neither `record` nor `load` calls `validate_grim_home` (`src/hook/consent.rs:210`, `:269`) | The settings gate calls it first |
| 21 | Refusals always lose the report (`finish`) | `grim update` emits its report with exit 65 and marks rows `refused` (`src/command/update.rs:333-371`) | Update carries `pieces` in its report |
| 22 | `validate_grim_home` has two causes (discussion :233) | It also returns `GrimHomeShared` on Windows (`hook_registrar.rs:1647`) | All three map to `not-armed` |
| 23 | `outputs` is "never empty" (`install_state.rs:350`) | No parse rule enforces it and no non-test consumer indexes `outputs[0]` | A zero-output record is sound; the doc widens |
| 24 | A filter on `kind == Settings` keeps repository state away from settings files (first draft of Decision 6) | Repository-delivered state can still authorise a removal through a record of another kind, on a path with no flag, consent or `$GRIM_HOME` check. Class: trust-boundary bypass, CWE-345. Locations: `install_state.rs:826`, `prune.rs:206`, `uninstall.rs:172`. Confirmed in a sandbox at HEAD; it predates this ADR | Decision 6 gains a kind-independent load rule, a `fix:` commit ahead of the feature |
| 25 | A piece stays while "any package in the lock declares it" (discussion :164) | A gated package's layer is not fetched, so its declaration is unknown at plan time | Row 6 reads surviving records. A gated package's record survives, so its pieces stay |
| 26 | `reap_dropped_clients` is the seam for a client leaving (first draft of Decision 5) | It deletes through `remove_entry`, which splits two segments and knows no `element` (`prune.rs:714`, `uninstall.rs:411`) | The kind gets its own reap and a closed skip list |

## Decision Drivers

1. The discussion's Decisions and Threads, carried without feature cuts.
2. Principle 9: additive only, and a project without this kind stays byte-identical.
3. T3: no file a repository can deliver may authorise a settings write or removal.
4. A user's value is never changed silently, and every applied change is visible (I5).
5. Merge code has a history of 19 fix commits, so new machinery stays small and shared.
6. One consent mechanism, and the harness stays the execution policy layer.

## Industry Context & Research

**Research artifacts:**
`.agents/research/research_config_artifact_{field_list,codebase,prior_art,vendor_semantics,archaeology,community}.md`.

**Trending approaches:** vendors that let a package carry settings restrict it to a small allowlist
(prior art § Direct Answer), and Claude's settings shape is being cloned by Qoder and Copilot (field
list § Trends).

**Key insight:** Kubernetes Server-Side Apply separates declared intent (in git) from field
ownership (with the live object), and classes fields as `set`, `map` and `atomic`. Grim's lock,
record and merge classes are that split, and the strongest evidence for keeping the record out of
the repository.

## Considered Options

### Ratified choices

Recorded with what they beat. These are closed.

| Ratified choice | Beat | Source |
|---|---|---|
| Ownership comes from the lock | A record as the source of ownership | Decisions |
| The file decides; a record never authorises an overwrite | Comparing old and new locks, which `grim install` after a pull cannot do | Decisions |
| A matching value is adopted | The owner's `ownership` error | Decisions |
| No hash in the lock | A per-value hash in the lock | Decisions |
| Consent per kind, no key logic | A key-scoped "loosens" gate | Decisions |
| One mechanism, one record, one answer | A second record, or per-kind consent | Decisions |
| Version bumps as for hooks | Re-asking on a bump | Decisions |
| One JSON document with vendor objects, in one layer | One layer per harness; a tar payload | Decisions |
| Vendor keys only | A generic core, or a generic-plus-vendor mix | Decisions |
| Public packages too | Own machines only | Decisions |
| Project scope writes `.claude/settings.local.json`; other harnesses are global-only | Writing tracked project files | Threads |
| `$GRIM_HOME/settings/<workspace-key>.json` for project scope | `<workspace>/.grimoire/state.json` (replaced by `git pull` in a local test) | Threads |

### Open axes

Criteria and weights: **S** safety against T1-T5 (4), **C** compatibility (4), **F** fidelity to the
source (3), **K** simplicity (2), **L** legibility (1). Each option scores 0 to 3. The maximum is
42.

| Axis | Option | S | C | F | K | L | Total | Honest cost |
|---|---|---|---|---|---|---|---|---|
| Piece record | **A: additive `element` plus RFC 6901 `entry`** | 3 | 3 | 3 | 2 | 3 | **40** | `entry` means two grammars by kind |
| | B: element suffix in the `entry` string | 3 | 1 | 2 | 1 | 1 | 25 | Breaks `split_pointer` readers, needs escaping |
| | C: one per-file output with a `pieces` list | 3 | 1 | 1 | 1 | 2 | 23 | Makes required `content_hash` meaningless |
| Engine place | **A: preflight plus a final phase** | 3 | 3 | 3 | 1 | 3 | **38** | Two functions, four call sites |
| | B: final phase only | 2 | 3 | 2 | 3 | 2 | 34 | `update` saves the lock before a 78 |
| | C: check in the resolver only | 2 | 3 | 1 | 1 | 2 | 27 | A pulled lock never reaches it |
| | D: per-artifact gate in the work list | 1 | 3 | 0 | 2 | 1 | 21 | Cannot list all pieces, writes before refusing |
| Record store | **A: merge on load, split on save** | 3 | 3 | 3 | 2 | 2 | **39** | Two files behind one type, and a closed skip list at generic removal seams |
| | B: a second `InstallState` value | 3 | 3 | 3 | 2 | 2 | 39 | Status, badge, TUI and pipeline signatures change; generic removal seams cannot reach the kind |
| | C: a settings-only store with its own consumers | 3 | 3 | 2 | 1 | 1 | 33 | Status, uninstall and prune each branch |
| Consent key | **A: `settings:` prefix on the key string** | 3 | 3 | 3 | 3 | 2 | **41** | Old report rows show the prefix |
| | B: unprefixed key | 1 | 3 | 2 | 3 | 1 | 29 | A hook `x` consents settings `x` |
| | C: new top-level record field | 3 | 0 | 1 | 2 | 2 | 21 | Older grims fail on `deny_unknown_fields` |
| Flag name | **A: `--trust-workspace`** | 3 | 3 | 3 | 3 | 2 | **41** | Reads oddly at global scope |
| | B: `--consent` | 2 | 3 | 3 | 2 | 1 | 34 | Implies a record; the flag writes none |
| | C: `--trust-settings` | 2 | 3 | 0 | 2 | 1 | 25 | Per kind, ruled out |
| Alias verb | **A: `visible_alias = "consent"` on `Hook`** | 3 | 3 | 3 | 3 | 2 | **41** | `consent list` shows hooks only |
| | B: `grim hook consent allow` | 3 | 3 | 2 | 2 | 1 | 35 | Kind-specific noun in front |
| | C: top-level `grim consent` | 3 | 1 | 3 | 1 | 3 | 30 | Count, telemetry and report change |
| | D: `grim allow`, `grim revoke` | 3 | 1 | 3 | 1 | 2 | 29 | Two new commands |
| Worktree file | **A: the worktree's own local file** | 3 | 3 | 2 | 3 | 2 | **38** | A main-checkout scalar shadows it |
| | B: refuse in a linked worktree | 3 | 3 | 0 | 3 | 2 | 32 | A feature cut |
| | C: the main checkout's local file | 1 | 3 | 2 | 1 | 2 | 26 | Writes outside the workspace |
| Cursor `permissions.*` | **A: ship global, `on-limited`** | 2 | 3 | 3 | 3 | 2 | **37** | A project block shadows it |
| | B: follow-up issue | 3 | 3 | 1 | 3 | 1 | 34 | Misses the Requirements' allow and deny |
| | C: also write project `cli.json` | 0 | 3 | 1 | 1 | 2 | 19 | Tracked file, I1 |
| Cursor file | **A: edit an existing file only** | 3 | 3 | 3 | 3 | 2 | **41** | A per-client decline on a fresh machine |
| | B: create a partial file | 1 | 3 | 2 | 3 | 1 | 29 | Repair on first creation is unverified |
| | C: create a skeleton with an invented `version` | 1 | 2 | 2 | 1 | 1 | 21 | Grim guesses a vendor format version |
| Unknown vendor | **A: refuse at build, warn at install** | 3 | 3 | 3 | 3 | 2 | **41** | A typo survives only if published past build |
| | B: refuse everywhere | 3 | 1 | 2 | 3 | 3 | 31 | A new harness breaks older consumers |
| | C: tolerate everywhere | 2 | 3 | 1 | 3 | 1 | 30 | Typos publish |
| Numbers | **A: integers only, canonical before hashing** | 3 | 3 | 3 | 2 | 2 | **39** | Refuses a float no key needs yet |
| | B: admit floats, canonical before hashing | 3 | 3 | 3 | 1 | 2 | 37 | Speculative |
| | C: hash raw values | 1 | 3 | 1 | 3 | 1 | 26 | `1` against `1.0` reads as `modified` |
| OpenCode expansion | **A: refuse at build, release, install** | 3 | 3 | 3 | 3 | 2 | **41** | A literal `{env:` string cannot ship |
| | B: escape on render | 1 | 3 | 2 | 1 | 1 | 25 | No documented escape exists |
| | C: warn only | 0 | 3 | 1 | 3 | 1 | 22 | The harness expands it anyway |
| Change list | **A: top-level array plus stderr lines** | 3 | 3 | 3 | 2 | 3 | **40** | One more key on four reports |
| | B: per-row field | 2 | 3 | 2 | 2 | 2 | 32 | An orphan removal has no row |
| | C: stderr only | 2 | 3 | 1 | 3 | 1 | 30 | No machine evidence |
| Codex rules | **A: typed field, rendered Starlark** | 3 | 3 | 3 | 2 | 2 | **39** | A renderer and a real-binary test |
| | B: follow-up issue | 3 | 3 | 1 | 3 | 1 | 34 | Codex keeps no command allow or deny surface |
| | C: raw Starlark passthrough | 0 | 3 | 2 | 3 | 1 | 25 | Publisher code in a config format |
| Write race | **A: re-read immediately before the write** | 2 | 3 | 3 | 3 | 2 | **37** | A small window remains |
| | B: no check | 1 | 3 | 0 | 3 | 1 | 23 | A harness write is lost silently |
| | C: sidecar lock | 2 | 2 | 1 | 1 | 1 | 22 | Harnesses ignore it, and it litters a user's directory |

## Decision Outcome

**Chosen:** option A on every open axis. Record store ties at 39 with B. A stands because the closed
skip list in Decision 5 gives B's protection with a unit pinning it, and every consumer keeps one
`&InstallState`. Compatibility is decided in § Migration and rollout.

### Decision 1: wire and declaration surface (one-way door for names)

| Surface | Contract |
|---|---|
| Kind | `ArtifactKind::Settings`, appended last; `ALL` length 7; `all_index` gains an arm; wire token `settings`; `subdir()` = `settings` |
| Layer | `application/vnd.grimoire.settings.v1+json`; `is_tar_kind` false; OCI empty config |
| Size cap | `SETTINGS_LAYER_SIZE_LIMIT` = 64 KiB. Two-way: a later widening is additive |
| Annotation | `com.grimoire.kind=settings` through `annotations_for_settings` |
| Lock | `[[settings]]` (`RawLock.settings`), emitted only when non-empty |
| Config | `[settings]` table; enters the declaration hash only when non-empty; appended to `DECLARATION_TABLES` |
| Schema | `grim schema --kind settings`; the literal joins six `value_parser`s and the requires enum in `schema.rs` |
| Feature flag | `[options.experimental] settings = true`, `settings_enabled()`, key `ExperimentalSettings` appended at `ALL[11]`. Project-settable as `hooks` is. No env form |
| Build | `grim build --kind settings` is required; `detect_kind` keeps mapping `.toml` to bundle |
| Publish order | After hooks, before bundles |
| Bundles | Yes. `RawBundleSource` gains `settings`, appended. Members sort last by the derived `Ord` |
| `requires` | Yes, both directions. The document splits off top-level `requires` as MCP does |
| Export | `grim export` declines through `admits` |
| Silent sites | Add arms at `fetch.rs:386-390`, `installer.rs:2425-2428`, `status.rs:669-677`, `client_supports_kind` (`installer.rs:1289`, answering from `settings_fields().is_empty()`), `kind_is_permanently_declined` (`installer.rs:1327`), `is_declined_global_pair` (`path_anchor.rs:1049`), `expected_outputs.rs:87,126` (no pending outputs), the resolver tables (`resolver.rs:121,352,388,678`), `grimoire_lock.rs:338,383,434`, `command_error.rs:43` |

With the flag off, `grim add` declares and locks. The settings phase then fetches nothing, writes
nothing and removes nothing, preflight is skipped, and status reports `gated`. Bundles and `requires` follow the
discussion: delivery is the hook rule, with no rule based on how an artifact was reached.

### Decision 2: document format (one-way door for the grammar)

```toml
# settings/team-defaults.toml, built with `grim build --kind settings`
description = "Team defaults"       # required, MCP's descriptive fields otherwise
[claude]
model = "sonnet"
[claude.permissions]
deny = ["Bash(rm:*)"]
[codex]
sandbox_mode = "workspace-write"
[[codex.rules]]
pattern = ["git", "push"]
decision = "prompt"
```

The descriptive fields are `McpDescriptor`'s (`src/oci/mcp.rs:175-210`). The key `requires` is
reserved. Every other top-level key names a vendor and must hold a table. Parsing is a hand-written
pass, because `flatten` defeats `deny_unknown_fields`. The layer is pretty JSON plus a newline with
sorted keys.

`SettingsDocument::validate(mode)` runs at build, at release and publish, and on every consumed
layer, since the registry is untrusted.

| Check | `Author` mode (build, release, publish) | `Consume` mode (install, add, update) |
|---|---|---|
| Unknown field under a known vendor | 65 | 65, reason `unsupported-artifact-feature` |
| Unknown vendor object | 65 | Warning; nothing is written for it |
| Wrong type, or a value outside an enum | 65 | 65 |
| `null` anywhere | 65 | 65 |
| TOML datetime | 65 | Not representable in the layer |
| Float, or an integer outside `i64` | 65 | 65 |
| `{env:` or `{file:` in any decoded string or key under `opencode` | 65 | 65, not forceable |
| Codex `default_permissions` with `sandbox_mode` or `sandbox_workspace_write`, same document | 65 | 65 |
| URL userinfo in a marketplace source (the literal-credential rule) | 65 | 65 |
| Control character in `codex.rules` | 65 | 65 |
| Binding name not a `SkillName` | 65 at `add` | 65 in the phase, as the control |

**Numbers.** No listed key takes a float. Integers may occur inside atomic objects. A file may spell
one `1.0`, so `settings_value_hash` canonicalises an integer-valued float to an integer before
hashing. `entry_value_hash` stays as is, so MCP's recorded hashes keep matching.

**Grim-managed keys.** No list row's first segment is `claudeMdExcludes`, `hooks`, `mcpServers`,
`mcp`, `mcp_servers` or `instructions`. A denylist test pins it, so overlap is unreachable rather
than checked at run time. `$schema` is never touched.

**OpenCode expansion.** OpenCode expands these tokens at load, with no documented escape (field list
§ 6). A package must not make a harness pull a user's environment or files into config. The check
runs on decoded values, and grim renders canonical JSON, so no escape spelling survives. A value
already in the user's file is not grim's to judge.

**Codex exclusion.** One document is checked here; across packages in Decision 5. A user's own
sibling key is never in the comparison, so a package value beside a user's excluded key is delivered
and the harness decides.

### Decision 3: the typed key list as data (two-way while experimental)

The list is a static table per vendor: `Vendor::settings_fields()`. Appendix A is its first content,
with 90 rows. Consent reads none of it, and the research's "loosens" marks are unused. The list is
Unstable while the feature is experimental. An unverified row stays off and goes to Appendix B.

A row's merge class is a layout fact, because it decides whether a record carries `element`.
Changing it is a layout move under Principle 9: a state migration, or the row re-added under a new
pointer. A golden test pins `(pointer, class)` for every row.

`on-limited` means verified with a named limit in the reference page. Grim's promise for every row
is *delivered*, not *enforced*.

- **Cursor `permissions.*`: ship, `on-limited`.** The Requirements name allow and deny, the fact is
  verified, and shadowing can only stop a rule applying. A project `.cursor/cli.json` with a
  `permissions` block replaces the global one (forum, staff-confirmed), so a global `deny` can be
  shadowed. T3 is the attacker exposed. The harness owns that merge, and the reference page says so.
- **Qoder `permissions.*`: ship, `on-limited`.** Its two pages disagree on array merge. The worst
  case is the same shadow.
- **Cursor's file.** Grim writes only into an existing `cli-config.json`. Otherwise that client
  declines with a warning and cause `client-config-absent`. Required pointers (`/version`,
  `/editor`, `/editor/vimMode`, `/permissions`, `/permissions/allow`, `/permissions/deny`) are never
  removed: a set keeps `[]`, and a scalar stays and goes to `abandoned_entries`.
- **Cursor's path.** `$CURSOR_CONFIG_DIR`, else `~/.cursor/cli-config.json` if it exists, else
  `$XDG_CONFIG_HOME/cursor/cli-config.json` on Linux. Grim honours the variable here because
  upstream documents it for exactly this file. `vendor_cursor.rs:23` ignores it for skills for the
  same reason: upstream does not document it there. A `VENDOR_ROOTS` row stays a pure function of
  env and home, so two rows carry it: `cursor-cli` (`$CURSOR_CONFIG_DIR`, else `~/.cursor`) and
  `cursor-cli-xdg`, anchors `cursor-cli-root` and `cursor-cli-xdg-root`, append-only, on the
  `kilo-config` precedent. `settings_config_path` makes the existence choice, and the record stores
  the anchor it chose.
- **OpenCode's file.** The first existing of `opencode.jsonc` and `opencode.json`, through
  `opencode_config::config_path_for_scope`. `config.json`, upstream's third name, is a recorded gap
  in Appendix B.
- **Claude `permissions/defaultMode` at project scope.** Only `default`, `acceptEdits` and `plan`
  apply there. Another value declines that piece with a warning, because Claude ignores it from a
  local file.
- **`enabledPlugins` and same-named keys stay off** (Open question 5).

### Decision 4: the piece model (one-way door for `element`)

A piece is the unit of ownership: client, file, pointer, and an element identity for a set element.
Each piece is one `ClientOutput`, as the Thread says.

- `ClientOutput` gains `element: Option<String>`, serde default, omitted when `None`, so existing
  state bytes do not change.
- For settings outputs `entry` is an RFC 6901 pointer of any depth with `~0` and `~1` decoded.
  `split_pointer` stays two-segment for MCP.
- The hash of a set element is `settings_value_hash(Value::String(element))`. It matches exactly
  when the element is present, so a hand-edited element reads as `missing` plus a foreign element
  the rule never compares. `current_hash` reads the array at the pointer and looks the element up,
  so a bare string equal to the element reads absent.
- `is_present` gains a `kind` parameter. Private `entry_format(kind)` and
  `entry_vendor_owned_keys(kind)` replace `mcp_format` and `mcp_vendor_owned_keys` on the read path.
  For settings they answer `settings_config_format()` and `&[]`.
- `read_entry_value` dispatches on `kind`. Settings reads use `value_at` at any depth, including
  one-segment TOML pointers, which `split_pointer` reads as absent today.

| Class | Piece | Add or change | Remove |
|---|---|---|---|
| `Object` | None: recursion only. A missing container is created | Implied | Pruned if left empty, except required pointers |
| `Set` | One per element | `upsert_array_element_at` | `remove_array_element_at`, then `PruneEmpty` on an emptied array unless required |
| `Map` | One per entry, compared whole | `upsert_member_at` | `remove_member_at` |
| `Scalar` (also ordered lists and atomic objects) | One | `upsert_member_at` | `remove_member_at` |
| `OwnFile` | The whole file, `entry: None`, byte hash | Atomic write | Delete |

**List of objects is dropped from the first slice.** No listed key has that shape, so no enum
variant ships. The Thread stays the design for the first such key.

JSON and JSONC share `json_splice`; TOML uses `toml_splice`. New functions take a full pointer and
are named in the contracts. TOML handles inline tables in place, treats `ArrayOfTables` as a type
mismatch, and keeps decor. Each file gets one read-modify-write, never one write per element.

### Decision 5: the rule engine and its place in a run (one-way door for the outcome surface)

**Placement.** One phase after other artifacts is not enough: it cannot give `update` a 78 before
its lock is saved. So the work splits in two.

| Step | Where | Why |
|---|---|---|
| `settings::preflight` | `add` and `lock`, before the lock is saved; `update` between `roll_forward` and `lock_io::save`, the slot that "leaves nothing on disk changed" | No lock and no client file is written on a 78. An accepted consent answer stays, because it precedes the armed set |
| `settings::preflight` | First step of `install_all_with_progress`, under `SettingsRun::Full` | Catches a hand-edited or pulled lock |
| `settings::apply` | Last step of `install_all_with_progress`, under `SettingsRun::Full` | Other artifacts still install on a refusal |

On a conflict in the second slot, the run returns one synthetic `ArtifactInstall` with `Err` and
does no artifact work, so `finish` gives 78. `Settings` leaves the per-artifact work list.

**Seats.** `install_and_persist` and `install_all_with_progress` take a required `SettingsRun<'_>`,
so a seat cannot forget it. `Full(&lock)` plans over the whole lock. `Named` keeps the whole lock
for consent and the conflict check, arms only the closure's packages, and removes a leftover only
inside the previous lock's closure. `Skip` runs no settings step.

| Seat | Lock it installs | `SettingsRun` |
|---|---|---|
| `install.rs:119`, a bare `grim update` | The full lock | `Full` |
| A named `grim update`, TUI Update | The closure's projection | `Named { lock, install, prune }`, the closure and the previous lock's closure |
| `add.rs:718`, TUI Install | A projection | `Full(&full_lock)` |
| `install.rs:365`, `update.rs:600`, `tui/app.rs:3135` | A dev synthetic lock | `Skip` |

**Generic removal seams.** `prune_orphans`, `reap_dropped_clients`, `reap_relocated_roots` and
`retired_outputs` skip the kind. The list is closed and a unit over settings records pins it,
because these seams delete through `remove_entry`, which splits two segments and knows no
`element`. `refuse_uninstallable_fallback` skips the kind too, so a settings-only lock with no
detected client reaches the zero-applicable row instead of exit 78.

**The conflict check.** It loads layers of armed packages only, through `fetch_verified_layer`, the
one way the phase loads a layer, so the digest compare, `check_required_features` and the edge
check still run though the kind left the work list. A flag-off or gated package's layer is not
fetched, so a fresh clone exits 0; its manifest edge check still runs, as for hooks. Past the flag,
`add` treats the added package as armed, including under `--no-install`. A package that arms later
meets the install slot. A package whose layer cannot load is left out of the comparison, and the
phase then fails its row and writes nothing. A conflict is:

- one vendor pointer with different values in two packages;
- one pointer a strict prefix of another (`/permission` as a string against `/permission/bash`);
- Codex `default_permissions` in one package with `sandbox_mode` or `sandbox_workspace_write` in
  another.

Documents are compared, not this machine's clients, so every machine agrees. The result is
`SettingsConflict`, exit 78, no reason slug, no `--force`.

**Offline.** The lock proves nothing about values, so:

| State | Result |
|---|---|
| Every settings record is at its locked pin, outputs intact, and every selected client has an output | No-op. No layer is needed; an earlier run proved the plan |
| Only orphans differ from the records | Removal runs; no layer is needed |
| A selected client has no output | Plan from the layer. Uncached and offline: warn, write nothing, exit 0, because nothing the lock proved has changed |
| Pin set changed or a piece drifted, layers cached | Normal run, including the conflict check |
| Pin set changed or a piece drifted, a layer uncached | Settings rows fail 81, no setting is written, other artifacts install |

**The rule, per piece.** F is the file's value, L the lock's, h the recorded hash of any record's
output with this piece's identity. `adopted` is true only if all those outputs are adopted. So a
second package declaring a value grim wrote does not turn it into an adopted value. **First match
wins, in the order 7, 3d, 3c, 3a, 3b, 2, 1, 5, 4.**

| # | Condition | Action | `adopted` after | Listed |
|---|---|---|---|---|
| 1 | L declares, F absent | Write | false | `added` |
| 2a | F = L, no record | None | true | No |
| 2b | F = L, hash(F) = h | None | kept | No |
| 2c | F = L, hash(F) ≠ h | None; record hash(L) | true | No |
| 3a | L differs, hash(F) = h, either flag | Write | false: grim owns it now | `changed` |
| 3b | L dropped, hash(F) = h, not adopted | Remove | gone | `removed` |
| 3c | L dropped, adopted, any F | Leave the value, drop the output, report `abandoned_entries` | gone | No |
| 3d | L dropped, F absent | Drop the output | gone | No |
| 4a | L differs, no record or hash(F) ≠ h | `overwrite`. `--force`: write | false | `overwritten` |
| 4b | L dropped, not adopted, hash(F) ≠ h | `overwrite`. `--force`: remove | gone | `removed` |
| 5 | A different type at the path or at an ancestor; L declares, or L dropped and not adopted | As row 4, naming the blocking ancestor. `--force` replaces it. With L dropped there is nothing to write: `--force` drops the output and leaves the blocking value, because that value is not grim's and the file does not change | false, or gone when L is dropped | `overwritten`; none when L is dropped |
| 6 | One piece, several declarers | One decision, recorded on every declarer. It stays while a record that survives this run declares it. A record this run drops, or rewrites without the piece, does not survive. Removal consults post-run records only, so it works offline. A gated package's record survives, so its piece stays | as above | once |
| 7a | File unparseable, a piece adds or changes | Whole phase refuses: 65, no reason, not forceable; the file is never replaced | unchanged | No |
| 7b | File unparseable, only removals | Drop the record, report `abandoned_entries` | gone | No |
| 8 | No L, no record | Never a piece. User keys and elements are never compared | n/a | No |

> **Amendment to rule row 6 (lock pin stability, RF5, 2026-11-06; fix round 2026-11-07).** The
> Accepted row stands for every run that reads the whole lock. A named `grim update` judges only
> its closure and prune (Decision 5, `Named` seat), so a piece that only leaving records hold gets
> a holder outside the closure:
>
> - **An armed package outside the closure that declares the piece.** The piece gets no verdict.
>   The leaving record keeps it on its previous pin, and a warning names `grim install`, which
>   hands it over. That install carries `--trust-workspace` when only the flag armed the declarer.
>   This holds for a package the run prunes, and for a lone armed package, whose layer the pass
>   reads store first.
> - **An armed package outside the closure whose layer cannot be read** (fix round). Nothing says
>   whether it declares the piece, so the pass stops before any write and keeps the value. The run
>   exits with the read's own code, `81` offline, as c5e3824's whole-lock run does. A warning names
>   the piece and `grim install`. This replaces the RF5 row's "a layer that does not load protects
>   nothing".
> - **A package consent leaves out** protects nothing.
>
> A named `--force` reaches the closure and its prune, the file prune's own reach. A refused value
> of a pruned package names `grim uninstall --force settings <name>` in its refusal line, not a
> rerun with `--force`. A value that record keeps beside it is named too, since that uninstall
> removes it.

A set element has no "different value": it is present or absent, so row 4 reaches it only through
row 5. All judging happens before any write. Any row 4, 5 or 7a outcome refuses the whole phase,
across all packages. `abandoned_entries` is a report field on `update` and `uninstall`, gaining
`element`, always present and `null` when `None`; `install` warns instead.

**Lifecycle mapping.**

| Event | Behaviour |
|---|---|
| Package drops a piece | Rows 3b, 3c, 4b |
| Lock drops a package (`remove`, a pulled lock) | Orphan record, planned as removal on every path, including `install`. No consent needed (Thread), and listed |
| `grim uninstall settings x` | No consent, works with the flag off. Routes through `settings_writer`, never `remove_entry`. Pre-flight lists all drifted pieces, new `UninstallError::SettingsDrifted`, 65 `modified`, and `--force` removes them. Adopted intact: left, `abandoned_entries`. A drifted adopted piece refuses, and `--force` removes it, as `uninstall.rs:239,351` do. A piece another surviving record declares is never removed or listed; only this package's output goes |
| `reap_dropped_clients`, MCP entries | Consults `adopted`: intact removed, adopted left and named in `abandoned_entries`, drifted `kept_modified_clients`. A separate `fix:` commit. It never sees the kind |
| Client set grows | New pieces, through rows 1 and 2 |
| Client set shrinks | `update` calls `settings_phase::reap_clients` at the slot `update.rs:262` uses, keeping its gate on an explicit `[options].clients`. Rows 3b, 3c and 4b under the Decision 7 table, through `settings_writer` |
| Vendor root relocated | Rows 1 and 2 apply at the new root. The value at the old root stays, and `abandoned_entries` names it when the record can still resolve it |
| Gated or flag-off package | Its pieces are not planned: no write, no removal (Decision 7) |

**`--force`.** `ForceScope::All` forces every piece; `None` none. Under `Only(nodes)`, a piece is
forced if any declaring package is in the set. `add --force` cannot reach an orphan, whose key comes
from no lock entry. A remaining `overwrite` refuses the whole phase. Then `refusals_outside_force`
matches the new outcome and names the pieces.

**The refusal.** `InstallOutcome::RefusedSettings { pieces }` on each settings row, then
`InstallErrorKind::SettingsRefused { pieces }`, exit 65, `forceable`. The reason is `modified` if
any piece has a record, else `untracked-destination`. Each piece carries its own `state`, so a mixed
list loses nothing. An unparseable file wins: reason none, not forceable, with `pieces` still attached.

**JSON.** `error_document` gains an optional `pieces` key inside `error`, because the top level is
pinned to that one key. It is omitted when absent, so every other error stays byte-identical. It is emitted whichever refusal `finish` kept first. `grim update` emits its
report with exit 65, so `UpdateReport` carries `pieces` too, always present.

**Text.** One stderr line per piece, for example `overwrite claude ~/.claude/settings.json /model
(team-defaults): modified`.

**The old value** appears only in `pieces[].current`, per the Decisions. It is bounded to 200
characters, has control characters escaped (CWE-150), and has URL userinfo masked, since a user's
file may hold what a package may not.

**Partial failure.** All-or-nothing holds for plan refusals. A mid-apply I/O error or race leaves
earlier files written and recorded (the `record_partial_pass` shape), and a retry converges. Each
file is written, then its record persisted. A crash between them yields adoption, the safe
direction.

### Decision 6: the record store (one-way door for path and envelope)

`$GRIM_HOME/settings/<workspace-key>.json` holds an envelope `{ v, workspace, records }`, with
`deny_unknown_fields`, `serde_repr` version and no field defaults. `records` are existing
`InstallRecord`s. The directory is 0700 and the file 0600. `workspace_key` stays in `hook_dispatch`
and `settings_store` calls it, as consent does; moving it buys only a name.

`InstallState` keeps one map, and every consumer keeps its one `&InstallState`. It gains
`settings_store: SettingsStore`.

- **Load (`load_project`).** Parse `state.json`, including the legacy fallback. Discard every `kind
  == Settings` record with a warning. That covers a file delivered by `git pull`. Then apply the
  kind-independent rule below. Then run `validate_grim_home`. If it passes, read the store and
  merge. If it fails, the state is `SettingsStore::Refused`.
- **Kind-independent rule (premise 24).** A filter on one kind is not enough. On load, an output its
  record's kind may not legally address is dropped with a warning. Legality reads the kind's anchor
  table and the vendor's own surfaces, never the path alone, because vendor files overlap. An
  ordinary record keeps every output. This is a `fix:` commit ahead of the feature.
  Three settings cases are withheld, not dropped:
  1. an OpenCode output that fails only on its file name (`$OPENCODE_CONFIG` moved);
  2. a settings output of a global state carried from another host (a `$GRIM_HOME` synced or
     copied across machines) whose client has no settings file on this host. Warp read on Windows
     is the case today. The anchor is not compared: a variable or Cursor's existing-file order
     moves a client's file between roots on one host, so an output whose path below its anchor
     names this host's file stays visible. A Warp output carried between macOS and Linux therefore
     stays visible at the other platform's root, a file that Warp does not read there;
  3. every settings output of a global state under a refused `GRIM_HOME`.

  The value stays in the file, no consumer reads the output, and the next save keeps it on its
  record. A host that resolves the file again reads the output back.
- **Save (the one chokepoint, also reached by mid-run persist).** `state.json` gets all but
  `Settings`, so it is byte-identical for a project without this kind. The store gets `Settings`. No
  settings records: the file is removed.
- **Unusable file.** A parse failure, a newer `v`, or `workspace` differing from the resolved
  workspace (compared as `consent.rs:193` does) reads as absent, with a warning. The first save
  keeps the old bytes as `<key>.json.unusable`, so a newer format is never destroyed silently. It
  is one slot: a second unusable file replaces it.
- **Losing the store** has the Thread's cost: earlier writes read as adopted, and a rule a package
  dropped in that gap stays in the file, unreported.

**Global scope** stays in `$GRIM_HOME/state/global.json` (`SettingsStore::Inline`). An older grim
reaching that file fails with **exit 74**, not 78: the unknown kind fails the state parse. A config
or lock with the kind gives 78 first, when one is read first. The line to correct is
`stability.md:450`. This is the honest behaviour and needs no code. It reaches only a user who
downgrades after a global settings install.

### Decision 7: consent (one-way door for key prefix, flag pair, verb)

**Key.** `settings:<binding>@<registry>/<repository>`, built by `consent_key_for(kind, binding,
source)`. Hook keys are unchanged. A settings binding passes `SkillName`, which holds no `:`, and a
hook key for a valid binding cannot start `settings:`, so the namespaces cannot meet.
Older grims treat it as opaque. No format change, per the Decisions.

**Path sources.** `verdict` answers `None`: no pinned registry, no key, never applied, even under
the flag, as for hooks. A repository can therefore not ship a settings file beside its config and
have it apply. Authors test through `grim build` and a local registry. The row is `not-armed`,
`not-registered`.

**The ladder.** `ConsentPolicies { hooks, settings }` builds two `HookPolicy` values from one read
of one record, one flag and one interactivity answer. Each kind answers its own feature flag first;
the rest of the ladder is unchanged.

**Carrier.** `InstallTarget` gains `settings_policy: Option<HookPolicy>` and `with_settings_policy`,
beside `hook_policy` (`target.rs:54`). `None` fails closed: the phase plans nothing, as a read-only
command neither arms nor reaps. Seats: `install.rs`, `update.rs`, `add.rs`, and
`tui/app.rs:2871,3023,3131`, where `tui_hook_policy` returns `ConsentPolicies` and `declared_hooks`
gives way to `declared_consent_keys`. `grim uninstall` attaches none: removal needs no consent.

- `declared_consent_keys(lock, flags)` is the union over kinds whose flag is on.
- The prompt fires if either kind reaches `ConsentRequired`. Accepting records the union. Turning
  the second flag on later reads as `consent-drifted`.
- `verdict` stays per binding, so a record not covering a package gates that package while covered
  ones converge.

**The prompt.** One function, `prompt_for_workspace`, with a noun slot: `Hooks`, `Settings`, or
`Hooks and settings`. A hooks-only workspace keeps the fragments pinned by `test_hook_arming.py:504`
and `test_hooks_boundary.py:444`. A settings clause reads: "Settings change how your AI client
behaves and what it may do." "It grants nothing to any other checkout" stays true (Decision 9).

**The flag.** `--trust-workspace` / `--no-trust-workspace` on `install`, `update` and `add`.
`--trust-hooks` / `--no-trust-hooks` become clap aliases of the same arguments, so pytest pins keep
passing. `HookTrustOpts` keeps its shape and `overrides_with` retargets. No env form, by the same
rule as hooks.

**The verb.** `#[command(visible_alias = "consent")]` on the `Hook` subcommand gives `grim consent
allow|revoke|list`.

- The subcommand count stays 26, and `Command::Hook` keeps telemetry `hook/allow` by variant. One
  test row is appended at `command_name.rs:144-147`.
- `HookConsentReport` is unchanged. Its `hooks` array holds prefixed entries.
- `allow` records both kinds regardless of the flags: it is the review gesture.

**Revoke.** It removes the record for both kinds. Applied values stay. A declared package goes
`gated`: no writes, no removals. The undo is `grim uninstall settings x`. The prompt's "Declining
arms nothing and changes no file" stays literal: a decline or `--no-trust-workspace` skips the whole
settings phase, orphans included.

| Run | Consented packages | Unconsented packages | Orphan records |
|---|---|---|---|
| Flag off | Untouched | Untouched | Untouched |
| Declined, or `--no-trust-workspace` | Untouched | Untouched | Untouched |
| Non-interactive, absent or drifted | Converge | `gated` | Removed, or refused by row 4b |
| Accepted, or `--trust-workspace` | Converge | Converge | Removed, or refused by row 4b |

**Gesture.** `grim add` is the gesture and global scope is consented, as for hooks. A `settings:`
key joins `consentable` in `resolve_for_add`. `disclose_bundle_hooks` becomes `disclose_transitive`,
at warn level, naming settings packages that arrive through a bundle or `requires`.

> **Proposed amendment to Gesture, awaiting the owner (lock pin stability, RF5, 2026-11-06; fix
> round 2026-11-07).** It is goal-loop decision RB-4 of the plan, not an owner decision, and it
> sits in the trust narrowing the Proposed block under `adr_artifact_trust_model.md` decision 1
> lists. Until the owner ratifies it, the Accepted text above is the decision; the shipped code
> follows this proposal. A carried settings target consents only while its pin checks out, as a
> carried hook's does (`kept_pin_check`, plan RB-4): its tag still names the pin's digest, or for a
> tagless pin its repository still serves the digest. Otherwise the add consents to nothing for it,
> and one warning says the tag now names another digest than the pin the add kept. It names
> `grim update <name>`, which moves the pin or says why it cannot, and then `grim consent allow`,
> which takes the lock as it stands (RF8). The fix round changed
> that name from `grim hook allow`, the same command, to the spelling settings refusals use.

**`GRIM_HOME` guard.** The phase calls `validate_grim_home` directly before reading any consent or
record. A refusal means `not-armed`, exit 0, causes `grim-home-relative`, `grim-home-in-workspace`
or `grim-home-shared`, and no writes. The gap for a `GRIM_HOME` inside a different repository is
inherited, and named in the threat table.

**Global root fence (plan PD-28, PD-33).** The vendor variables that place a global settings file
are environment, which a repository can set. So at global scope the phase declines a settings file
before reading it when its config directory is relative (`settings-root-relative`) or lies inside
the work tree the command runs in (`settings-root-in-workspace`). That tree is the outermost
ancestor of the working directory holding a `.git` directory, or a regular `.git` file with a
`gitdir:` line, that does not hold `$HOME`. So a nested repository fences its superproject, and a
tree holding `$HOME`, where every default root lies, is exempt. A `.git` of another type (a FIFO,
a device) marks nothing and is never opened. A file is inside by its own path or through a link on it. A path with a `..`
component is refused while a tree is fenced, since grim cannot place it outside, and an unreadable
working directory refuses every global root. The warning says which of these four placed the file
inside, and the remedy follows that evidence: for a link, run grim from outside the repository. A
refusal means `not-armed`, exit 0, no write, and a recorded value withheld with its pin. Each run
names every refused selected client once per cause. No reader opens a fenced file: one predicate
judges each recorded settings output by its recorded path before any read, for the survey,
`status`, the TUI and the search badge, whether its client is selected or only still recorded, and
the `status` row's `arming` names every client the fence refuses. A row whose record holds a
refused client's output reads `skipped`. The recorded edges are in the threat table's T3 row.

### Decision 8: status and reports (one-way door for keys)

**Tokens.** `gated` and `not-armed` extend to the kind, and their "Hook only." docs widen. Rows
reuse `HookArming` through a new `settings_arming(...)`; `hook_arming` is untouched. Each indicator
keeps its Decisions meaning: `stale` first, then a whole-artifact gate, then `derive_state`'s
lifecycle answer. `FeatureFlagOff`, `WorkspaceNotConsented` and `ConsentDrifted` keep their
`state()`.

| Indicator | A settings row reads this when |
|---|---|
| `installed` | Every recorded output is intact and the record is at the locked pin |
| `outdated` | Outputs are intact and the record's pin differs from the lock's |
| `modified` | A recorded output's hash no longer matches the file |
| `missing` | No record exists, or a recorded value was deleted |
| `stale` | The lock no longer matches `grimoire.toml` |
| `gated` | Flag off, workspace not consented or drifted, or zero applicable pieces |

New cause literals are additive:

| Cause | State | Meaning |
|---|---|---|
| `no-applicable-settings` | `gated`, only when zero pieces apply across all selected clients; otherwise a note on the lifecycle state | For example, only Codex keys at project scope |
| `client-config-absent` | Per-client note | Cursor's file does not exist |
| `local-file-tracked` | Advisory, never moves the row | `.claude/settings.local.json` is tracked |
| `settings-root-relative` | `not-armed`, per client, global scope only | The client's global settings file hangs off a relative config directory (Decision 7, root fence) |
| `settings-root-in-workspace` | `not-armed`, per client, global scope only | The client's global settings file lies inside the repository the command runs in, by its own path, through a link or by a `..` spelling, or the working directory cannot be read (Decision 7, root fence) |

`outputs_pending` stays `[]` for the kind, because status holds no document. A status row's
`outputs` gain `element`, always present.

**Zero applicable pieces.** The phase writes a zero-output record, the install row is `skipped`, a
warning names the package, and the exit is 0. Without a record, status would report `missing` and
could never settle.

**`AlreadyTracked`.** The settings phase reports `local-file-tracked` as an install warning, and
status probes through `is_tracked`. The hook path's `warn!` stays.

**The change list.** `SettingsChange { package, client, file, key, element, action }`, `action` one
of `added`, `changed`, `removed`, `overwritten`. It holds no values, old or new. Element text is the
identity, and it is the I5 evidence.

- Reports: always-present top-level `settings_changes` on `InstallReport`, `UpdateReport`,
  `AddReport` and `UninstallReport`. Top level, because an orphan removal has no row; `[]` when
  empty, because `src/api/` bans `skip_serializing_if`. `{"items": […]}` readers see one extra key.
- `install_added` returns the list, since it discards its report today. `UninstallResult` gains
  `settings_changes`, because uninstall runs no install pipeline.
- Text: the phase emits one stderr `tracing::warn!` line per change at apply time, so it survives
  `finish` dropping the report, then one closing line that a running client may need a restart
  (Decision 11). A file shows under `~`, unless a work tree around the command holds `$HOME`: then
  the line shows the link-resolved absolute path, so it does not hide that the file lands there.
- The TUI sends tracing to a log file, so it shows the same list in its own result panel.
- JSON on a later refusal: `finish`, and `update` on a hard `Err`, wrap the error in
  `Error::Settings`. The error document gains `settings_changes` beside `pieces`.
- Telemetry: the kind literal only. `declared_identifiers` adds settings. No pointer, element or
  value is ever emitted.

### Decision 9: Claude project scope (one-way door for the path)

Project scope writes `.claude/settings.local.json`, never `settings.json`.

**Linked worktree: the worktree's own file.** Real 2.1.317 reads both files, the main checkout
winning on a shared key, and reads a worktree-only file. Writing the main checkout's file follows
Claude's docs. But it writes outside the workspace, shares one file between per-worktree records and
consents, and falsifies the prompt's "It grants nothing to any other checkout".

Limits, named: a main-checkout scalar shadows the worktree's value, lists from both apply, and this
depends on Claude reading "a file an earlier version left there". A new watchlist row and the
`upstream-refresh` check track it. A later move is a layout move under Principle 9.

**Confinement.** Every project write passes `confined_target(surface, link_confinement(workspace,
scope))`. A refusal is `not-armed`, `not-registered`, exit 0.

**Ignore rule.** The phase calls `ensure_settings_local_excluded`. `drop_settings_local_exclude`
runs only when neither hooks nor settings use the file, with both callers checking. `git_info_dir`
must resolve the common directory (premise 15), as a separate `fix:` commit. One `info/exclude` then
serves every worktree, so a linked worktree never drops the line, and each run's `ensure` restores
it for a sibling. The rule is hygiene, not a control.

Copilot's `.github/copilot/settings.local.json` stays out: not auto-ignored. Copilot also reads
`.claude/settings*.json` for `extraKnownMarketplaces`, so one Claude write reaches both. The
reference page says so.

### Decision 10: own a file (one-way door for the file name)

`codex.rules` is a typed array, global only, and the one row that is not a `config.toml` key. Each
entry holds `pattern` (a non-empty string list), `decision` (`allow`, `prompt`, `forbidden`) and
optional `justification`. `match` and `not_match` stay off: their failure behaviour is unverified.
`pattern` is deliberately narrower than Codex's grammar, which also allows a list of alternatives in
one position.

The file is `<codex-root>/rules/grim-<binding>.rules`. The `grim-` prefix can never name Codex's
`default.rules`, and the binding grammar is the traversal control. The piece is the whole file, with
a byte hash, under the same rule table. `adopted` is emitted on this file output too.

The renderer is one closed function. Control characters are refused at validation, and
`starlark_string()` escapes `\` and `"`, so a value cannot close a string or add a statement. That
is the T1 injection control. No other Starlark is emitted. A real-binary test runs `codex execpolicy
check --rules <file>`. One file per binding means no cross-package conflict, and Codex takes the
most restrictive decision. A unit parses every rendered file back and compares it with the input,
over a hostile corpus. Whether one malformed `.rules` file disables a client's other rules is not
stated upstream, so a recorded real-binary run settles what the reference page says.

### Decision 11: harness writes between read and write

`settings_writer::apply` re-reads immediately before the splice and re-judges every piece against
the plan. On a mismatch without `--force` it aborts that file with `SettingsRaced`: exit 75,
reason `locked`, retryable, the shape Cline's lock already reports. A file that no longer parses
aborts with `SettingsBecameUnparseable`, even under `--force`: exit 65, no reason, because a rerun
refuses that file too, so it is data, not a race (plan PD-32). Both apply to every vendor.
`--force` skips the comparison, never the parse.

The remaining window is re-read to rename: a harness write there can be lost. No sidecar lock goes
beside a user's file: harnesses do not honour one, and Cline is the only vendor with a documented
lock.

A running harness may hold its own copy of the file. It may miss the write until it restarts, and
its own later save may drop grim's values. Grim assumes neither. A dropped value reads `missing`,
and the next `grim install` restores it through row 1 with no `--force`. The reference page states
each harness's reload behaviour after checking it upstream.

## Component contracts

```rust
// src/oci/settings.rs (new)
pub const SETTINGS_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.settings.v1+json";
pub const SETTINGS_LAYER_SIZE_LIMIT: u64 = 64 * 1024;
pub enum ValidationMode { Author, Consume }
pub struct SettingsDocument { /* MCP descriptive fields */ pub vendors: BTreeMap<String, Map<String, Value>> }
impl SettingsDocument {
    pub fn from_toml_str_with_requires(raw: &str) -> Result<(Self, Vec<BundleMember>), SettingsError>;
    pub fn to_layer_bytes(&self) -> Result<Vec<u8>, SettingsError>;
    pub fn from_layer_bytes(bytes: &[u8]) -> Result<Self, SettingsError>; // caps, parses, validates Consume
    pub fn validate(&self, mode: ValidationMode) -> Result<Vec<SettingsWarning>, SettingsError>;
}
pub enum SettingsError { // all 65; UnknownField under Consume carries unsupported-artifact-feature
    UnknownField { vendor, pointer }, UnknownVendor { vendor }, WrongType { vendor, pointer, expected },
    NotInEnum { vendor, pointer, allowed }, NullValue { pointer }, DatetimeValue { pointer },
    FloatValue { pointer }, IntegerRange { pointer }, ExpansionToken { pointer },
    ExclusiveKeys { first, second }, CredentialInUrl { pointer }, RulesControlCharacter { index },
    TooLarge { actual: u64, limit: u64 }, Malformed { detail },
}

// src/install/vendor.rs: additive `Vendor` methods; defaults keep other vendors unchanged
pub enum MergeClass { Object, Set, Map, Scalar, OwnFile }
pub enum SettingsScopes { Both, GlobalOnly }
pub enum SettingsValueType { Bool, String, Enum(&'static [&'static str]), StringList,
    OneOf(&'static [SettingsValueType]), EnumMap(&'static [&'static str]) /* object replaced whole, keys free, values from a closed set; OpenCode's pattern objects */,
    Atomic { keys: &'static [&'static str] } } // closed first level; inner values checked for null and floats
pub struct SettingsField { pub pointer: &'static str /* an angle-bracketed segment such as `<name>` marks a map key */, pub ty: SettingsValueType,
    pub merge: MergeClass, pub scopes: SettingsScopes, pub map_keys: Option<&'static [&'static str]>,
    pub project_values: Option<&'static [&'static str]>, pub required_by_client: bool /* never removed */ }
fn settings_fields(&self) -> &'static [SettingsField] { &[] }
fn settings_config_path(&self, workspace: &Path, scope: ConfigScope) -> Option<PathBuf> { None }
fn settings_config_format(&self) -> McpConfigFormat { McpConfigFormat::Json }

// src/install/install_state.rs: `current_hash` keeps its signature and dispatches on `kind`
pub struct ClientOutput { /* existing fields */ pub element: Option<String> }
pub enum SettingsStore { Inline, Workspace(PathBuf), Refused }
pub fn is_present(&self, kind: ArtifactKind, roots: &AnchorRoots, containment: Containment) -> …; // gains `kind`
pub fn settings_value_hash(value: &serde_json::Value) -> Digest;

// src/install/settings_store.rs (new)
#[serde(deny_unknown_fields)]
struct SettingsRecordFile { v: SettingsRecordVersion, workspace: PathBuf, records: Vec<InstallRecord> }
pub(crate) fn path(grim_home: &Path, workspace: &Path) -> PathBuf;
pub(crate) fn load(path: &Path, workspace: &Path) -> Vec<InstallRecord>; // unusable: empty plus a warning
pub(crate) fn save(path: &Path, workspace: &Path, records: &[&InstallRecord]) -> io::Result<()>;

// src/install/json_splice.rs, toml_splice.rs (new; `split_pointer` untouched)
pub enum PointerRead { Absent, Found(Value), Blocked { ancestor: Vec<String> } } // Blocked: row 5
pub fn split_pointer_path(pointer: &str) -> Option<Vec<String>>; // RFC 6901
pub fn value_at(text: &str, path: &[String]) -> io::Result<PointerRead>; // Err: unparseable, row 7
pub fn upsert_member_at(text: &str, path: &[String], value: &Value) -> io::Result<Splice>;
pub fn remove_member_at(text: &str, path: &[String]) -> io::Result<Splice>;
pub fn upsert_array_element_at(text: &str, path: &[String], element: &str) -> io::Result<Splice>;
pub fn remove_array_element_at(text: &str, path: &[String], element: &str) -> io::Result<Splice>;

// src/install/settings_writer.rs (new)
pub enum SettingsOp { Upsert { path, value }, Remove { path }, AddElement { path, element },
    RemoveElement { path, element }, PruneEmpty { path, required /* the walk stops at these pointers */ } }
pub struct Expected { pub path: Vec<String>, pub element: Option<String>, pub hash: Option<Digest> }
pub fn apply(path: &Path, format: McpConfigFormat, ops: &[SettingsOp], expected: &[Expected], force: bool)
    -> Result<(), SettingsWriteError>; // one re-read, one re-judge, one atomic_write_through_symlink
pub enum SettingsWriteError { Raced { file }, Unparseable { file } /* even under force */, Io { file, source } }

// src/install/settings_phase.rs (new)
pub(crate) fn preflight(lock: &GrimoireLock, input: &SettingsInput<'_>) -> Result<(), crate::error::Error>;
    // 78 on conflict; a layer failure defers to the phase
pub(crate) fn apply(lock: &GrimoireLock, input: &SettingsInput<'_>, force: &ForceScope,
    state: &mut InstallState) -> SettingsPhaseResult;
pub(crate) fn reap_clients(desired: &[ClientTarget], input: &SettingsInput<'_>,
    state: &mut InstallState) -> SettingsPhaseResult;

// src/install/target.rs
pub fn with_settings_policy(self, policy: HookPolicy) -> Self; // None: the phase plans nothing

// src/install/installer.rs, install_error.rs, uninstall.rs
pub enum SettingsRun<'a> { Full(&'a GrimoireLock), Skip }
pub struct InstallRun { pub items: Vec<ArtifactInstall>, pub settings: SettingsOutcome }
#[derive(Default)] pub struct SettingsOutcome { pub changes: Vec<SettingsChange>, pub pieces: Arc<[RefusedPiece]> }
InstallOutcome::RefusedSettings { pieces: Arc<[RefusedPiece]> }
pub fn install_all_with_progress(/* existing */, settings: SettingsRun<'_>) -> InstallRun;
pub async fn install_and_persist(/* existing */, settings: SettingsRun<'_>) -> Result<InstallRun, InstallFailure>;
// InstallFailure is an InstallError plus the changes already made, so a failed run still lists them.
InstallErrorKind::SettingsRefused { pieces: Arc<[RefusedPiece]> }       // 65, reason by precedence, forceable
InstallErrorKind::SettingsConflict { conflicts: Vec<RefusedPiece> }     // 78, no reason
InstallErrorKind::SettingsUnparseable { client: String, path: PathBuf } // 65, no reason, no file content
InstallErrorKind::SettingsRaced { path: PathBuf }                       // 75, locked, retryable (PD-32)
InstallErrorKind::SettingsBecameUnparseable { path: PathBuf }           // 65, no reason, not forceable (PD-32)
InstallErrorKind::InvalidBindingName { kind, reason }                   // 65: a settings binding outside the name grammar, met at install
UninstallError::SettingsDrifted { pieces: Vec<RefusedPiece> }           // 65, modified, forceable
pub struct AbandonedEntry { /* existing */ pub element: Option<String> } // always in JSON, null when None
pub struct UninstallResult { /* existing */ pub settings_changes: Vec<SettingsChange> }

// src/error.rs: transparent, so `{err:#}` and the exit code do not change
pub struct SettingsFailure { pub error: Error, pub outcome: SettingsOutcome } // Display, source forward to `error`
Error::Settings(Box<SettingsFailure>) // #[error(transparent)]
// `classify` moves its match to `classify_variant(&Error)`; the new arm recurses on `error`.
// `finish` and `update` wrap only when `outcome` is non-empty, so every other run takes today's path.

// src/api/: field order frozen; `file` is null on a conflict row, which is about documents
pub struct SettingsChange { pub package: String, pub client: String, pub file: String, pub key: String,
    pub element: Option<String>, pub action: SettingsChangeAction } // added|changed|removed|overwritten
pub struct RefusedPiece { pub package: String, pub client: String, pub file: Option<String>, pub pointer: String,
    pub element: Option<String>, pub marked: PieceMark /* overwrite|conflict */,
    pub state: Option<PieceState> /* modified|untracked-destination */,
    pub wanted: Option<String>, pub current: Option<String> }
pub struct UpdateReport { items: Vec<UpdateEntry>, settings_changes: Vec<SettingsChange>, pieces: Vec<RefusedPiece> }

// src/hook/
pub fn consent_key_for(kind: ArtifactKind, binding: &str, source: &LockedSource) -> Option<String>;
pub struct ConsentPolicies { pub hooks: HookPolicy, pub settings: HookPolicy }
pub enum ConsentNoun { Hooks, Settings, HooksAndSettings }
pub fn prompt_for_workspace(workspace: &Path, record: &Path, new: &[String], noun: ConsentNoun) -> String;
```

| Situation | Exit | Reason | `forceable` | Text marker |
|---|---|---|---|---|
| Overwrite pieces, any with a record | 65 | `modified` | true | `overwrite` |
| Overwrite pieces, none with a record | 65 | `untracked-destination` | true | `overwrite` |
| Unparseable file on add or change, alone or with overwrite pieces | 65 | none | false | names the file |
| Document invalid, or unknown field at install | 65 | none, or `unsupported-artifact-feature` | false | names the pointer |
| Conflict across packages | 78 | none | false | `conflict` |
| Harness wrote during the run, the file still parses | 75 | `locked` | false | retryable |
| Harness wrote during the run, the file no longer parses | 65 | none | false | names the file; `--force` does not pass it |
| Binding name outside the grammar, met at install | 65 | none | false | names the kind |
| Layer uncached offline, plan needs it | 81 | none | false | names the package |
| Registry failure loading a layer | 69 | none | false | names the package |
| Registry auth failure loading a layer | 80 | none | false | names the package |
| Store or state write failed | 74 | none | false | names the file |
| Flag off, gated, guard or root fence refused | 0 | n/a | n/a | status cause |

## Threat table

Scoped to `arch-threat-model.md`.

| Ref | Exposure | Control | Residual |
|---|---|---|---|
| T1 | A publisher ships permissive settings or a hostile rule | Digest pin; closed typed list with no hooks, env, endpoints or credentials; `{env:` refusal; closed Starlark renderer; consent at project scope | **Deliberate deviation:** no re-ask on a version bump. A consented publisher can widen a later version. Evidence, not prevention: lock diff and change list (I5). Deviation 5 sits here too: turning the flag off does not undo a bad package's values |
| T2 | A tag moves | The layer is fetched by the lock's digest | Consent keys carry no digest; the digest half of I2 still holds |
| T3 | A cloned repository declares settings or forges a record | Flag and consent gate every write; `.grimoire/state.json` settings records are discarded and every output is checked against what its kind may address; the record lives in `$GRIM_HOME`; `GRIM_HOME` guard; path sources never apply; `confined_target`; at global scope the root fence (Decision 7) declines a settings file whose config directory is relative or inside the outermost work tree around the command that does not hold `$HOME`, by its own path, through a link or by a `..` spelling, and an unreadable working directory refuses every global root (`settings-root-relative`, `settings-root-in-workspace`) | **Deliberate deviations:** a project `.cursor/cli.json` or Qoder project file can shadow a global rule, and `GRIM_HOME` inside a different repository passes the guard (inherited gap). A vendor root set to an absolute path inside a different repository passes the root fence too, and a root in a dotfiles repository is refused while the command runs there (false positive). Two more fence edges are recorded, not fixed (plan PD-33), both CWE-15: a `HOME` the repository sets drops the fence, since the exemption reads `$HOME` from the environment and a trusted home source needs `getpwuid_r`, which `unsafe_code = "forbid"` rules out; and a checkout with no `.git` (an archive, jj, hg, Sapling, or a `.git` file without `gitdir:`) is not fenced. Deviation 13 sits here too: the flag is project-settable, so a repository can turn it on, but a settings key still needs a human's `allow` |
| T4 | An injected agent runs `grim add`, or `--trust-workspace` | I5 evidence: change list, lock diff, `grim status`; prompts need a terminal; a flag cannot be set by a file | **Deliberate deviation:** `grim add` is a gesture even when an agent makes it. The agent holds the user's privilege, and grim cannot tell them apart |
| T5 | A local reader sees state | Store is 0600; no credential on the list; `current` masks URL userinfo; nothing in argv or env | Element text sits in the record, at 0600 |
| N1 | A teammate drops or bumps a package | Non-goal: a reviewed commit. The change list shows it | Removal needs no consent |
| N2 | A same-privilege process edits the store | Non-goal: a forged record cannot overwrite a differing value, since the file decides | Evidence only |
| N3 | A hostile harness | Non-goal | n/a |
| N4 | `--force`, `--trust-workspace` in the user's own CI | Non-goal: consent means the user may consent | n/a |
| N5 | A slow or wrong setting the user chose | Non-goal | n/a |
| I1 | The record authorises removal, so it is armable | Named in I1: it lives under `$GRIM_HOME`. Values reach only the per-developer local file at project scope | Values are repo-resident there, as hook registrations are |
| I2 | Consent keys on a name | Recorded exception extended, with the same two halves | The T1 residual above |
| I3 | Grim failure read as a verdict | Gated and guard-refused rows exit 0 with a cause; a real failure exits non-zero | n/a |
| I4 | Default-deny | Experimental flag off by default, consent on by default at project scope | Global scope is consented, as for hooks |
| I5 | Evidence | Change list on every changing run, including a teammate's bump through `grim install` | n/a |
| I6 | Secrets | No secret is read, passed or stored | n/a |

## Quantified Impact

| Metric | Before | After | Notes |
|---|---|---|---|
| Artifact kinds | 6 | 7 | Appended |
| Subcommands | 26 | 26 | `consent` is an alias |
| `ErrorReason` slugs | 9 | 9 | No new slug |
| `HookArmingCause` tokens | 18 | 23 | Additive: the five Decision 8 causes |
| Typed fields | 0 | 90 | Six vendors |
| Writes per touched file per run | n/a | 1 | Never one per element |
| Bytes changed in a project without the kind | n/a | 0 for lock and state | Reports gain `settings_changes: []` |

## Consequences

**Positive:**

- One rule and one check serve install, status, the TUI and uninstall.
- A user's value is never silently replaced, and a forged repository record does nothing.

**Negative:**

- A hand edit on a declared key makes `grim install` exit 65 until the user acts: the Thread's accepted cost.
- Flag-off and revoke leave values in place, and a linked worktree on Claude can be shadowed.

**Risks:**

- Vendor key churn. Mitigation: the experimental flag, the `upstream-refresh` check, and
  Consume-mode refusal naming a grim update.
- Claude drops the "earlier version" read. Mitigation: a watchlist row; a move is a layout move.
- Merge regressions. Mitigation: one writer, and acceptance tests replaying the archaeology's
  failure modes.

## NFR coverage

| Attribute | Coverage |
|---|---|
| Reliability | Plan refusals are all-or-nothing, retries converge, and a crash leaves adoption |
| Performance | Layers load only for armed packages. With one armed package and every selected client covered, a run reads no layer. With two or more, an install pass reads each layer once, since a package recorded while another was gated must still meet the cross-package conflict check: the pass's preflight hands the layers it read to the apply step. `add` and `update` read every armed layer once more, in the conflict check they run before the lock is saved (Decision 5), so they read each layer twice and a warning a layer gives can print twice. A package with no piece for a selected client costs one layer read per run. One write per file |
| Offline | No-op with intact records, locked pins and covered clients. Orphan removal needs no layer. A changed plan with an uncached layer is 81 for the settings rows only |
| Observability | `grim status` causes, change list, `debug` span per file; telemetry stays key-free |
| Portability | `atomic_write_through_symlink` keeps symlinked dotfiles; the Windows `GrimHomeShared` cause applies |

## Migration and rollout

**Release.** `Cargo.toml` is 0.15.1 and dependencies are unreleased, targeting 0.16.0. This ships in
the next unreleased minor, after dependencies land, behind the experimental flag. No migration is
needed: nothing existing moves.

**Principle 9 matrix:**

| Contract | Change | Older grim meets it | Evidence |
|---|---|---|---|
| Lock | `[[settings]]` when non-empty | 78 | `deny_unknown_fields` on `RawLock` |
| Config | `[settings]` table | 78 | Same |
| Config | `[options.experimental] settings` | 78 | `ExperimentalOptions` denies unknown keys |
| Layer | New media type, annotation | `kind_from_manifest` gives `None` for add and search | Existing path |
| Bundle | Settings member | 65 | Premise 9 |
| `requires` | Settings edge | 65 | Premise 9 |
| `global.json` | Settings records, `element` | **74** when reached | Decision 6 |
| `.grimoire/state.json` | Never holds the kind | Unchanged | Decision 6 |
| `$GRIM_HOME/settings/` | New directory | Ignored | No reader |
| Consent record | `settings:` keys, no format change | Opaque in `evaluate`. `hook allow` drops them, so a newer grim re-prompts | Fail-safe |
| Reports | `settings_changes`, `pieces` keys | n/a: emitted by the new binary | Additive; plain output unchanged |
| Error document | `pieces`, `settings_changes` when present | n/a | Omit-when-absent |
| Schemas | Additive kind and table | n/a | Additive |
| Status | New cause literals; two tokens widen | Consumers see an unknown literal | Paired extension release |
| Cursor anchors | `cursor-cli-root`, `cursor-cli-xdg-root` | Only in `global.json`, already 74 | Append-only tags |
| Antigravity and Warp anchors (#270) | `antigravity-cli-root`, `warp-config-root` | Only in `global.json`, already 74 | Append-only tags (`SHIPPED_ANCHOR_TAGS` in `path_anchor.rs`) |
| Project without the kind | None | Identical | Upgrade fixture |
| `.grimoire/state.json` load | An output its kind may not address is dropped, for any kind | Unchanged | Ordinary records keep every output; a fixture pins it |
| `reap_dropped_clients` | An adopted MCP entry stays on a dropped client, named in `abandoned_entries` | Removes it | A shipped exit-0 behaviour changes, so `upgrading.md` calls it out |
| `git_info_dir` | The rule moves to the common `info/exclude` and covers every worktree; an old per-worktree line stays inert | Writes the per-worktree file | `upgrading.md` note |
| JSON vendor files grim splices (MCP, hooks, `claudeMdExcludes`, settings) | A leading byte-order mark is read past and stays first, so the file is edited and read | Refuses the file, exit 74 (v0.15.1) | A shipped exit code changes for the released kinds, so `upgrading.md` calls it out |

**Unstable while the flag exists.** `[settings]`, `[[settings]]`, the experimental key, the document
grammar, `grim schema --kind settings`, `--trust-workspace`, the `consent` alias,
`settings_changes`, `pieces`, the store envelope and the typed key list. `stability.md:24` gains this
as a third exception, and `:204-205` stops calling the flag pair hook-only. They stay additive by
choice.

**Upgrade fixtures:** a project with no settings kind (lock and state byte-identical), a forged
committed `state.json` including a record of each other kind, with a positive control that an
ordinary MCP record survives, an old consent record with hook keys, and `test_old_grim_settings.py` under
`GRIM_OLD_BIN`, local opt-in.

**Separate commits ahead of the feature:** `fix:` `git_info_dir` common directory; `fix:`
`reap_dropped_clients` consults `adopted`; `fix:` the state load rule (premise 24).

**Docs that change** (loop deliverables, none in this change): `settings.md`,
`guides/share-settings.md`, `guides/dependencies.md`, `commands.md`, `json-interface.md`,
`artifacts.md`, `clients.md`, `concepts.md`, `stability.md` (including the 74 line), `upgrading.md`,
the catalog drift review, `AGENTS.md` environment rows, subsystem rules, the watchlist, and
`upstream-refresh` files.

**For the meta-orchestrator:** the follow-up issue from Appendix B; index, `grim search` and VS Code
extension work on the kind; a paired extension release.

## Convention deviations, named

1. The consent record's `hooks` field and `HookConsentReport.hooks` hold settings keys.
2. The plain consent header still reads "Hooks", and `consent list` lists hooks only.
3. `HookPolicy`, `HookArming`, `HookArmingCause` and `McpConfigFormat` serve settings under old
   names. A rename would touch frozen report fields and buys only a name.
4. `codex.rules` is not a `config.toml` key, the one non-native name in a "native keys" document.
5. Flag-off leaves applied values, while hooks disarm. Settings have no dispatcher to stop.
6. Revoke does not undo values; `grim uninstall` does.
7. `settings:` keys extend the I2 name-key exception to a second kind.
8. `adopted` is emitted on a file output, not only an entry output.
9. `entry` has two grammars by kind.
10. `test_c153_config_list_adds_the_experimental_hooks_row` and the `ALL` index tests extend by
    appending. "No other row is added" no longer holds.
11. `--trust-workspace` reads oddly at global scope, where it can only deny.
12. `InstallOutcome`'s return wrapper changes an internal signature. No public API exists.
13. `grim hook allow` records settings keys even with the settings flag off.

## Open questions

- Closed by the plan (PD-1): Cursor's documented path order between `$CURSOR_CONFIG_DIR`, `~/.cursor`
  and XDG is unverified. Use the existing-file order in Decision 3: grim only edits a file that
  exists, so a wrong order finds the file Cursor would also rewrite, and a watchlist row re-checks it.
- Closed by the plan (PD-2): `grim status` does not detect a main-checkout scalar shadowing a
  worktree's value. It would need a second parse of a file outside the workspace, and the reference
  page plus the watchlist row cover the first slice.

### Discussion questions closed

| # | Question | Resolution |
|---|---|---|
| 1 | Behind `[options.experimental]`? | Yes (Decision 1) |
| 2 | Named `settings` or `config`? | `settings` (Decision 1) |
| 3 | Does `--trust-hooks` answer for this kind? | Yes, alias of `--trust-workspace` (Decision 7) |
| 4 | Which fields per harness? | Appendix A, with Appendix B as the follow-up |
| 5 | Does `enabledPlugins` join? | No (Decision 3) |
| 6 | Other marketplace keys? | Copilot and Codex have one; Cursor, OpenCode and Qoder do not |
| 7 | Which file in a linked worktree? | The worktree's own (Decision 9) |
| 8 | A harness writing between read and write? | Re-read, exit 75, no sidecar (Decision 11) |

## Links

- [`config-artifact-type.md`](../discussions/config-artifact-type.md): ratified source
- [`adr_artifact_trust_model.md`](./adr_artifact_trust_model.md),
  [`adr_hook_workspace_consent.md`](./adr_hook_workspace_consent.md): amended here
- [`adr_artifact_dependencies.md`](./adr_artifact_dependencies.md),
  [`adr_agent_artifact_kind.md`](./adr_agent_artifact_kind.md),
  [`adr_render_layout_stability.md`](./adr_render_layout_stability.md)
- [`arch-threat-model.md`](../../.claude/rules/arch-threat-model.md),
  [`arch-principles.md`](../../.claude/rules/arch-principles.md)
- [Kubernetes Server-Side Apply](https://kubernetes.io/docs/reference/using-api/server-side-apply/),
  [RFC 6901](https://www.rfc-editor.org/rfc/rfc6901)

---

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-11-04 | Michael Herwig + Claude (architect) | Initial draft, **Proposed**. Amends trust ADR decision 2, the consent ADR, and invariants I1 and I2. |
| 2026-11-04 | Claude (plan review panel) | Review fixes: `SettingsRun` seats, closed removal skip list, kind-independent state load rule, row 6 and rule order, client coverage offline, policy carrier, read path, Unstable list, two pure Cursor rows. |
| 2026-11-04 | Claude (plan re-validation) | **Accepted**, per the plan's PD-15. Requires `install_and_persist` to return `InstallRun`, and places `pieces` inside the error document's `error` object. |
| 2026-11-05 | Claude (closing-review fix pass) | Decision 6 withholds a third case: a settings output carried from another host whose file this host does not resolve. The Principle 9 matrix names the two anchors #270 appended. Appendix B gives Copilot `trustedFolders` and Cursor `autoAcceptWebSearch` a verdict, and the trust-gate rule names its four keys. The threat table's T3 row gains the global root fence and its two first edges (plan PD-28), and the Performance row says when a layer is read (PD-30). |
| 2026-11-05 | Claude (closing review 3 fix pass) | Decision 7 describes the root fence as it now stands: the outermost work tree, a regular `.git` only, a `..` spelling refused, an unreadable working directory failing closed, and evidence-led remedies. Decision 8 lists the fence's two causes, and the token count is 23. Decision 11, the exit table and the contracts block take PD-32: `SettingsRaced` (75), `SettingsBecameUnparseable` (65), and `InvalidBindingName` at install (65). The T3 row records two more fence edges, a repository-set `HOME` and a checkout with no `.git` (PD-33). The Performance row names the second layer read on `add` and `update`. The Principle 9 matrix records the byte-order mark a JSON vendor file may now start with. |
| 2026-11-06 | Claude (lock pin stability, RF4) | Decision 5's seat table gains `Named`, per the pin stability plan's RD4-12. A named `grim update` and TUI Update arm only the closure's packages: writes, restores, the edited-value refusal, `--force`, pin moves and rows stop there, and the prune stays at the previous closure. Consent, the conflict check, the removal guard and one verdict per shared piece still read the whole lock. Residual: an outside package with no record does not hold a value the closure drops, and `grim install` writes it back. |
| 2026-11-06 | Claude (lock pin stability, RF5) | Amendment as landed, superseding the RF4 row's residual (plan RD4-14 and the RF5 pass). Rule row 6 gains a holder on a named run: a piece that only leaving records hold, which an armed package outside the closure declares, gets no verdict. The value stays, the leaving record keeps it on its previous pin, and a warning names `grim install`, which hands it over. This holds for a package the run prunes too, and for a lone armed package, whose layer the run reads store first since the conflict check reads none. Only a package consent leaves out, or a layer that does not load, protects nothing. A named `--force` reaches the closure and its prune, the file prune's own reach, and a refused value of a pruned package names `grim uninstall --force settings <name>`. Gesture (Decision 7): a carried settings target consents only while its pin checks out, as a hook's does (`kept_pin_check`, plan RB-4). The amendment blocks after the rule table and after Gesture carry this text. |
| 2026-11-07 | Claude (lock pin stability, RF5 fix round) | Both amendment blocks gain the fix round. An armed package outside a named closure whose layer cannot be read now stops the pass and keeps the value, exiting with the read's code (`81` offline), as c5e3824 does; this replaces the RF5 row's "a layer that does not load protects nothing". The kept-value remedy repeats `--trust-workspace` when only the flag armed the declarer. A pruned package's refusal names its forced uninstall and the value it also removes. The kept-pin warning names `grim consent allow`. |
| 2026-11-07 | Claude (lock pin stability, RF6) | The amendment to Gesture is marked Proposed, awaiting the owner: it is goal-loop decision RB-4, part of the trust narrowing the trust ADR's Proposed block lists, and the Accepted text stays the decision until the owner ratifies it. No behaviour changed. |

---

## Appendix A: first-slice key list

Pointers are RFC 6901 into the vendor's native file. An angle-bracketed segment such as `<name>` is a map key. Status is `on` or
`on-limited`. Scopes: `both` or `global`.

Files: Claude `settings.json` under `$CLAUDE_CONFIG_DIR`, or `.claude/settings.local.json` at
project scope (JSON). Codex `$CODEX_HOME/config.toml` (TOML). Copilot `$COPILOT_HOME/settings.json`
(JSONC). Cursor `cli-config.json` (JSON). OpenCode `opencode.json` or `.jsonc`. Qoder
`$QODER_CONFIG_DIR/settings.json` (JSONC).

| Vendor | Pointer | Value type | Merge class | Scopes | Status |
|---|---|---|---|---|---|
| claude | `/model` | string | scalar | both | on |
| claude | `/effortLevel` | enum `low`, `medium`, `high`, `xhigh` | scalar | both | on |
| claude | `/outputStyle` | string | scalar | both | on |
| claude | `/language` | string | scalar | both | on |
| claude | `/spinnerTipsEnabled` | bool | scalar | both | on |
| claude | `/alwaysThinkingEnabled` | bool | scalar | both | on |
| claude | `/includeGitInstructions` | bool | scalar | both | on |
| claude | `/permissions/allow` | string list | set | both | on |
| claude | `/permissions/deny` | string list | set | both | on |
| claude | `/permissions/ask` | string list | set | both | on |
| claude | `/permissions/defaultMode` | enum `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`, `manual`; project: first three | scalar | both | on |
| claude | `/sandbox/enabled` | bool | scalar | both | on |
| claude | `/extraKnownMarketplaces/<name>` | atomic `{source: {source: "github", repo, ref?}}` | map | both | on |
| codex | `/model` | string | scalar | global | on |
| codex | `/model_reasoning_effort` | string, not enumerated | scalar | global | on |
| codex | `/approval_policy` | enum `on-request`, `never` | scalar | global | on |
| codex | `/sandbox_mode` | enum `read-only`, `workspace-write`, `danger-full-access` | scalar | global | on |
| codex | `/sandbox_workspace_write/network_access` | bool | scalar | global | on |
| codex | `/sandbox_workspace_write/writable_roots` | string list | set | global | on |
| codex | `/sandbox_workspace_write/exclude_slash_tmp` | bool | scalar | global | on |
| codex | `/sandbox_workspace_write/exclude_tmpdir_env_var` | bool | scalar | global | on |
| codex | `/default_permissions` | string | scalar | global | on |
| codex | `/permissions/<name>` | atomic `{description, extends, filesystem, network, workspace_roots}` | map | global | on |
| codex | `/tui/alternate_screen` | enum `auto`, `always`, `never` | scalar | global | on |
| codex | `/tui/animations` | bool | scalar | global | on |
| codex | `/tui/auto_recap` | bool | scalar | global | on |
| codex | `/tui/raw_output_mode` | bool | scalar | global | on |
| codex | `/history/persistence` | enum `save-all`, `none` | scalar | global | on |
| codex | `/hide_agent_reasoning` | bool | scalar | global | on |
| codex | `/marketplaces/<name>` | atomic `{source, source_type, ref, sparse_paths}` | map | global | on |
| codex | `rules` (typed, own file) | list of `{pattern, decision, justification?}` | own-file | global | on |
| copilot | `/model` | string | scalar | global | on |
| copilot | `/effortLevel` | enum `low`, `medium`, `high`, `xhigh` | scalar | global | on |
| copilot | `/theme` | enum `default`, `github`, `dim`, `high-contrast`, `colorblind` | scalar | global | on |
| copilot | `/defaultMode` | enum `interactive`, `plan`, `autopilot` | scalar | global | on |
| copilot | `/defaultPermissionMode` | enum `manual`, `assisted`, `allow-all` | scalar | global | on |
| copilot | `/permissions/disableBypassPermissionsMode` | enum `disable` | scalar | global | on |
| copilot | `/allowedUrls` | string list | set | global | on |
| copilot | `/deniedUrls` | string list | set | global | on |
| copilot | `/disabledMcpServers` | string list | set | global | on |
| copilot | `/disabledSkills` | string list | set | global | on |
| copilot | `/askUser` | bool | scalar | global | on |
| copilot | `/includeCoAuthoredBy` | bool | scalar | global | on |
| copilot | `/respectGitignore` | bool | scalar | global | on |
| copilot | `/renderMarkdown` | bool | scalar | global | on |
| copilot | `/stream` | bool | scalar | global | on |
| copilot | `/transcriptView` | enum `default`, `concise` | scalar | global | on |
| copilot | `/mergeStrategy` | enum `rebase`, `merge` | scalar | global | on |
| copilot | `/sandbox/enabled` | bool | scalar | global | on |
| copilot | `/extraKnownMarketplaces/<name>` | atomic `{source, autoUpdate}` | map | global | on |
| cursor | `/permissions/allow` | string list; required pointer | set | global | on-limited |
| cursor | `/permissions/deny` | string list; required pointer | set | global | on-limited |
| cursor | `/approvalMode` | enum `allowlist`, `auto-review`, `unrestricted` | scalar | global | on |
| cursor | `/sandbox/readBoundary` | enum `system`, `workspace` | scalar | global | on |
| cursor | `/editor/vimMode` | bool; required pointer | scalar | global | on |
| cursor | `/notifications` | bool | scalar | global | on |
| cursor | `/hints` | bool | scalar | global | on |
| cursor | `/rewind` | bool | scalar | global | on |
| cursor | `/suggestNextPrompt` | bool | scalar | global | on |
| cursor | `/maxMode` | bool | scalar | global | on |
| cursor | `/display/showLineNumbers` | bool | scalar | global | on |
| cursor | `/display/showThinkingBlocks` | bool | scalar | global | on |
| cursor | `/display/showStatusIndicators` | bool | scalar | global | on |
| cursor | `/display/showStatusLineRunningTime` | bool | scalar | global | on |
| cursor | `/attribution/attributeCommitsToAgent` | bool | scalar | global | on |
| cursor | `/attribution/attributePRsToAgent` | bool | scalar | global | on |
| opencode | `/model` | string | scalar | global | on |
| opencode | `/small_model` | string | scalar | global | on |
| opencode | `/default_agent` | string | scalar | global | on |
| opencode | `/autoupdate` | bool or `"notify"` | scalar | global | on |
| opencode | `/snapshot` | bool | scalar | global | on |
| opencode | `/compaction/auto` | bool | scalar | global | on |
| opencode | `/compaction/prune` | bool | scalar | global | on |
| opencode | `/permission` (string form) | enum `allow`, `ask`, `deny` | scalar | global | on |
| opencode | `/permission/<tool>` | enum `allow`, `ask`, `deny`, or a pattern object compared whole; tool is one of `read`, `edit`, `glob`, `grep`, `bash`, `task`, `todowrite`, `skill`, `lsp`, `question`, `webfetch`, `websearch`, `external_directory`, `doom_loop` | map | global | on |
| qoder | `/model/name` | string | scalar | global | on |
| qoder | `/general/defaultPermissionMode` | enum `default`, `accept_edits`, `plan`, `auto`, `bypass_permissions`, `dont_ask` | scalar | global | on |
| qoder | `/general/vimMode` | bool | scalar | global | on |
| qoder | `/general/enableAutoUpdate` | bool | scalar | global | on |
| qoder | `/general/enableNotifications` | bool | scalar | global | on |
| qoder | `/output/format` | enum `text`, `json` | scalar | global | on |
| qoder | `/ui/theme` | string | scalar | global | on |
| qoder | `/ui/hideTips` | bool | scalar | global | on |
| qoder | `/ui/hideBanner` | bool | scalar | global | on |
| qoder | `/ui/showLineNumbers` | bool | scalar | global | on |
| qoder | `/ui/compactToolOutput` | bool | scalar | global | on |
| qoder | `/permissions/allow` | string list | set | global | on-limited |
| qoder | `/permissions/ask` | string list | set | global | on-limited |
| qoder | `/permissions/deny` | string list | set | global | on-limited |
| qoder | `/security/disableYoloMode` | bool | scalar | global | on |

Where this differs from the research: Claude `/theme`, `/attribution` and Codex
`/tui/notification_*` and `/file_opener` move off, because their value sets are not shown and a
schema-rejected value can skip a whole file. OpenCode's two `/permission/<tool>` rows are one row
with a union type.

### Rows added under #270 (2026-11-05)

[#270](https://github.com/grimoire-rs/grimoire/issues/270) added 226 rows, 316 in all. The full,
test-pinned key list is `docs/src/content/docs/settings.md#fields`, so the table below counts rows
and names only the `on-limited` ones. Every new harness is global only. Rows, schema keys and roots
were appended, none renamed or removed (Principle 9). Evidence: the
`research_upstream_*_settings_20261105.md` artifacts.

| Vendor | File and format | Root | Rows added | Scopes | `on-limited` |
|---|---|---|---|---|---|
| claude | existing files | existing | 10 (Appendix B) | both | `/sandbox/filesystem/allowWrite`, `/allowRead` |
| codex | existing file | existing | 4 (Appendix B) | global | none |
| copilot | existing file | existing | 42 (Appendix B) | global | none |
| opencode | existing file | existing | 1 (Appendix B) | global | none |
| qoder | existing file | existing | 1 (Appendix B) | global | none |
| antigravity | `settings.json`, JSON | `antigravity-cli-root` (new, `~/.gemini/antigravity-cli`) | 25 | global | `/permissions/allow`, `/ask`, `/deny` |
| amp | `settings.jsonc` when it exists, else `settings.json`, JSONC | `amp-root` | 8 | global | none |
| droid | `settings.json`, JSON | `droid-root` | 30 | global | `/sessionDefaultSettings/reasoningEffort`, `/sandbox/filesystem/allowWrite`, `/sandbox/network/allowedDomains` |
| kilo | `kilo.jsonc`, else a lone `kilo.json`, JSONC | `kilo-config-root` | 9 | global | none |
| kiro | `settings/cli.json`, strict JSON, flat dotted keys | `kiro-root` | 34 | global | `/knowledge.defaultIncludePatterns`, `/knowledge.defaultExcludePatterns` |
| gemini | `settings.json`, JSON, shared with the MCP entry | `gemini-root` | 33 | global | `/tools/allowed`, `/tools/confirmationRequired`, `/tools/sandboxAllowedPaths` |
| warp | `settings.toml`, TOML 1.1 | `warp-root` on macOS, `warp-config-root` (new, XDG `warp-terminal`) on Linux, none elsewhere | 29 | global | none |

Cursor gained no row. Agents, Cline, Junie, Zed, OpenClaw and Goose stay without rows; the
re-check trigger for each is in `.claude/skills/upstream-refresh/references/domains.md`. A key
that switches off a harness's workspace or folder trust gate, or pre-trusts a folder, stays off,
whatever its Routing line says (threat model T3). Four such keys are known: Gemini
`/security/folderTrust/enabled`, Antigravity `/trustedWorkspaces` and Kiro
`chat.disableTrustAllConfirmation` (each routed off in its harness's 2026-11-05 research artifact),
and Copilot `trustedFolders` (Appendix B). A value
holding an expansion token is refused at build: `{env:` and `{file:` for OpenCode (existing) and Kilo
(PD-35), `$NAME` and `${NAME}` for Gemini (PD-37).

## Appendix B: follow-up issue list

Off the list. The meta-orchestrator files these.

[#270](https://github.com/grimoire-rs/grimoire/issues/270) settled every item on 2026-11-05. The
evidence for each outcome is in `.agents/research/research_upstream_appendix_b_settings_20261105.md`
(Claims, Live CLI check, Routing). 58 rows shipped, all on the existing files and scopes: Claude 10,
Codex 4, OpenCode 1 and Qoder 1 in step 3a of P-270 (`plan_settings_follow_ups.md`), Copilot 42 in
step 3b, Cursor none. The middle column keeps the first-slice reason.

| Item | Why it was off (2026-11-04) | Outcome under #270 |
|---|---|---|
| Claude `/theme`, `/editorMode`, `/tui` | Value format not shown | Shipped in step 3a: three enum rows, both scopes. `/theme` leaves out `custom:` themes, which name a theme file a package cannot ship. |
| Claude `/attribution` | Sub-key shapes not shown | Shipped in step 3a: `/attribution/commit` and `/attribution/pr` (string), `/attribution/sessionUrl` (bool), both scopes. A user's `attribution: false` blocks all three pointers, so a write there refuses and `false` stays without `--force`. |
| Claude `/sandbox/filesystem/*` | List merge across files not stated | Shipped in step 3a: `allowWrite`, `denyWrite`, `denyRead`, `allowRead` as set rows, both scopes. The sandboxing page now states the merge: paths combine across files. `allowWrite` and `allowRead` are `on-limited`: an admin-required sandbox ignores them in a project's local file, and a relative path resolves per file. |
| Codex `/tui/notification_condition`, `/tui/notification_method`, `/file_opener` | Value sets not listed | Shipped in step 3a: three enum rows, global. The config reference and schema list the values, and `codex debug prompt-input` accepted each one and rejected the controls. |
| Codex `/approval_policy` granular table | Table shape not enumerated | Still off. grim's types express the outer union, not the inner table. A missing required inner key or a non-bool fails Codex's whole config load, and a misspelled inner key is ignored, so a typo goes unseen (both verified). Widening the shipped enum row would also change a first-slice row. |
| Codex top-level `/web_search` | Undefined on the page | Shipped in step 3a: enum `disabled`, `cached`, `indexed`, `live`, global. `live` loosens, and an admin requirement can refuse a value. |
| Codex rules `match`, `not_match` | Behaviour on an invalid example unverified | Still off. Now verified: one wrong example fails the whole policy load, other rules files included. `Atomic` cannot check an example against its pattern. Shipping it needs `grim build` to evaluate the examples: new code and an owner decision. |
| Copilot tool rules, and the final 26k characters of the config page | No key found; page unread | The page was read to its end as raw markdown. Tool rules stay off: they are managed keys only, and `copilot config permissions.allow` is refused as organization policy, so no user key exists. The page yields 42 rows, shipped in step 3b: 7 enum, 33 bool and 2 string-list set rows, global. The keys it leaves off are in the next table. |
| Cursor `/sandbox/mode`, `/sandbox/networkAccess` | Values not enumerated | Still off. `sandbox.mode` lists no values for the stored key; `enabled` and `disabled` are documented for the `--sandbox` flag only. `sandbox.networkAccess` has display labels, not stored strings. |
| Cursor `/model` | Shape unverified, CLI-managed | Still off: the object shape is not given, and the CLI manages it. |
| Cursor `autoAcceptWebSearch` | Not on the list: the #270 research found it in the CLI changelog (v2026.07.13) | Off. The configuration reference (fetched 2026-11-05) does not list the key. The changelog line names neither its type nor its pointer in `cli-config.json` (top level or nested; the `/config` toggle sits under Permissions), and no Cursor binary runs credential-free to check a value. It also loosens a gate: web searches run without approval. Re-check when the reference lists it with a type. |
| OpenCode `/compaction/reserved`, `tui.json` keys, `/share`, `/username` | Type or page unread | `/share` shipped in step 3a as enum `manual`, `disabled`. `auto` is left out: it shares every session, a data egress only the user may turn on. `/compaction/reserved` stays off: deprecated, and an integer (grim has no number type). `tui.json` keys stay off: they live in a second file, and `settings_config_path` returns one file per harness, so they need an ADR decision. `/username` stays off: it is the telemetry identity, which is scoped out, and no behaviour default. |
| OpenCode `config.json` as a third file name | Upstream reads it; grim's path code does not | A finding, not a row. OpenCode 1.19.5 reads `config.json` beneath `opencode.json` and `opencode.jsonc` (verified), so a grim write can outrank a user value grim never read. The fix is to read it as a lower layer, or to decline when it sets the pointer. |
| Qoder `/model/reasoningEffort` | Values not listed | Shipped in step 3a: enum of eight values, global. |
| Qoder array merge between project and user files | Its two pages disagree | Still unresolved. Neither page names the merge for `permissions.*`, and the bundled schema does not settle it. The shipped `/permissions/*` rows stay `on-limited`. |
| Every other harness grim supports | Out of the first slice (Requirements) | Settled per harness in the "Settings keys per harness" table of `.claude/skills/upstream-refresh/references/domains.md`: the harness's rows, or none with the reason and the re-check trigger. |

Copilot keys the full page shows that stay off:

| Copilot key | Why it stays off |
|---|---|
| `/tabs/hide` | The elements form a closed set of five tab ids that `StringList` cannot check, and Copilot ignores an unknown id silently, so a typo ships unseen. |
| `/tabs/sort` | An ordered list over the same unchecked ids. |
| `/footer/<item>` | "Sub-keys include" does not claim the list is complete, so a closed key set is unsourced. `/statusline` also manages the object. |
| `/sandbox/userPolicy/network/allowLocalNetwork`, `/allowedHosts`, `/blockedHosts`, `/sandbox/userPolicy/filesystem/deniedPaths` | Kept off by decision in the first slice (field list § 1.3, network hosts). `deniedPaths` also creates missing directories before launch. The values and merge are verified if the owner reverses it. |
| Tool rules `permissions.allow`, `ask`, `deny`, `limitTo` | Managed sources only, refused at user level (row above). |
| `/subagents/agents/<name>` | `Atomic` cannot check the inner enums `modelPolicy` and `contextTier`. |
| `/commandHistoryMaxSize`, `/subagents/maxConcurrency`, `/subagents/maxDepth` | Number values; grim has no number type. |
| `/contextTier` | The reference lists it for repository and managed files only, and `copilot help config` lists it as a user key. The pages disagree, and grim is global-only here. |
| `/remoteSessions`, `/remoteExport` | Remote control and session-history upload: data egress, scoped out like telemetry. |
| `/worktreePathTemplate`, `/skillDirectories`, `/ignoredSkillsLocations` | Filesystem locations and content discovery paths, outside the three categories. |
| `bannerStyle`, `notifications`, `memory`, `inlineImageLiveWindow` | In `copilot help config` only, not on the reference page. Re-check when the page lists them. |
| `trustedFolders` | Off for good, not a candidate. It is the CLI's record of permanently trusted directories, kept in the automatically managed `config.json` (internal state), not in the `settings.json` grim edits. Writing it would pre-trust a folder and skip the trust prompt (threat model T3), as Antigravity `/trustedWorkspaces` would. Sources: the configure-copilot-cli how-to and the config dir reference, fetched 2026-11-05. |

Kept off by decision, with no follow-up: `enabledPlugins` and same-named keys, Claude
`/availableModels`, `/fallbackModel`, `/modelPicker`, Qoder `/permissions/additionalDirectories` and
`/autoMode`, and everything the field list's method note scopes out.
