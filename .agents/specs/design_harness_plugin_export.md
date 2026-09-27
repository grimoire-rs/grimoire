# Design record — harness plugin export (phase 1)

**Companion to** `.agents/adr/adr_harness_plugin_export.md` (decisions D1–D13
and rationale). This file carries **contracts** (C-001…C-036) and **scenarios**
(S-001…S-031) only; each is written so a tester can produce a failing test
without opening the source. **IDs are append-only.**

**Status:** Accepted with the ADR (review round 1 amendments A1–A20 and re-validation fixes R1–R9 applied) · **Date:** 2026-09-27 · **Source:** `.agents/discussions/harness-native-marketplaces.md` (phase 1)

Goal criteria (`.agents/goals/harness-native-marketplaces.md`) cited as tags:
**G1** ADR fixes schema/JSON/exit codes · **G2** export ad-hoc + declared per
`--client`, folder or `--zip` · **G3** rendered like install, byte-reproducible ·
**G4** manifests carry name, version, omissions · **G5** `update --marketplace`
rolls pins, installs nothing · **G6** rename fails closed · **G7** no duplicate
lock/update/render path · **G8** tests, docs, `grim-usage` drift review.

Notation: `M` = a `marketplace.toml`; `L` = its lock; `P` = a plugin name;
`c` = a client; "emitted name" = the member name after rename;
`FetchScope` = `crate::fetch::FetchScope` (`src/fetch.rs:102`), the registry
context returned by `command::resolve_fetch_scope(ctx, global, config,
workspace) -> anyhow::Result<FetchScope>` (`src/command.rs:518`).

---

## A. `marketplace.toml`

### C-001 — `export::marketplace::load(path: &Path) -> Result<MarketplaceManifest, ExportError>` [G1, G2]

```rust
pub struct MarketplaceManifest { pub path: PathBuf, pub plugins: BTreeMap<String, PluginDecl> }
pub struct PluginDecl {
    pub include: Vec<String>,          // non-empty
    pub description: Option<String>,
    pub version: Option<String>,       // leading `v` stripped at load
    pub rename: Option<RenameRule>,
}
pub struct RenameRule { pub strip_prefix: String }   // non-empty
```

- Wire: top-level `[plugins.<name>]` only; `#[serde(deny_unknown_fields)]` at
  every level. Top-level `name`, `owner`, `description` are **reserved**: an
  unknown-key error in phase 1 (the error hint names them as reserved).
- `path` must end in `.toml`, its stem must not itself end in `.toml`, and
  its file name must not be `grimoire.toml`; checked before reading. `path`
  is made absolute at load (anchor = its parent). The ad-hoc in-memory
  manifest's `path` is `<cwd>/<name>.toml` (never read or written; anchor =
  cwd).
- Read via `config::read_capped`. Every failure — bad path name (above), not
  found, oversized, unreadable, TOML syntax, unknown key, type mismatch, empty
  `include`, empty `strip_prefix`, invalid plugin name (C-002), declared
  `version` failing C-023's grammar — returns `ExportError::Manifest { path,
  message }` → **65**, message carrying the path once.
- Zero `[plugins]` tables parse successfully (the "none declared" error is
  C-014's, at selection time).
- `plugins` iterates in byte order of the name (BTreeMap).

### C-002 — Plugin name rule [G1, G4]

`export::marketplace::validate_plugin_name(&str) -> Result<(), String>`: a
plugin name is valid iff `SkillName::parse` accepts it (lowercase ASCII
alnum, `-`/`.` separators, no leading/trailing/consecutive separator, ≤64).
Applied to: `[plugins.<name>]` keys (→ 65), `--name` (→ 64,
`CommandError::InvalidBindingName` parity), the name derived from a single ref
(→ 64 with hint `pass --name`). Edge: `a--b`, `A`, `a:b`, `a/b`, 65-char names
all reject.

### C-003 — Include ref → plugin `DesiredSet` [G2, G7]

`export::resolve::plugin_set(decl, anchor: &Path, ctx: &FetchScope, access) -> Result<DesiredSet, crate::error::Error>`

- Each include string goes through the **extracted `add` declare seam**, the
  function `grim add <reference>` now calls:
  ```rust
  pub(crate) async fn declare_reference(input: &str, overrides: DeclareOverrides /* kind, name */,
      anchors: DeclareAnchors /* cwd: Option, config_dir — absolute */, ctx: &FetchScope,
      access: &(dyn Fn() -> anyhow::Result<Arc<dyn OciAccess>> + Sync))
      -> Result<Declared /* kind, binding, DeclaredSource */, DeclareError>
  ```
  *(Execution amendment:* `access` is a lazy builder, called only on the
  registry branch right after `resolve_reference`, so `grim add` keeps its
  exit ordering; `DeclareOverrides.kind` is `Option<ArtifactKind>`;
  `DeclareAnchors.cwd: None` reads `current_dir()` only for a relative path;
  path errors carry the binding (`PathInvalid { name, value, reason }`).)
  Registry refs: kind from the manifest annotation, binding = last path
  segment. Path sources: kind by shape, binding = the **packed intrinsic
  name** (as `add.rs:463` does), not the last path segment. The `SkillName`
  binding guard (`add.rs:194`, `:467`) moves with it. `DeclareError`
  preserves the underlying cause (not-found, offline miss, access error,
  guard). `grim add` maps every cause exactly as today (unresolved tag,
  missing manifest or kind → `KindInferenceFailed` 65, `add.rs:819-835`;
  transport errors keep their own class; guard → 64) — no expectation edit.
  Export maps causes: tag or manifest not found → **79** (`ExportError::IncludeNotFound`, execution amendment); `Ok(None)` from
  the `Operation::Query` digest lookup under `--offline` → **81** (explicit
  offline check, `add.rs:810` returns `None` for both); no kind annotation
  → `ExportError::Manifest` **65**; binding guard → **65** in declared mode
  (the name comes from a file), **64** in ad-hoc mode (C-002 parity);
  transport/auth → existing 69 / 80.
- Anchors: declared mode `cwd = config_dir = M.parent()`; ad-hoc = the
  current directory.
- Bundles land in `set.bundles`, other kinds in their table, keyed by binding.
- Direct-vs-direct: two includes yielding the same `(kind, binding)` with
  equal `Display` of the expanded identifier (or equal path source) → one
  entry; otherwise `ExportError::MemberConflict { plugin, kind, name, first,
  second }` → **78**, message naming both include strings.
- Direct-vs-bundle, path-vs-bundle: checked after resolution (C-009).
  Bundle-vs-bundle: the resolver's existing `BundleConflict` (78).

**Amended in review (2026-09-27):** ad-hoc mode's `NoKind` failure does not
join the generic "no kind annotation → `ExportError::Manifest` 65" path
above. It maps to `CommandError::KindInferenceFailed` instead — the same
error `grim add` returns for the same cause (plan decision 29). Same exit
code (65, via `classify`'s `CommandError` arm), different type and
message; declared mode is unaffected and keeps `ExportError::Manifest` 65
for a `NoKind` failure.

### C-004 — Marketplace declaration hashes [G1, G7]

`export::marketplace::declaration_hashes(m: &MarketplaceManifest, ctx: &FetchScope) -> Result<DeclarationHashes, ExportError>`
→ `DeclarationHashes { whole: String, per_plugin: BTreeMap<String, String> }`

- Expansion of one plugin: the sorted, deduplicated array of each include's
  expansion — `Identifier` `Display` for a registry ref (via
  `resolve_reference`, no network; a tagless, digestless ref gains `:latest`
  as `grimoire.toml` and `grim add` do; a malformed include → `Manifest` 65), the `PathSource` `Display` prefixed
  `path:` for a local path.
- `per_plugin[P]` = `sha256:<hex>` of RFC 8785 JCS of P's array; `whole` =
  same over the object `{"<P>": [...], …}` keyed by plugin name.
- Constant `MARKETPLACE_HASH_VERSION: u8 = 1` in `src/config/hash.rs`, beside
  and independent of `DECLARATION_HASH_VERSION`; stored as
  `declaration_hash_version`.
- `ctx`: declared mode `resolve_fetch_scope(ctx, false, None, Some(M.parent()))`;
  ad-hoc mode the cwd context (`resolve_fetch_scope(ctx, ctx.global(),
  ctx.config(), None)`).
- **Invariants:** changing `description`, `version` or `rename` changes no
  hash; reordering `include` changes none; adding/removing/retagging an
  include changes that plugin's hash and `whole`, no other plugin's hash;
  adding a plugin changes `whole` only. Same hashes from any cwd. Computable
  with `--offline` and no cache.

---

## B. Lock (shared type, `plugin` scope on the wire)

### C-005 — `plugin` is a wire-only field [G1, G7]

- `LockedArtifact` is **unchanged** in memory (no field, no constructor edit).
- `RawLockedArtifact.plugin: Option<String>` — `#[serde(default)]`,
  `#[schemars(skip)]`. `RawLock` gains `plugin: Vec<RawPluginHash { name,
  declaration_hash }>` (`[[plugin]]`) — `#[serde(default)]`,
  `#[schemars(skip)]`.
- The shared raw parse yields `(Option<String>, LockedArtifact)` pairs plus the
  `[[plugin]]` rows; `plugin` never reaches `LockedArtifact`.
- The marketplace serializer view emits `plugin` immediately after `name` and
  `[[plugin]]` only when non-empty; the `grimoire.lock` serializer
  (`GrimoireLock::to_toml_string`) is untouched.
- **Invariant:** `grimoire.lock` bytes equal the pre-change build's (golden
  test over the existing lock fixtures).

### C-006 — Lock flavor validation [G1]

- `lock_io::load`: any raw entry with `plugin`, or a non-empty `[[plugin]]`
  table → `LockErrorKind::ScopeMismatch { message }` → **78**. Only callers
  that propagate load errors surface it (`install`, `status`, …); `grim
  context` records it in its `lock_error` field and exits 0 (existing design,
  `context.rs:85-97`);
  `update`, `lock` and `add` call `.ok()` (`update.rs:91`, `lock.rs:53`,
  `add.rs:312`) and treat it as an absent lock — existing behaviour,
  unchanged.
- `lock_io::load_marketplace` → `ScopeMismatch` (78) when: an entry lacks
  `plugin`; an entry's `plugin` has no `[[plugin]]` row; `[[bundle]]` is
  non-empty; any `plugin` value, `[[plugin]].name`, or skill/rule/agent entry
  `name` fails `SkillName::parse`; an mcp entry `name` fails the containment
  check (non-empty, no `/`, `\`, `..` or NUL — mcp and bundle bindings are
  exempt from `SkillName` in `grim add`, `add.rs:188-190`). `declaration_hash_version != MARKETPLACE_HASH_VERSION`
  → existing `UnsupportedVersion` (78).
- `save_marketplace` runs the same checks before writing (never write what
  load rejects — `lock_io::save`'s size-check precedent).
- `ScopeMismatch` is classified in `classify_lock` beside `TomlParse`.

### C-007 — `MarketplaceLock` and its IO [G7]

```rust
pub struct MarketplaceLock { pub metadata: LockMetadata, pub plugins: BTreeMap<String, GrimoireLock> }
pub fn load_marketplace(path: &Path) -> Result<MarketplaceLock, LockError>                         // lock_io
pub fn save_marketplace(path: &Path, lock: &MarketplaceLock, previous: Option<&MarketplaceLock>)
    -> Result<(), LockError>                                                                          // lock_io
```

- One part per plugin, keyed by the `[[plugin]]` rows (a part may be empty):
  that plugin's entries, `bundles` empty; its `metadata` is a copy of the
  top-level `[metadata]` (`lock_version`, `generated_by`, `generated_at`,
  `declaration_hash_version`) except `declaration_hash` = the per-plugin hash
  (on load, and when C-009 builds parts).
  `[metadata].declaration_hash` = C-004 `whole`.
- Wire order: `[metadata]`, `[[plugin]]` by name, then each kind array sorted
  by `(plugin, name)`; no `[[bundle]]`.
- Shares the raw parse, serializer, `atomic_write_through_symlink` and the
  size cap with `load`/`save`.
- `generated_at` preserved iff the plugin key sets are equal and every part is
  `content_equal` to its previous part; else bumped exactly as `save` does.
- Property: `load_marketplace(save_marketplace(x, None)) == x` for every valid
  `x`.

### C-008 — `lock_io::content_equal` reused per part [G7]

`content_equal` is untouched and not made scope-aware; `save_marketplace`
calls it once per plugin part. Existing tests unchanged. The same artifact
pinned to different digests by two plugins lives in two parts and is never
conflated.

### C-009 — The single marketplace resolution seam [G5, G7]

```rust
pub enum PluginSelection { All, Some(BTreeMap<String, PluginPick>) }
pub enum PluginPick { Whole, Members(BTreeSet<String>) }
pub struct MarketplaceResolution {
    pub lock: MarketplaceLock,
    pub bundle_pins: BTreeMap<String /*plugin*/, Vec<PinnedIdentifier>>, // in memory only
}
pub async fn resolve_marketplace(
    m: &MarketplaceManifest, previous: Option<&MarketplaceLock>, selection: &PluginSelection,
    ctx: &FetchScope, access: &Arc<dyn OciAccess>,
) -> Result<MarketplaceResolution, crate::error::Error>
```

Lives in `export::resolve`. Steps:

1. Compute C-004 hashes. Every selected plugin name must be in `M`, else
   `SelectorNotFound`/`PluginNotFound` **79**, before any network access.
2. `All` ≡ every declared plugin as `Whole`.
3. `Members` on a plugin whose part is present and stale (C-033) → the
   resolver's `StaleLock` error kind (**65**, `stale-lock`), before any
   `DesiredSet` is built (no network).
4. Per selected P, `set = plugin_set(P)`, then `resolver::roll_forward` (C-034; moved from `update::` in review 2026-09-27):
   `Whole` → `names = []`; `Members(n)` with a fresh part → that part with
   `metadata.declaration_hash := set.declaration_hash_cached()` (freshness
   proven in 3), `names = n`; `Members(n)` with no part → `previous = None`
   (full resolve), then every name in `n` must name an entry of the result,
   else `SelectorNotFound` **79** and nothing is saved.
5. Per resolved part, **before** its `bundles` are cleared: any two entries of
   one `(kind, name)` → `MemberConflict` 78; a direct registry entry whose
   `(kind, name)` matches a member of a bundle snapshot with a different
   expanded identifier → `MemberConflict` 78 naming both sources. Then the
   registry bundle pins go to `bundle_pins[P]`, `bundles` is cleared and the
   part's metadata set per C-007.
   *Amended in review (2026-09-27):* the first clause (two entries of one
   `(kind, name)` in a part) is not a separate check. The resolver's dedupe
   and its `BundleConflict` already guarantee it, so step 5 enforces only the
   direct-versus-bundle-member conflict.
6. Unselected plugins: part carried verbatim from `previous` when present;
   parts of plugins no longer in `M` dropped.
7. Result metadata: `whole` hash, `MARKETPLACE_HASH_VERSION`,
   `generated_by` = current, `generated_at` = now.

Used by export (C-014) and `update --marketplace` (C-011); no other code
calls the resolver for marketplaces.

**Amended in review (2026-09-27):** the shipped signature gained two
parameters the block above does not show — `origin: IncludeOrigin` and
`offline: bool` — threaded down into `plugin_set`/`declare_failure` (plan
decision 26): `origin` chooses the declared-vs-ad-hoc classification
(C-003), `offline` distinguishes a floating-tag miss (`IncludeNotFound`
79) from an offline cache miss (`AccessErrorKind::OfflineMiss` 81).

### C-010 — Lock path and advisory lock [G5]

- Lock path = `M.with_extension("lock")` (`marketplace.toml` → `marketplace.lock`,
  `team.toml` → `team.lock`). C-001's name rule keeps it distinct from `M`,
  from `grimoire.lock` and from the advisory sidecar.
- Any command that may write `L` holds `ConfigFileLock::try_acquire(M)`
  (sidecar `<M>.lock`, e.g. `marketplace.toml.lock`, `file_lock.rs:78`) across
  resolve + write; contention → **75** `locked`.
- `L` is loaded with `load_marketplace` (not-found → absent; every other error
  propagates) and saved with `save_marketplace(path, &lock, previous)`.

---

## C. `grim update --marketplace`

### C-011 — CLI and branch [G5, G7]

`UpdateArgs` gains:

```rust
/// Roll the pins of a marketplace lock forward instead of the project set.
#[arg(long, value_name = "PATH", conflicts_with_all = ["force", "client"])]
pub marketplace: Option<PathBuf>,
```

- `update::run`'s first statement: `if let Some(m) = &args.marketplace { return run_marketplace(ctx, m, &args.names).await; }`.
- `run_marketplace`: reject `ctx.global()` / `ctx.config()` (global flags,
  outside `UpdateArgs`) with `ExportError::Usage` **64**; load M (C-001);
  acquire lock (C-010); load `L`; parse selectors (C-012); registry context
  `resolve_fetch_scope(ctx, false, None, Some(M.parent()))` + `access_seam_scoped(ctx, false, None, Some(M.parent()))` *(execution amendment: the insecure-host set comes from M's directory too)*;
  `resolve_marketplace` (C-009); `save_marketplace`; report (C-013).
- It never calls install scope resolution (`scope_resolution::resolve`),
  `InstallTarget::parse`, the installer, prune, reap, `sync_config` or install
  state (`resolve_fetch_scope` uses `scope_resolution::resolve_in` for the
  registry set only); works without any `grimoire.toml`.

### C-012 — Selector grammar [G5]

`<P>` | `<P>:<member>`; exactly zero or one `:`; both halves non-empty.
No selectors → `PluginSelection::All`. A member selector selects **every
artifact with that lock name in the plugin, across kinds** (mirrors `grim
update <name>` / `resolve_lock_partial`, which match by name). `Whole`
subsumes `Members` for the same plugin.

| Input | Result |
|---|---|
| `team` | `Some{team → Whole}` |
| `team:hex-plan` | `Some{team → Members{hex-plan}}` (lock name, pre-rename) |
| `a b:x` | `Some{a → Whole, b → Members{x}}` |
| `a a:x` | `Some{a → Whole}` |
| `team:` / `:x` / `a:b:c` / empty | **64** |
| `ghost` (not in M) | **79** `SelectorNotFound { selector }`, checked against M before anything else |
| `team:ghost`, team's part present | **79** `SelectorNotFound` (amended in review 2026-09-27: was resolver `TagNotFound`; the resolver's `NotDeclared` maps to the same selector error as the part-absent row) |
| `team:ghost`, team's part or `L` absent | **79** `SelectorNotFound`, checked against the freshly resolved plugin; `L` not written |

**Amended in review (2026-09-27), C-011 and C-012:** the shipped
`run_marketplace` order is M → selectors → advisory lock → `L`, not M →
lock → `L` → selectors as C-011 states above (plan decision 30): a
malformed selector (64) is caught before either the lock or `L` is
touched. Correspondingly, C-012's `ghost` row above ("checked against M
before anything else") holds only for the selector *shape* check; whether
the named plugin is actually declared in M is validated inside
`resolve_marketplace` (step 1 of C-009), which runs after both the
advisory lock is acquired and `L` is loaded (plan decision 35) — an
unknown plugin name still exits 79, just not before those two steps.

### C-013 — `UpdateEntry.plugin` [G1, G5, G7]

`pub plugin: Option<String>` serialized as `plugin`, always present: `null` on
every normal `grim update` row, the plugin name on marketplace rows. Rows are
built by the **existing** `update::build_report`, extended with a `plugin`
argument stamped on every row: `update::run` passes `None`; `run_marketplace`
calls it once per plugin part of the result (new part vs the same plugin's
previous part — a diff keyed on `(plugin, kind, name)`) with empty
pruned/reaped slices, concatenating rows in plugin order. Hence `old`/`new`/
`action` as today and `reaped_clients: []`, `kept_modified_clients: []`,
`retained: []`, `abandoned_entries: []`, `refused: false`. Dropped plugins
produce no rows. Plain table gains no column for normal updates; marketplace
runs prefix `Name` with `<plugin>:`.

---

## D. `grim export plugin`

### C-014 — CLI surface and input matrix [G2]

```rust
pub struct ExportPluginArgs {
    pub refs: Vec<String>,                 // positional, 0..n
    #[arg(long)] pub name: Option<String>,
    #[arg(long = "plugin", conflicts_with = "refs")] pub plugins: Vec<String>,
    #[arg(long, value_name = "PATH", conflicts_with = "refs")] pub marketplace: Option<PathBuf>,
    #[arg(long = "client")] pub client: Vec<String>,
    #[arg(long)] pub zip: bool,
    #[arg(long, short = 'o', value_name = "DIR", default_value = ".")] pub output: PathBuf,
    #[arg(long)] pub version: Option<String>,
    #[arg(long)] pub force: bool,
}
```

| Refs | `--name` | `--plugin` | Behaviour |
|---|---|---|---|
| 1 | absent | — | ad-hoc; name = binding of the ref (C-002) |
| 1 | set | — | ad-hoc; name = `--name` |
| ≥2 | set | — | ad-hoc merged plugin |
| ≥2 | absent | — | **64** "`--name` is required when exporting more than one reference" |
| 0 | set | any | **64** |
| 0 | — | `P…` | declared `P…` from M (`--marketplace` or `./marketplace.toml`); unknown `P` → **79** |
| 0 | — | none | every declared plugin; M declares none → **65** with hint (`--plugin`, `<ref>… --name`) |
| ≥1 | — | set | **64** (clap conflict) |

Ad-hoc: an in-memory `MarketplaceManifest` holding one
`PluginDecl { include: refs, description: None, version: None, rename: None }`
(anchor and registry context = current directory) goes through
`resolve_marketplace(&m, None, &PluginSelection::All, …)` — **no lock file
read or written**, no manifest lock taken.
Declared: `L` loaded (C-010); the exported plugins that are stale (C-033) are
re-resolved via `resolve_marketplace(Some{stale → Whole})`, fresh ones carried
byte-identically; `L` is saved **after** outputs are placed (C-027) when any
plugin was re-resolved or dropped, or `L` was absent.

### C-015 — Client selection and families [G2]

- `target::parse_client_list(values: &[String]) -> Result<Vec<ClientTarget>, InstallError>`:
  comma-split, trim, drop empties, dedupe preserving order, unknown name →
  `UnsupportedClient` (**78**). Extracted from `InstallTarget::parse`, which
  now calls it (no behaviour change).
- Family map (total, `export::family::family_of(c) -> Option<Family>`):
  `claude, droid, junie, openclaw` → `Family::Claude`;
  `copilot, codex, cursor, agents` → `Family::AgentPlugins`; all others → `None`.
- Chain: `--client` → `[options].clients` (project config if a scope resolves,
  else global) → `[agents]`. No detection.
- Explicit client with `None` family → `ExportError::NoPluginFormat { client }`
  → **78**. Config-derived → dropped with stderr note
  `client '<c>' has no plugin format; skipped`; empty after dropping → `[agents]`.

### C-016 — Admission gate and omissions [G4]

`admits(family, client, kind) -> Result<(), OmitReason>`:

| Kind | Claude family | Agent Plugins |
|---|---|---|
| skill | `kind_support != Declined` else `client-declined` | same |
| agent | `kind_support != Declined` else `client-declined` | `no-format-surface` |
| mcp | `kind_support != Declined` else `client-declined`; then C-020 may yield `not-representable` | admitted for every Agent Plugins client (the file shape is the family's, not the client's — no `kind_support` gate); C-036 may yield `not-representable` |
| rule | `no-format-surface` | `no-format-surface` |

`OmitReason` serializes as the kebab literals above. `kind_surface` is never
consulted. Resulting omissions are sorted by `(kind, emitted name)`.
Expected today: droid/openclaw → agents + mcp `client-declined`; junie → agent
`client-declined`, mcp emitted (unless C-020 declines it); claude → all but
rules; copilot/codex/cursor/agents → skills + mcp (unless C-036 declines it).

### C-017 — Staging seam `installer::stage_locked_artifact` [G3, G7]

```rust
pub(crate) struct StagedArtifact { pub dir: tempfile::TempDir, pub canonical: PathBuf, pub support_dir: Option<PathBuf> }
pub(crate) async fn stage_locked_artifact(artifact: &LockedArtifact, kind: ArtifactKind,
    access: &Arc<dyn OciAccess>, anchor: &Path, materializer: &impl ArtifactMaterializer,
    staging_parent: &Path) -> Result<StagedArtifact, crate::error::Error>
```

Extracted verbatim from `install_one` (registry → `fetch_verified_layer`, path
→ `pack_verified_local`, `DefaultMaterializer`, `locate_canonical`, support
dir); `install_one` calls it with `std::env::temp_dir()`.
`installer::fetch_verified_layer` (`installer.rs:1957`) becomes `pub(crate)`;
MCP members go `fetch_verified_layer` → `McpDescriptor::from_layer_bytes`,
as `install_mcp` does (`installer.rs:2110`); `locate_canonical` is never
called for MCP. Errors keep their existing classification (offline miss 81,
digest mismatch 65, …).

### C-018 — Member rendering [G3, G7]

For each admitted skill/agent of plugin `P` and client `c`, with `root` = the
per-output staging root:

- skill → `c.materialize(MaterializeRequest{ kind: Skill, name: emitted, artifact_root: canonical, dest: root/skills/<emitted>, scope: Global, pinned: <provenance>, support_dir: None })`.
- agent (Claude family only) → canonical agent file first passed through
  `render::rebind_agent_name` when renamed (C-019), then
  `materialize(kind: Agent, dest: root/agents/<emitted>.md, scope: Global, …)`.
- `pinned` = `artifact.source.provenance()` (the same string install stamps).
- Never written: `bin/`, rule files, anything outside `root` (C-035).
- **Byte-equality contract:** every file under `skills/<n>/` and
  `agents/<n>.md` equals the file `grim install --client c` writes for the
  same locked member (project scope, or global where the vendor hosts the kind
  only globally — OpenClaw skills), when the member is not renamed.

### C-019 — `render::rebind_agent_name(doc: &str, binding: &str) -> Option<String>` [G6]

Same contract as `rebind_skill_name` but parsing `AgentFrontmatter`: rewrites
only the frontmatter `name`; `None` when the name already equals `binding` or
the document does not parse. Deterministic. Called only by export.

### C-020 — Plugin MCP file assembly [G3, G4]

For admitted MCP members, in emitted-name byte order. **Claude family:**
`(pointer, value) = c.vendor().mcp_entry(Global, emitted, &descriptor)`;
`None`, or a pointer whose container is not `mcpServers`, → omitted
`not-representable`; else `json_splice::upsert_member(&text, "mcpServers",
emitted, &value)` (`json_splice.rs:57`, returns `io::Result<Splice>`) and
apply the returned splice (`Changed(s)` → `text = s`; `Unchanged` → keep),
starting from `text = ""`. Written to `root/.mcp.json` only when ≥1 member
was admitted. Semantic contract: parsed JSON equals the `mcpServers` members
`grim install --client c` writes for the same members (project scope).
Known `None` case: Junie declines a descriptor carrying OAuth
(`vendor_junie.rs:124-127`) or env refs (`:132-137`).
**Agent Plugins:** `value = family::agent_plugins_mcp_entry(emitted,
&descriptor)` (C-036); `None` → omitted `not-representable`; else the same
`upsert_member` assembly under `mcpServers`, written to `root/mcp.json` only
when ≥1 member was emitted. One assembly routine serves both families.

### C-021 — Rename: names [G6]

`export::rename::apply(plugin: &str, members: &[LockedArtifact], rule: Option<&RenameRule>) -> Result<Vec<(LockedArtifact, String /*emitted*/)>, ExportError>`

- No rule → emitted = lock name.
- Name starts with `strip_prefix` → remainder; else unchanged.
- **Every** emitted skill/rule/agent name, renamed or not, must pass
  `SkillName::parse`; an mcp emitted name must pass C-006's containment
  check; an empty or failing name → `RenameInvalid { plugin, from, to }`
  **65**.
- Two members of the same kind with equal emitted names → `RenameCollision { plugin, kind, name, members: [a, b] }` **65**.
- Runs before any fetch; failure writes nothing.

### C-022 — Rename: stale-reference scan [G6]

`export::rename::scan(root: &Path, renamed: &[(String /*old*/, String /*new*/)]) -> std::io::Result<Vec<StaleRef>>`

- Runs on every staged per-client tree after member rendering, before
  manifests are written; skipped when nothing was renamed.
- Walks every regular file under `root/skills/` and `root/agents/` only (not
  `.mcp.json`, not manifests), dotfiles included; skips files that are not
  valid UTF-8; lines split on `\n`. A read/walk failure aborts the export
  before placement → `ExportError::Io` **74**.
- A hit = an occurrence of `old` whose preceding char (if any) and following
  char (if any) are not in `[A-Za-z0-9_-]`.
- Any hit across any client → `RenameStaleReference { plugin, hits }` **65**,
  message lines `<client>: <relpath>:<line>: '<old>'` (relpath `/`-joined from
  the plugin root), sorted, first 50 then `… and N more`.
- Positive cases: `../hex-core/references/x.md`, `run /hex-plan`,
  `` `hex-plan` ``, `see hex-plan.`, `skills/hex-plan/SKILL.md`.
  Negative cases: `grimoire-hex-plan`, `hex-planner`, `hex_plan` (underscore is
  a name char), the rewritten frontmatter line.

### C-023 — Version [G4]

`export::family::plugin_version(base: Option<&str>, members: &[(LockedArtifact, String)]) -> Result<String, ExportError>`

- Base precedence: `--version` → declared `version` / ad-hoc single-ref
  `org.opencontainers.image.version` annotation → `0.0.0`. The annotation is
  read with `access.fetch_manifest(pin)` where `pin` = the single ref's
  bundle pin from `MarketplaceResolution.bundle_pins` (bundle) or the single
  lock entry's pin (non-bundle) — never by re-resolving the floating tag;
  ignored when absent, invalid, or the ref is a path source.
- One leading `v` stripped; must parse via `semver::Version::parse` with empty
  build metadata (pre-release allowed). Single home of this grammar:
  `export::marketplace::normalize_version(&str) -> Option<String>`, used by
  C-001, this function and the annotation filter. An invalid `--version` →
  `InvalidVersion { value }` **65**; an invalid declared `version` already
  failed C-001 (`Manifest` 65).
- Suffix: `+` + first 12 lowercase hex of SHA-256 over the concatenation of
  `"{kind}\t{emitted}\t{digest}\n"` lines sorted bytewise, one per plugin
  member (all members, independent of client and omissions); `digest` = the
  registry manifest digest or the path pack hash, `sha256:<hex>` form.
- Examples: same pins twice → same string; one member digest changes → suffix
  changes; rename changes → suffix changes; `--version v2.0.0` → `2.0.0+…`;
  `--version 1.0.0+x` → 65; `--version latest` → 65.

### C-024 — Description [G4]

`description = base ?? ONRAMP`, at most 500 UTF-16 units; `README.md =
join("\n\n", ["# <name>", base?, omissions?, ONRAMP]) + "\n"` at the plugin
root, per client. Amended 2026-09-27 (owner review): the omissions and the
on-ramp moved out of the description.

- `base` = `--description` / declared `description` / ad-hoc single-ref
  `org.opencontainers.image.description` annotation (same pinned source as
  C-023) / absent; trimmed; omitted when empty. An authored base (flag or
  declared) over 500 → `DescriptionTooLong` 65; an annotation base is cut
  in the description only (last whole sentence keeping ≥ half the budget,
  else word boundary + `…`) with a stderr warning; the README keeps it whole.
- `omissions` = `"Omitted for <client>: " + join(", ", "<kind> <emitted>") + "."`,
  in C-016 order; absent when nothing was omitted.
- `ONRAMP` = `Packaged by grim (https://grimoire.rs); install grim for pinned, updatable installs.` (exact bytes).

Logo (added 2026-09-27, owner review): `--logo` / declared `logo` (relative
to the manifest directory), `.png`/`.svg` case-insensitive, ≤ 1 MiB, regular
file → `assets/logo.<lowercase ext>` in every client tree, README line
`![<name>](assets/logo.<ext>)` under the title, and for Agent Plugins
`extensions."com.openai".interface.logo = "./assets/logo.<ext>"` (Codex's
documented key; Claude's `plugin.json` has no icon field). Outside the
declaration hash and the version hash, like `description`. A bad logo →
`InvalidLogo` 65 before staging. Fallback: an ad-hoc single registry ref
with no `--logo` takes `logo.svg`/`logo.png` from its repository's
description companion (`__grimoire`, floating); not-found/offline → none
silently, other failures or a refused logo → warn, none.

### C-025 — `plugin.json` emitters [G4]

```rust
#[derive(Serialize)] struct ClaudeManifest<'a> { name: &'a str, version: &'a str, description: &'a str }
#[derive(Serialize)] struct AgentPluginsManifest<'a> {
    #[serde(rename = "$schema")] schema: &'static str,  // "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
    name: &'a str, version: &'a str, description: &'a str }
```

Written with `serde_json::to_vec_pretty` + one `\n` to
`root/.claude-plugin/plugin.json` (Claude) or `root/plugin.json` (Agent
Plugins). Exactly these keys in this order. Rust unit tests assert the exact
key list and order and the `$schema` const — no regex. The Agent Plugins 1.0
name pattern `^(?!.*(?:--|\.\.))[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?$` is
asserted in the acceptance suite with Python `re` over every emitted
`plugin.json` `name` (no JSON-schema validator or regex dependency is added
to the crate).

### C-026 — Zip writer [G2, G3]

`export::archive::write_zip(root: &Path, out: &Path) -> std::io::Result<()>`

`zip` 8.6, `default-features = false`. Entries: every regular file under
`root`, name = `/`-joined relative path, sorted bytewise; `Stored`;
`DateTime::default()`; `unix_permissions(0o644)`; no directory entries; no
extra fields; no comment. Archive root = plugin root (for Claude family,
`.claude-plugin/plugin.json` is an entry at depth 1). A symlink under `root`
→ `io::ErrorKind::InvalidInput`, mapped by the caller to `ExportError::Io`
**74**. An unsafe entry name (C-035) → `io::ErrorKind::InvalidData`, mapped
to `ExportError::UnsafeEntry { path }` **65**.

### C-027 — Output placement [G2]

- Final path per (P, c): `<DIR>/<P>.<c>` (dir) or `<DIR>/<P>.<c>.zip`.
- Order: create `<DIR>` if absent → `tempfile::Builder::new().prefix(".grim-export-").tempdir_in(<DIR>)`
  → stage all outputs (C-016…C-026, C-035) → collect every final path → if any
  exists and `--force` is absent → `OutputExists { paths }` **65** reason
  `untracked-destination`, nothing placed → place each output.
- Without `--force` (atomic no-replace): a zip via
  `std::fs::hard_link(staged, final)` (EEXIST → `OutputExists` 65), then the
  staged file removed; a directory via `std::fs::rename` (existing non-empty
  directory or non-directory → `OutputExists` 65). Residual race, documented:
  a concurrently created **empty** directory can be replaced. A filesystem
  without hard links fails with 74.
- With `--force`: a zip is `rename`d over the existing file (atomic replace; a
  symlink at the path is replaced, its target untouched). A directory (or any
  existing non-regular-file path): the old path is first renamed into the
  staging dir (a symlink is moved, never followed), then the new one renamed
  in; the old tree is removed with the staging dir.
- Then, declared mode only, save `L` per C-014.
- Any error before the first placement leaves `<DIR>` with no new entry and the
  staging dir removed (`TempDir` drop). A failure during placement leaves
  already-placed outputs complete and the rest absent (each output is atomic).
- Two identical `(P, c)` pairs are deduplicated before staging.
- Every (P, c) whose admitted-member set is empty → `EmptyPlugin { plugin, client }` **65** before any placement.

### C-028 — `ExportError` and classification [G1]

`crate::export::export_error::ExportError` (thiserror), wrapped as
`crate::error::Error::Export`, classified exhaustively **over the
`ExportError` variants**:

| Variant | Exit | Reason |
|---|---|---|
| `Usage(String)` | 64 | — |
| `Manifest { path, message }` | 65 | — |
| `NoneDeclared { path }` | 65 | — |
| `InvalidVersion { value }` | 65 | — |
| `RenameInvalid`, `RenameCollision`, `RenameStaleReference` | 65 | — |
| `EmptyPlugin { plugin, client }` | 65 | — |
| `UnsafeEntry { path }` | 65 | — |
| `OutputExists { paths }` | 65 | `untracked-destination` |
| `NoPluginFormat { client }` | 78 | — |
| `MemberConflict { .. }` | 78 | — |
| `PluginNotFound { name }`, `SelectorNotFound { selector }` | 79 | — |
| `IncludeNotFound { plugin, include }` *(execution amendment)* | 79 | — |
| `Io { path, source }` | `classify_io` (74 / 77) | — |

Export-owned failures only. `resolve_marketplace`, `plugin_set` and staging
return `crate::error::Error`, so resolver, access, lock and install errors
propagate with their existing classification (stale-lock 65, not-found 79,
69/80/81, locked 75).

### C-029 — `ExportReport` [G1, G2, G4]

`src/api/export_report.rs`, `Printable`:

```rust
pub struct ExportReport { items: Vec<ExportItem> }
pub struct ExportItem {
    pub plugin: String, pub client: String,
    pub family: Family,          // "claude" | "agent-plugins"
    pub format: OutputFormatKind,// "dir" | "zip"
    pub path: PathBuf,           // absolute final path
    pub version: String,
    pub members: Vec<ExportMember>,   // {kind, name, lock_name, pinned}, sorted (kind, name)
    pub omitted: Vec<ExportOmission>, // {kind, name, reason}, sorted (kind, name)
}
```

JSON: `{"items":[…]}`, items ordered by plugin (byte order) then client
(selection order); every key always present; empty arrays as `[]`. Plain:
table `Plugin | Client | Version | Path | Omitted` (omitted = count). Emitted
only on success.

### C-030 — Reproducibility contract [G3]

Same `M`, same `L` (or same ad-hoc refs resolving to the same digests), same
grim build → byte-identical output: the sorted list of `(relpath, sha256)` for
a directory output, and the zip file's SHA-256, across two runs with different
`TZ`, `umask`, working directory and output directory. No output byte depends
on time, locale, absolute paths, hostname or directory-iteration order.

### C-031 — Principle 9 invariants [G1, G7]

1. `grim schema --kind lock` stdout is byte-identical to `main`'s.
2. Every existing `grimoire.lock` test fixture round-trips byte-identically.
3. `grim update --format json` rows gain only `plugin` (always `null`
   without `--marketplace`); no existing key changes.
4. No new `ExitCode` variant, no new `ErrorReason` slug, no new schema id in
   `test/tests/test_check_urls.py`. Test hook: a unit test enumerates every
   `ExitCode` variant (`src/cli/exit_code.rs:20`) and `ErrorReason` slug
   (`src/error.rs:98`) and asserts equality with a literal list captured from
   `main` before this change.
5. `grim install`/`grim add`/`grim update` behaviour is unchanged after the
   C-003, C-015, C-017, C-034 extractions (existing suites green, no
   expectation edits).

### C-032 — Docs and catalog obligations [G8]

- `docs/src/content/docs/commands.md`: `## grim export plugin {#export-plugin}`
  (flags, input matrix, family table, output naming, JSON shape, exit codes)
  and `--marketplace` under `## grim update {#update}`.
- A how-to under `docs/src/content/docs/guides/` (declaration comments per
  `docs-quality.md`) — "Hand a team a plugin without grim": declare
  `[plugins.team]`, export `--client claude --zip`, upload in the Claude app,
  roll forward with `update --marketplace`. It warns that (a) stdio MCP
  `command` values relative to a project do not travel inside a plugin
  (Claude resolves relative commands against its cwd — use absolute or `PATH`
  commands, or `${CLAUDE_PLUGIN_ROOT}`); (b) a `description` edit or a grim
  upgrade can change bytes under the same version — bump `version` (ADR D6).
- A per-client support table for plugin members (C-016), including droid and
  openclaw dropping MCP because their install path declines it.
- `marketplace.toml`/`marketplace.lock` reference (where `configuration.md`
  documents `grimoire.toml`/`grimoire.lock`, anchor `{#marketplace-toml}`).
- `stability.md`: frozen rows for `marketplace.toml`, `marketplace.lock`
  (incl. `[[plugin]]`), export JSON + version grammar; export member bytes
  listed under Unstable.
- `json-interface.md`: export report + update `plugin` field.
- Catalog drift review per `catalog/README.md:114-118` of
  `catalog/skills/grim-usage` **and** `catalog/skills/grim-authoring`.
  grim-usage targets: `SKILL.md` frontmatter description (command list gains
  `export`), `references/consume.md` (`update --marketplace`),
  `references/publish.md` (export) — not `references/updating.md`, which is
  the maintainer re-verification protocol.

### C-033 — Per-plugin staleness [G1, G5]

- Plugin P of `M` is **stale** iff `L` is absent, P has no part in `L`, or
  P's part `metadata.declaration_hash` ≠ C-004 `per_plugin[P]`. Parts of
  plugins not in `M` are dropped on the next write. `[metadata].declaration_hash`
  (`whole`) is recorded, not consulted.
- Export: re-resolves exactly the stale plugins among those it exports; every
  other part is carried byte-identically (same entries, same hash).
- Update: `<P>` and no-selector runs resolve the selected plugins regardless
  of staleness; `<P>:<member>` requires P's part to be fresh (C-009 step 3,
  **65** `stale-lock`) unless the part is absent (full resolve of P).
- A plugin's exported bytes change only when its own declaration or pins
  change.

### C-034 — `update::roll_forward` [G5, G7]

**Amended in review (2026-09-27):** lives in `src/resolve/resolver.rs` as
`resolver::roll_forward` (same signature), which breaks the
`export::resolve` ↔ `command::update` module cycle.

```rust
pub(crate) async fn roll_forward(set: &DesiredSet, previous: Option<&GrimoireLock>, names: &[String],
    access: &Arc<dyn OciAccess>, scope: ConfigScope, options: &ResolveOptions, anchor: &Path)
    -> Result<GrimoireLock, ResolveError>
```

Extracted verbatim from `update::run`'s branching (`update.rs:93-130`):
`names` empty → `resolve_lock`; non-empty with `previous` → `resolve_lock_partial`
(stale guard inside); non-empty without `previous` → `resolve_lock`.
`update::run` calls it (behaviour unchanged, C-031.5); `resolve_marketplace`
calls it once per selected plugin. No other roll-forward exists.

### C-035 — Path containment [G1, G2]

- Before writing any staged file, export asserts its destination is inside
  the staging root: relative, every component `Normal` (no `..`, no root, no
  drive prefix) and no `\` in any emitted name → else
  `ExportError::UnsafeEntry { path }` **65**, nothing placed.
- `write_zip` refuses entry names containing `\`, a drive prefix, a `..`
  component or non-UTF-8 bytes (`io::ErrorKind::InvalidData` → `UnsafeEntry`
  **65**).
- Defence in depth over C-006 (`load_marketplace` name check) and C-021
  (emitted-name check): a name that slipped through either never reaches the
  filesystem.

### C-036 — Agent Plugins `mcp.json` entry [G3, G4]

`export::family::agent_plugins_mcp_entry(name: &str, d: &McpDescriptor) -> Option<serde_json::Value>`
— a pure projection in the style of `Vendor::mcp_entry`
(`vendor_cursor.rs:103`), per `research_plugin_manifest_fields.md` § MCP file
mapping. Deterministic (object keys sorted by `serde_json::Map`).

| `McpServer` | `mcp.json` value |
|---|---|
| `transport = stdio` | `type: "stdio"`, `command`, `args` (omitted when empty), `env` (omitted when empty), `cwd` (when set) |
| `transport = http` | `type: "streamable-http"` (renamed, not copied), `url`, `headers` (omitted when empty) |
| `transport = sse` | `type: "sse"`, `url`, `headers` (omitted when empty) |

- **Declined → `None`** (member omitted `not-representable`): `transport = ws`
  (the spec has no websocket type); `oauth` present (no auth surface); a `${`
  sequence in `command`, `url`, an `env` key, or any header name or value
  (the spec performs no expansion there, so a reference would be sent
  literally). `${…}` in `args` elements, `env` values and `cwd` pass through
  verbatim (the spec expands those).
- **Dropped, as every vendor projection does** (`vendor_cursor.rs:146`):
  `timeout`, `always_load`, `headers_helper` — refinement fields with no
  Agent Plugins key; a `tracing::warn!` names the field; the member is still
  emitted.
- Unit cases: one per table row; each decline; each dropped field absent from
  the value; byte-identical output across two calls.

---

### C-037 — Project as a plugin (amendment 2026-09-27)

- `--project [PATH]`: `ExportMode::Project { name, project: ProjectLock }` (PATH = dir or grimoire.toml, else discovery; PATH with --global/--config → 64);
  `ProjectLock { dir, lock, meta }` from the resolved scope via
  `install::fresh_lock` (missing → `LockMissing` 79, stale → `LockStale` 65).
  Members = `lock.iter_artifacts()`; no resolver call; no lock written. Name
  `--name` > `[plugin].name` > `Usage` 64. Metadata decl = `[plugin]` fields;
  flags override via `plugin_input`. `PluginInput.project_dir` anchors path
  sources and turns a drift into 65 "run `grim lock` in <dir>".
- `[plugin]`: `config::plugin_meta::PluginMeta`, validated at load
  (`PluginInvalid` 78), never in `declaration_hash`.
- `project` plugins: `PluginDecl.project`, exclusive with `include`
  (`Manifest` 65). `include_plugins()` feeds `declaration_hashes`,
  `resolve_marketplace` and L; a project plugin's decl is `merged_decl`
  (decl field ?? project field; project logo made absolute). Stale project
  lock → `Manifest` 65 naming the project. `update --marketplace`: `All`
  skips, a selector naming one → `Usage` 64.

## Scenarios

Fixtures: `make_artifact`/`make_bundle`/`write_config` helpers in
`test/tests`; bundle `team-stack` = skill `team-plan`, skill `team-review`,
agent `team-reviewer`, rule `team-style`, mcp `team-srv` (stdio).

**S-001 — Claude-app zip from one bundle ref.** [G2, G4]
`grim export plugin <reg>/team-stack:1 --client claude --zip -o dist` → exit 0;
`dist/team-stack.claude.zip` exists; its entries include
`.claude-plugin/plugin.json` at depth 1, `skills/team-plan/SKILL.md`,
`agents/team-reviewer.md`, `.mcp.json`; no `team-style` file; manifest `name`
= `team-stack`, `version` = `<annotation version of the pinned bundle>+<12 hex>`,
description is the annotation text; `README.md` lists `Omitted for claude:
rule team-style.` and the on-ramp sentence. No `marketplace.lock` anywhere. Errors: registry down → 69;
tag absent → 79.

**S-002 — Agent Plugins directory.** [G2, G4]
`… team-stack:1 --client codex -o out` → `out/team-stack.codex/plugin.json`
with the `$schema` const first; `skills/` and `mcp.json` whose
`mcpServers.team-srv` is `{"args":…,"command":…,"type":"stdio"}`; JSON
`omitted` = agent `no-format-surface`, rule `no-format-surface`; description
names both.

**S-003 — Two refs need `--name`.** [G2]
`grim export plugin a:1 b:1` → 64, nothing written; with `--name duo` → exit 0,
one plugin `duo` containing both; version base `0.0.0`.

**S-004 — Member conflict.** [G2]
With `--name d`: refs `acme/x:1` and `other/x:1` (both skills named `x`) → 78
naming both refs; same ref twice → deduplicated, exit 0. Direct-vs-bundle:
`<reg>/team-stack:1` plus `other/team-plan:1` (a different repo) → 78 naming
both sources; plus `<reg>/team-plan:<the member's own tag>` → deduplicated,
exit 0. Path-vs-bundle: `<reg>/team-stack:1` plus `./local/team-plan` (a
skill dir whose intrinsic name is `team-plan`) → 78.

**S-005 — Declared plugin, no lock.** [G2, G5]
`M` declares `[plugins.team] include=["<reg>/team-stack:1"]`;
`grim export plugin --plugin team --client claude -o dist` → exit 0, output
placed, then `marketplace.lock` written with every entry `plugin = "team"`
and a `[[plugin]]` row for `team`; a second run with `--force` → exit 0,
lock bytes unchanged (`generated_at` preserved) and output byte-identical.

**S-006 — Stale lock after an edit.** [G5]
Add an include to `team` → next export re-resolves `team`, rewrites `L`
(team's `[[plugin]]` hash changes); editing only `description` does not
rewrite `L` and keeps the version suffix.

**S-007 — Offline from a warm cache.** [G3]
After S-005, `grim --offline export plugin --plugin team --client claude -o dist2 --force`
→ exit 0, byte-identical to S-005's output; cold cache offline → 81.

**Amended in review (2026-09-27):** the warm-cache half above does not
hold. Export keeps no manifest cache, so even a plugin whose blobs are
already cached still needs the network to re-check the floating tag; the
offline warm-cache run exits 81 (`OfflineMiss`) too, the same as the
cold-cache half, not 0 (plan decision 36). The regression test for the
warm-cache byte-identical claim is a strict `xfail` pending a manifest
cache.

**S-008 — Selection errors.** [G2]
`--plugin ghost` → 79; no refs, no `--plugin`, `M` has no `[plugins]` → 65
with hint; `--plugin team` with no `./marketplace.toml` → 65 "manifest not
found"; `--marketplace team.json` or `--marketplace grimoire.toml` → 65;
`--plugin team a:1` → 64; `--name x --plugin team` → 64.

**S-009 — Clients without a plugin format.** [G2]
`--client opencode` → 78; config `clients = ["claude","opencode"]` and no
`--client` → one output `team.claude`, stderr note naming opencode;
config `clients = ["opencode"]` → falls back to `team.agents`.

**S-010 — Per-client declines.** [G4]
`--client droid,junie,claude` over `team-stack` → three outputs; droid omits
agent + mcp (`client-declined`); junie omits agent (`client-declined`), keeps
`.mcp.json`; claude keeps both; rule omitted in all.

**S-011 — Rendered like install.** [G3]
For each of claude, codex, openclaw: every file under `skills/<n>/` and
`agents/<n>.md` equals the file `grim install --client <c>` produced for the
same lock (project scope; OpenClaw global under a sandboxed `HOME`);
`.mcp.json` parses to the same `mcpServers` members as the installed one.

**S-012 — Reproducible.** [G3]
Run S-001 and S-002 twice with different `TZ`, `umask` and `-o` → zip SHA-256
equal; directory `(relpath, sha256)` lists equal.

**S-013 — Rename success.** [G6]
`[plugins.team.rename] strip_prefix = "team-"` (fixture files contain no
self-references) → `skills/plan/`, `skills/review/`, `agents/reviewer.md`,
`mcpServers.srv`; `SKILL.md` frontmatter `name: plan`; agent frontmatter
`name: reviewer`; JSON members `{name:"plan", lock_name:"team-plan"}`;
version suffix differs from the un-renamed export.

**S-014 — Rename empty/invalid.** [G6]
`strip_prefix = "team-plan"` → remainder empty → 65; `strip_prefix = "team"`
→ `-plan` → 65; nothing written, `L` untouched.

**S-015 — Rename collision.** [G6]
Members `team-plan` and `plan` with `strip_prefix = "team-"` → 65 naming both.

**S-016 — Stale reference.** [G6]
`team-review/SKILL.md` line 7 contains `../team-plan/notes.md` → 65; stderr
names `claude: skills/review/SKILL.md:7: 'team-plan'`; `dist` gains no entry;
`L` not written (if it was absent).

**S-017 — Overwrite policy.** [G2]
Re-run S-001 → 65, JSON error `reason: "untracked-destination"`,
`forceable: true`, zip unchanged; with `--force` → replaced, exit 0. Same for
a directory output: a non-empty existing `dist/team-stack.codex/` → 65
without `--force`; with `--force` → replaced, no file of the old tree left. A
symlink at the output path (zip or directory) with `--force` → the link is
replaced, its target untouched.

**S-018 — Empty plugin.** [G4]
A plugin of only an agent exported `--client codex` → 65 `EmptyPlugin`.

**S-019 — Version override and bump.** [G4]
`--version v1.2.0` → `1.2.0+<h>`; publish a patch of `team-plan` under the same
floating tag, `grim update --marketplace marketplace.toml team`, re-export with
`--force` → exit 0, `1.2.0+<h'>`, `h' ≠ h`. `--version 1.2.0+ci` → 65.

**S-020 — Update rolls all, installs nothing.** [G5]
`grim update --marketplace marketplace.toml` with no `grimoire.toml` in scope
→ exit 0; `L` pins roll forward; no vendor directory, `.grimoire/` or state
file is created or modified (tree snapshot before/after of the workspace and
`HOME`, excluding `GRIM_HOME` and `L` itself); JSON rows carry `plugin`.
`--marketplace m.toml --global` and `--config x.toml` → 64.

**S-021 — Update one plugin.** [G5]
Two plugins `a`, `b` share floating members; `… update --marketplace m.toml a`
→ only `a`'s entries change; `b`'s bytes in `L` identical.

**S-022 — Update one member.** [G5]
`… a:team-plan` → only the entries named `team-plan` (every kind) re-resolved;
other `a` entries carried.

**S-023 — Update selector errors.** [G5]
`… ghost` → 79; `… a:ghost` → 79; `… a:` → 64; `a` stale in `L` + `a:team-plan`
→ 65 `stale-lock`; `a` stale + `a` → exit 0 (re-resolved); `L` absent +
`a:ghost` → 79 and `L` not written; `--marketplace m.toml --client claude` → 64;
`--force` → 64.

**S-024 — Lock flavors reject each other.** [G1]
A `grimoire.lock` entry with `plugin = "x"` (or a `[[plugin]]` table) →
`grim install`, `grim status` → 78; `grim context` reports it in
`lock_error` and exits 0; `grim update` / `grim
lock` / `grim add` treat it as absent and re-resolve (existing `.ok()`
behaviour, unchanged). A `marketplace.lock` entry without `plugin`, or with
a non-empty `[[bundle]]` → `export --plugin` / `update --marketplace` → 78.

**S-025 — Normal update report is additive.** [G1]
`grim update --format json` in a project → every row has `"plugin": null`; all
previous keys unchanged.

**S-026 — JSON shape.** [G1]
S-010 with `--format json` → three items in `--client` order, each with all
eight keys; `members`/`omitted` sorted by (kind, name).

**S-027 — Lock contention.** [G5]
Hold `marketplace.toml`'s advisory lock; `export --plugin team` and
`update --marketplace` → 75, `reason: "locked"`, `retryable: true`.

**S-028 — Junie drops an OAuth MCP server.** [G4]
A bundle whose `mcp` member's descriptor carries an OAuth block, exported
`--client junie` → exit 0; no `.mcp.json` entry for it; JSON `omitted` has
`{kind:"mcp", reason:"not-representable"}`; description names it.

**S-029 — Hand-edited lock cannot escape.** [G1, G2]
`marketplace.lock` edited to `name = "../evil"` on one entry, its `[[plugin]]`
hash left matching → `export --plugin team` → 78; `dist` gains no entry,
nothing is written outside it.

**S-030 — Per-plugin staleness.** [G5]
Plugins `a`, `b`, both with floating members, exported once. Publish new
digests under both floating tags; edit only `a`'s `include`;
`export --plugin a --force` → only `a`'s entries and `[[plugin]]` hash in `L`
change; `b`'s lock bytes identical; a following `export --plugin b --force`
yields bytes identical to `b`'s first export.

**S-031 — Agent Plugins MCP projection.** [G3, G4]
A plugin with mcp members `web` (http), `feed` (sse), `sock` (ws), `authd`
(http + oauth) and `hdr` (http, header value `Bearer ${TOKEN}`) exported
`--client agents,cursor` → both outputs carry `mcp.json` with `web` as
`type: "streamable-http"` and `feed` as `type: "sse"`; `sock`, `authd` and
`hdr` omitted `not-representable` and named in each description; a `timeout`
on `web` is absent from its entry; the `agents` output is emitted even though
the `agents` vendor declines MCP on install.

---

## Traceability

| Goal | Contracts | Scenarios |
|---|---|---|
| G1 | C-001, C-002, C-004–C-006, C-013, C-028, C-029, C-031, C-033, C-035 | S-024–S-026, S-029 |
| G2 | C-001, C-003, C-014, C-015, C-026, C-027, C-029, C-035 | S-001–S-005, S-008, S-009, S-017, S-029 |
| G3 | C-017, C-018, C-020, C-026, C-030, C-036 | S-007, S-011, S-012, S-031 |
| G4 | C-002, C-016, C-020, C-023–C-025, C-029, C-036 | S-001, S-002, S-010, S-018, S-019, S-028, S-031 |
| G5 | C-009–C-013, C-033, C-034 | S-005, S-006, S-019–S-023, S-027, S-030 |
| G6 | C-019, C-021, C-022 | S-013–S-016 |
| G7 | C-003, C-005, C-007–C-009, C-011, C-013, C-015, C-017, C-018, C-031, C-034 | S-011, S-020 |
| G8 | C-032 | (docs build + `task catalog:verify`) |
