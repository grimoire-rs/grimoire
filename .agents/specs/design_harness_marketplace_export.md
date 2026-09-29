# System Design: Harness marketplace export (phase 2)

## Metadata

**Status:** Accepted (with the ADR, 2026-09-29)
**Author:** Architect (hex-architect xhigh)
**Date:** 2026-09-28 (Round-1 review amendments 2026-09-29; Round-2 planning amendments 2026-09-29)

> **ADR § Amendments (Round 2) R2-1…R2-27 supersede any conflicting text
> below.** In-place edits: §5.2/§5.3 client table and defaults (Qoder in,
> Cursor opt-in), §5.4 order and rename, §5.8 verification job and policy
> gate. Traceability IDs (C-/S-) live in
> `.agents/plans/plan_harness_marketplace_export.md`.
**Beads Issue:** N/A
**Related PRD:** `.agents/discussions/marketplace-phase-2.md`
**Related ADRs:** `.agents/adr/adr_harness_marketplace_export.md` (decisions D1–D13, cited as "ADR Dn"), `.agents/adr/adr_harness_plugin_export.md` (phase 1)

**Tech Strategy Alignment:**
- [x] Language/Framework follows Golden Path (Rust 2024 + Tokio in grim; TypeScript/Node in `grimoire-indexer` as today)
- [x] No database, no service; storage is git + OCI registries
- [x] Infra tier: static (git forge + Pages)
- [ ] Observability: not applicable (CLI + CI jobs; JSON reports and job logs are the signals)
- [x] Deviations documented in the ADR (none)

This document carries no traceability IDs; those originate in the plan.

## Executive Summary

`grim export marketplace` turns a curated `marketplace.toml` into a git
repository that harnesses add as a plugin marketplace:

- Claude Code, Copilot, Codex and Qoder by default (R2-15; set frozen at 1.0, R2-11).
- Cursor opt-in through `[marketplace].clients` (R2-11, R2-12).
- Junie, OpenClaw and Droid best effort, through the Claude file.

Every run is a stateless full regeneration. grim renders every selected
`(plugin, client)` and places only trees whose bytes differ. Ownership is by
convention, so there is no bookkeeping file.

A scheduled job runs `grim update --marketplace`, then
`grim export marketplace`, and opens a PR/MR on diff. On GitLab the job comes
from `grimoire-components`; on GitHub it is a documented workflow.

`grimoire-indexer` stops describing every package on every build. It also
serves a `/marketplace/` landing page with per-harness add commands.

---

## 1. Context (C4 Level 1)

### System Context Diagram

```
                 ┌──────────────┐  edits toml, merges bot PRs
                 │   Curator    │───────────────────────────────┐
                 └──────────────┘                               ▼
┌────────────┐  grim publish   ┌──────────────┐  pull   ┌───────────────────────┐  clone/pull  ┌──────────────┐   use   ┌──────────┐
│ Publishers │────────────────▶│ OCI registry │◀────────│ Marketplace repo + job│◀─────────────│   Harnesses  │◀────────│ Consumer │
└────────────┘                 └──────────────┘         │ (git forge, CI, grim) │              └──────────────┘         └──────────┘
                                      ▲                 └───────────────────────┘                     ▲
                                      │ HEAD probes                  ▲ link                            │ add command
                               ┌──────────────┐  build   ┌──────────────────────┐   visit   ┌──────────┴──┐
                               │ Index CI     │─────────▶│ Index site           │◀──────────│  Visitor    │
                               │ (indexer)    │          │ /, /p/…, /marketplace│           └─────────────┘
                               └──────────────┘          └──────────────────────┘
```

### Actors & External Systems

| Actor / system | Role | Trust |
|---|---|---|
| Curator | Owns `marketplace.toml`, reviews and merges bot PRs | trusted |
| Publishers | Push artifacts to OCI registries | integrity-verified only; compromise out of scope (`adr_artifact_trust_model.md:83-85`) |
| OCI registries | Source of every member | untrusted transport, digest-pinned |
| Git forge (GitHub, GitLab) | Hosts the marketplace repo, runs the job | platform |
| Harnesses | Read their marketplace file, install their tree | external consumers; compare `version` for updates; some auto-update fleets on merge |
| Consumer | Adds the marketplace, installs plugins | — |
| Index site | Discovery surface; links the marketplace | static |

---

## 2. Containers (C4 Level 2)

### Container Diagram

```
┌──────────────────────── Marketplace repo (git) ─────────────────────────┐
│ marketplace.toml   marketplace.lock   .gitignore (.grim-export*)        │
│ .claude-plugin/marketplace.json ─▶ claude/<p>/   (.claude-plugin/plugin.json …)
│ .github/plugin/marketplace.json ─▶ copilot/<p>/  (plugin.json …)        │
│ .agents/plugins/marketplace.json ─▶ codex/<p>/                          │
│ .cursor-plugin/marketplace.json ─▶ cursor/<p>/   (after the probe)      │
│ CI file: GitLab component include | GitHub workflow                     │
└───────────────▲────────────────────────────────────────────▲───────────┘
                │ bot branch push, PR/MR                     │ checkout
┌───────────────┴──────────── Maintenance job ────────────────┴──────────┐
│ pinned grim (sha256 literal) → update → export → policy gate → PR/MR   │
│ verification job (pull_request): in-place render → git status → validate    │
└───────────────┬────────────────────────────────────────────────────────┘
                │ grim CLI (Rust)                 ┌── grimoire-indexer (Node) ─┐
                ▼                                 │ enrich: probe pool + slice  │
         OCI registries ◀─────────────────────────│ build: Astro + /marketplace │
                                                  └────────────▲────────────────┘
                                                               │ index.config.json `marketplace`
                                                        grimoire-index repo
```

### Container Descriptions

| Container | Repo | Tech | Responsibility | Changes |
|---|---|---|---|---|
| grim CLI | `grimoire` (this repo) | Rust | resolve, render, version, write the marketplace tree | new subcommand, `[marketplace]`, D7 version |
| Marketplace component | `grimoire-components` | GitLab CI component | scheduled regenerate → MR; verification job | new `templates/marketplace.yml` |
| GitHub workflow | documented in grim docs; carried by `grimoire-rs/marketplace` | GitHub Actions | same flow for GitHub | new guide page |
| Marketplace repo | e.g. `grimoire-rs/marketplace` | git | deployed container: data + generated trees | new repo (seed) |
| Indexer | `grimoire-indexer` | TypeScript, Astro | enrich + site render | probe pool, rotating slice, landing page |
| Index repo | `grimoire-index` | config + data | sets the `marketplace` config key | config only |

---

## 3. Components (C4 Level 3)

### grim components

| Module | Status | Responsibility |
|---|---|---|
| `src/command/export.rs` | extended | `ExportCommand::Marketplace(ExportMarketplaceArgs)` |
| `src/export/marketplace.rs` | extended | parse + validate the optional `[marketplace]` table for every loader caller |
| `src/export/resolve.rs` | reused unchanged | stale detection, `resolve_marketplace`, lock path |
| `src/export/stage.rs` | split only | declared path (`:278-365`) shared by both commands; staging split from placement; layout-aware final path |
| `src/export/marketplace_export.rs` | **new** | client table, ownership convention, inventory compare, removal, containment checks, documents, report |
| `src/export/family.rs` | changed | `plugin_version` from the tree inventory |
| `src/export/archive.rs` | extended | `tree_inventory` over the existing `collect` (`:74`) / `entry_name` (`:109`) walk |
| `src/api/export_report.rs` | extended | `MarketplaceExportReport`, `MarketplaceItem`, `MarketplaceFileItem`, `ExportAction` |
| `src/export/export_error.rs`, `src/error.rs` | unchanged variants | reused `Manifest`, `EmptyPlugin`, `UnsafeEntry`, `OutputExists`, `InvalidLogo` with new messages |

### Component Responsibilities (flow of one `grim export marketplace` run)

1. **CLI** (`command/export.rs`) — parse flags. Build the fetch scope and the
   access seam anchored at the manifest directory (phase-1 decision 35). There
   is no client flag.
2. **Declared resolution** (`stage.rs` declared path, reused) —
   - `marketplace::load` validates `[marketplace]` for every caller; this
     command also requires the table;
   - take the `ConfigFileLock` on the manifest, then `resolve::load_lock`;
   - run per-plugin `is_stale`, and `resolve_marketplace` for stale plugins
     only;
   - resolve `project` plugins through `ProjectLock::load`.
3. **Clients** — `[marketplace].clients` or `DEFAULT_CLIENTS`, first mention
   wins, deduplicated.
4. **Output-root lock** — an advisory lock on
   `<canonical root>/.grim-export`, held until step 9.
5. **Containment pre-check** — `symlink_metadata` on every ancestor of every
   owned path, for selected and table clients. Also sweep stale
   `.grim-export-*` dirs with a warning.
6. **Plugin inputs** — `plugin_input` per plugin: rename, metadata
   precedence, **version base only**. Marketplace-only rule: the logo and
   `path:` members resolve inside the manifest dir, with no symlink.
7. **Stage** (`stage.rs` seams, reused) — for every plugin:
   - `stage_members` once (fetch + verify);
   - per selected client: `render_members`, `stale_scan`, README and logo;
   - the D7 version: write `plugin.json` with the base, compute
     `tree_inventory`, rewrite with the suffix;
   - `archive::check_tree`, then the unsafe-name check.
   - A plugin empty for a client gives an `empty` row and no tree. A plugin
     empty for all selected clients exits 65 `EmptyPlugin`.
8. **Ownership check** — the D1 convention against disk. A foreign
   destination exits 65 `untracked-destination` unless `--force`.
9. **Compare and place.**
   1. Per tree: equal inventory → `unchanged`, else place (`place`,
      aside-replace).
   2. Marketplace files: plain `atomic_write` when the bytes differ.
   3. Removals, with empty parents removed up to the root (never `.github/`).
   4. The lock under the phase-1 rule (`stage.rs:357-364`).
10. **Report** — `MarketplaceExportReport`.

### Maintenance job components

| Step | GitLab component | GitHub workflow |
|---|---|---|
| Install grim | inline installer, `sha256sum -c` against input `grim_sha256` | same, env literal |
| Registry login (optional) | `grim login` from inputs (publish vocabulary) | same, from secrets |
| Roll pins | `grim update --marketplace $M --format json > "$TMP/update.json"` | same, `$RUNNER_TEMP` |
| Render | `grim export marketplace --marketplace $M --format json > "$TMP/export.json"` | same |
| Policy gate | `git status --porcelain=v1 -z --untracked-files=all`; allow-list from `export.json`; LFS + size check | same |
| No-op exit | empty status → exit 0 | same |
| Commit | bot identity, `chore(marketplace): …` | same |
| Deliver | `--force-with-lease` + push options open/refresh the MR | `gh pr view \|\| gh pr create --body-file`, `gh pr edit --body-file` |
| Keepalive | n/a | separate job, `permissions: {actions: write}` only |
| Verify | MR pipeline job | `on: pull_request` job |

### Indexer components

| Module | Change |
|---|---|
| `src/enrich/index.ts` `enrichOne` | probe first; carry the seeded record unless in the slice; set `describedAt` |
| `src/enrich/index.ts` `enrichIndex` | bounded worker pool, `--concurrency`; select the re-describe slice (⌈N/7⌉ oldest `describedAt`, missing first) |
| `src/cli/enrich.ts` | flag `--concurrency` (default 8) |
| `src/enrich/checkpoint.ts` | fix the partial seed (`hydrateFiles`, `:225`); carry `describedAt` |
| `src/data/index.ts` `compileIndex` | strip `describedAt` from `all.json` records |
| `src/config.ts` | `marketplace?: { url: string; name: string } \| null`, default `null`, validated |
| `src/renderer/astro/pages/marketplace…` | route rendered only when configured, built after the `rmSync` (`src/data/index.ts:166`) |

---

## 4. Key Design Decisions

| Decision | Choice | ADR |
|---|---|---|
| Generation model | stateless full regeneration, byte-compare placement, ownership by convention | D1 |
| Layout | `./<client>/<plugin>/`, harness-owned file paths, default `claude, copilot, codex, qoder`, `cursor` opt-in (R2-11, R2-15) | D2 |
| Document | one Claude-shaped document, per-client `source`/`version` | D3 |
| Marketplace metadata | optional `[marketplace]` table, validated by every loader caller | D4 |
| CLI | `--marketplace`, `-o`, `--force` | D5 |
| Compare / safety | inventory compare; source exec bit; ancestor + name checks | D6 |
| Version | rendered-tree hash over canonical JSON tuples, both export commands | D7 |
| Report / exits | sibling report; no new variant or exit code | D8 |
| Job | PR default, fixed bot branch, pinned grim, App token preferred, in-place verification (R2-1) | D9 |
| Indexer | HEAD probe, rotating slice, pool, landing page | D10 |

---

## 5. API Design

### 5.1 CLI

```
grim export marketplace [--marketplace <PATH>] [-o, --output <DIR>] [--force]
  global: --format json|plain, --offline, --progress
```

Clap shape: `ExportMarketplaceArgs { marketplace: Option<PathBuf>, output: Option<PathBuf>, force: bool }`.
`output: None` resolves to the manifest's directory after
`std::path::absolute`, then canonicalises for the output-root lock key.

### 5.2 `marketplace.toml`

```toml
[marketplace]
name = "grimoire"
owner = { name = "Grimoire" }            # email optional
description = "Curated grim packages"    # optional
clients = ["claude", "copilot", "codex", "qoder"] # optional; claude|copilot|codex|qoder|cursor

[plugins.grim-essentials]
include = ["ghcr.io/grimoire-rs/grim-essentials:1"]
```

Rust shape (contract, not implementation):

```rust
pub struct MarketplaceManifest {
    pub path: PathBuf,
    pub plugins: BTreeMap<String, PluginDecl>,
    pub marketplace: Option<MarketplaceMeta>,   // new; None for the ad-hoc in-memory manifest (stage.rs:216)
}
#[serde(deny_unknown_fields)]
pub struct MarketplaceMeta {
    pub name: String,
    pub owner: MarketplaceOwner,
    #[serde(default)] pub description: Option<String>,
    #[serde(default)] pub clients: Vec<String>,  // validated against the D2 table at load
}
#[serde(deny_unknown_fields)]
pub struct MarketplaceOwner { pub name: String, #[serde(default)] pub email: Option<String> }
```

- `RawManifest` gains `#[serde(default)] marketplace: Option<MarketplaceMeta>`.
- Validation runs inside `marketplace::load`, so `export plugin`,
  `export marketplace` and `update --marketplace` all exit 65 `Manifest` on
  a bad table.
- `declaration_hashes` never reads `marketplace`.
- A client outside the table gives the served-by hint in the `Manifest`
  message, for example "junie reads `.claude-plugin/marketplace.json`;
  select `claude`".
- Reserved names follow ADR D4:
  - substrings `anthropic`, `claude`, `official`, `qoder` (R2-15; the substring rule subsumes the old `claudeai-`/`qoder-` prefixes);
  - the exact Claude set `agent-skills`, `knowledge-work-plugins`, `inline`,
    `builtin`, `skills-dir`, `synced`, `github`, `gh`, `npm`, `pip`, `uv`,
    `cargo`.

### 5.3 Marketplace document (per client file)

Fixed-order structs, `serde_json::to_vec_pretty` + `\n`:

```rust
struct MarketplaceDoc<'a> {
    name: &'a str,
    owner: OwnerDoc<'a>,                                  // { name, email? (skip if None) }
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<MetadataDoc<'a>>,                    // { description }
    plugins: Vec<EntryDoc<'a>>,                           // byte order of name; may be empty
}
struct EntryDoc<'a> { name: &'a str, source: String /* "./<client>/<plugin>" */, version: &'a str, description: &'a str }
```

Client table (`marketplace_export.rs`):

```rust
pub(crate) const MARKETPLACE_CLIENTS: [(ClientTarget, &str /* file */); 5] = [
    (Claude, ".claude-plugin/marketplace.json"), (Copilot, ".github/plugin/marketplace.json"),
    (Codex, ".agents/plugins/marketplace.json"), (Qoder, ".qoder-plugin/marketplace.json"),
    (Cursor, ".cursor-plugin/marketplace.json"),
];
pub(crate) const DEFAULT_CLIENTS: &[ClientTarget] = &[Claude, Copilot, Codex, Qoder]; // frozen at 1.0 (R2-11)
pub(crate) fn served_by(client: ClientTarget) -> Option<ClientTarget>;          // hint text only: junie/openclaw/droid → Claude
pub(crate) fn tree_rel(client: ClientTarget, plugin: &SkillName) -> PathBuf;     // "<client>/<plugin>"
```

Per-client manifest location (both export commands, R2-12/R2-15):
Claude, Droid, Junie, OpenClaw → `.claude-plugin/plugin.json`; Qoder →
`.qoder-plugin/plugin.json` (same bytes); Agent Plugins → root
`plugin.json`, and Cursor additionally `.cursor-plugin/plugin.json` =
`claude_plugin_json` bytes. Every manifest is written before the inventory,
so each enters the version, and the D7 rewrite patches every manifest.

### 5.4 Ownership and removal (convention, no state)

```
selected   = [marketplace].clients or DEFAULT_CLIENTS
for c in MARKETPLACE_CLIENTS:
  file_ours(c) = file absent, or file parses as JSON with .name == [marketplace].name
  if c in selected:
      owns ./<c>/ and file(c)
      refuse (65 untracked-destination, unless --force) when:
         file present and not file_ours(c), or
         ./<c>/ non-empty and file(c) absent
      remove every entry under ./<c>/ not in the render set
  elif file(c) present and file_ours(c):          # dropped client
      remove file(c) and ./<c>/
  else: leave alone
```

- A marketplace file that fails to parse is not ours.
- A file naming another marketplace (a `[marketplace].name` rename) is not
  ours; the 65 message names both names and says `--force` adopts (R2-10).
- Removal unlinks symlinks without following them.
- Empty-parent pruning stops before the output root and never removes
  `.github/` or `.agents/` (R2-8).
- Write order: marketplace files → trees → removals → lock (R2-9).
- One row per `(plugin, client)`: a declared plugin empty for `c` is
  `empty` even when its stale tree was deleted (R2-13).

### 5.5 JSON report

```json
{
  "items": [
    {"plugin": "team", "client": "claude", "family": "claude",
     "path": "/abs/repo/claude/team", "version": "1.4.0+3f9a0c12b7de",
     "action": "unchanged",
     "members": [{"kind": "skill", "name": "plan", "lock_name": "team-plan", "pinned": "ghcr.io/acme/team-plan@sha256:…"}],
     "omitted": []},
    {"plugin": "old", "client": "claude", "family": "claude",
     "path": "/abs/repo/claude/old", "version": null,
     "action": "removed", "members": [], "omitted": []}
  ],
  "files": [
    {"client": "claude", "path": "/abs/repo/.claude-plugin/marketplace.json",
     "action": "written", "plugins": ["team"]}
  ]
}
```

| Field | Type | Values / rule |
|---|---|---|
| `items[].action` | string | `written` = placed; `unchanged` = rendered inventory equal to disk, nothing placed; `removed` = deleted; `empty` = nothing for this client, not listed |
| `items[].path` | string | absolute tree path; for `empty`, the path the tree would occupy (absent on disk) |
| `items[].version` | string \| null | set for `written`/`unchanged`; null for `empty`/`removed` |
| `items[].members`, `omitted` | arrays | full for `written`/`unchanged` (every run renders); `empty`: `members` `[]`, `omitted` every member; `removed`: both `[]` |
| `files[].action` | string | `written`, `unchanged`, `removed` |
| `files[].plugins` | string[] | sorted; `[]` when removed |

Ordering:

- Current plugins come first, sorted by plugin bytes, then by client
  selection order.
- `removed` items follow, sorted by `(client, plugin)` bytes.
- Files are listed in selection order, then removed files by client name.

### 5.6 Version and tree inventory

```rust
/// Sorted by entry name bytes; regular files only; never follows symlinks.
pub(crate) fn tree_inventory(root: &Path) -> io::Result<Vec<(String /* entry name */, bool /* exec */, String /* sha256 hex */)>>; // archive.rs
/// suffix = first 12 hex of sha256(serde_json::to_vec(&inventory)) — compact JSON array of [name, exec, sha] tuples;
/// root staged with plugin.json version = base.
pub(crate) fn plugin_version(base: &str, inventory: &[InventoryEntry]) -> String;   // family.rs, replaces the member-line form; inventory from archive::tree_inventory (plan C-001/C-002)
```

- `PluginInput.version` becomes `version_base: String` (normalized,
  default `0.0.0`). `ExportItem.version` is set per client after staging.
- `export plugin` and `export marketplace` call the same staging function,
  so their trees and versions are byte-equal.
- The on-disk compare (D6) uses the same `tree_inventory`, which gives one
  walk and one definition of "equal".
- Required tests:
  - the codex counterexample: `{a, b}` ≠ `{"a\n<sha>  b"}`;
  - a README/`ONRAMP` golden;
  - the exec bit carried from source.

### 5.7 Errors

No new variant. New messages on reused variants:

| Variant (exit) | New message cases |
|---|---|
| `Manifest` (65) | `[marketplace]` missing (export marketplace only), bad name, reserved name, bad owner/email, unknown or non-marketplace client (+ served-by hint); marketplace-only `path:` member outside the manifest dir |
| `InvalidLogo` (65) | logo outside the manifest dir or a symlink (export marketplace only); bytes-read cap (both commands) |
| `UnsafeEntry` (65) | symlink / non-dir / reparse-point ancestor; case collision; platform-reserved name |
| `OutputExists` (65 `untracked-destination`) | foreign `./<client>/` or marketplace file without `--force` |

`src/error.rs:1253`'s hand-written table test gets no new row. The plan
still adds message assertions for the reused variants.

### 5.8 Maintenance job contract

**GitLab component `templates/marketplace.yml` inputs**

| Input | Default | Rule |
|---|---|---|
| `stage` | `deploy` | — |
| `image` | `alpine:3.22` | needs sh, tar, sha256sum, git, curl |
| `grim_version` | (required) | `^v\d+\.\d+\.\d+$`, no `latest` |
| `grim_sha256` | (required) | 64 hex, x86_64 musl `.tar.gz` |
| `manifest` | `./marketplace.toml` | — |
| `mode` | `merge-request` | `merge-request` \| `push` |
| `branch` | `grim/marketplace` | bot branch, force-updated |
| `token` | (required) | project access token: **Developer** + `write_repository` for MR mode; **Maintainer** only for push mode onto a protected branch |
| `max_file_bytes` | `10485760` | policy-gate size cap |
| `registry`, `registry_user`, `registry_password` | `''` | empty skips `grim login` |

Job rules:

- Trigger on `$CI_PIPELINE_SOURCE == "schedule"`, on `"web"`, or on a push
  to `$CI_DEFAULT_BRANCH` that changes `marketplace.toml`.
- `resource_group: grim-marketplace`.
- MR mode:
  `git push --force-with-lease -o merge_request.create -o merge_request.target=$CI_DEFAULT_BRANCH -o merge_request.title=… origin HEAD:refs/heads/$branch`.
- Push mode: `git push origin HEAD:$CI_DEFAULT_BRANCH`, with no `ci.skip`.
- A verification job runs on `merge_request_event`. It uses the same
  in-place render and `git status` check as GitHub below (R2-1).

**GitHub workflow (documented) contract fragments**

- `on: schedule (daily) | push: {branches: [main], paths: [marketplace.toml]} | workflow_dispatch`.
- Permissions: top-level `permissions: {contents: read}`.
  - The regenerate job gets `contents: write` and `pull-requests: write`.
  - The keepalive job gets only `actions: write`.
- `concurrency: {group: grim-marketplace, cancel-in-progress: false}`.
- Token: output of `actions/create-github-app-token@<sha>` with
  `permission-contents: write`, `permission-pull-requests: write` and
  `repositories: ${{ github.event.repository.name }}`. It never gets
  `workflows`. The private key lives in an Environment restricted to the
  default branch. Fallback: `github.token`.
- Every `uses:` is pinned by full SHA. No third-party PR action; plain `gh`.
- No `${{ github.head_ref }}` or `${{ inputs.* }}` inside `run:`. Values pass
  through `env:`.
- The PR body is built by a script from `export.json` / `update.json`:
  - plugin names re-validated as `SkillName`;
  - pins matched against `@sha256:[0-9a-f]{64}$`;
  - capped at 60 000 chars;
  - written to `$RUNNER_TEMP/body.md` and passed with `--body-file`.
- Verification job (R2-1, R2-6):
  - Trigger: `on: pull_request` (never `pull_request_target`), **no
    `paths:` filter**.
  - Registry login runs only if
    `github.event.pull_request.head.repo.full_name == github.repository`.
  - Steps, in the checkout (nothing copied):
    1. `grim export marketplace --marketplace "$GITHUB_WORKSPACE/marketplace.toml" --format json > "$RUNNER_TEMP/verify.json"`.
    2. `git status --porcelain=v1 -z --untracked-files=all --ignored=matching -- marketplace.lock .claude-plugin/marketplace.json .github/plugin/marketplace.json .agents/plugins/marketplace.json .qoder-plugin/marketplace.json .cursor-plugin/marketplace.json claude/ copilot/ codex/ qoder/ cursor/`
       must print nothing.
    3. `claude plugin validate` over `.claude-plugin/marketplace.json` and
       every `claude/<plugin>`.
  - With the `GITHUB_TOKEN` fallback the regenerate job runs steps 1–3
    itself before pushing (R2-7).

**Policy gate** (both forges):

- Parse `git status --porcelain=v1 -z --untracked-files=all --no-renames`
  NUL-separated: one path per record (R2-5). A changed path that is a
  symlink fails.
- The allow-list is exact:
  - `marketplace.lock`;
  - every `files[].path` in `export.json`, made relative;
  - `<client>/**` for every `files[].client`, which includes removed clients.
- Any path outside the allow-list fails.
- `git check-attr filter` = `lfs` on a changed path fails.
- A changed file over `max_file_bytes` fails.
- All three checks fail before the commit.

### 5.9 Indexer contracts

- `grim-indexer enrich [--seed] [--concurrency <n>]`.
- The slice size is ⌈N/7⌉. Selection is ascending `describedAt`; missing
  entries count as oldest; ties break on the name.
- `data.json` gains `describedAt` (RFC 3339 UTC, seconds). `compileIndex`
  deletes it from the `all.json` record, as it does `descDigest`.
- `index.config.json`:
  `"marketplace": {"url": "https://github.com/grimoire-rs/marketplace", "name": "grimoire"}`.
  - `url` must parse as `https:`.
  - `name` must match the ADR D4 grammar.
- Rendering of `url`: `owner/repo` iff the host is exactly `github.com` and
  the path has exactly two non-empty segments, else the full URL. Text
  interpolation only, never `set:html`.
- The route `/marketplace/` exists iff `marketplace` is non-null. It is
  built after `compileIndex`, and it joins the sitemap.
- Optional `clients` (R2-2), default `["claude","copilot","codex","qoder"]`:
  one row per listed client; the Cursor row (dashboard import, no CLI) only
  when `cursor` is listed. No fallback-harness rows.
- `url` is https-only with no userinfo (stricter than `validateUrlShape`).

---

## 6. Data Model

### Entity Relationship

```
marketplace.toml ──1:N──▶ PluginDecl ──resolves──▶ marketplace.lock part (GrimoireLock)  [include plugins]
        │                       └──────────────▶ project grimoire.lock                  [project plugins]
        └──1:1──▶ MarketplaceMeta ──1:N──▶ selected client
selected client ──1:1──▶ marketplace file ──1:N──▶ entry{name, source=./client/plugin, version, description}
                └──1:N──▶ tree ./client/plugin  (derived; ownership by convention)
```

### Key Entities

| Entity | Owner | Lifetime |
|---|---|---|
| `marketplace.toml` | curator | hand-edited |
| `marketplace.lock` | `grim update --marketplace` / export (phase-1 rule) | per pin move |
| marketplace files | `grim export marketplace` | per entry change |
| `<client>/<plugin>/` | `grim export marketplace` | per render change |

### Data Migration Strategy

None: all files are new, and no state file exists.

- An existing repo with foreign `./<client>/` dirs or marketplace files
  needs one reviewed `--force` run.
- Phase-1 manifests without `[marketplace]` are unaffected.
- A grim older than release 2 exits 65 on a manifest carrying
  `[marketplace]`. Upgrade pinned pipelines first (ADR Migration).

---

## 7. Security Architecture

### Authentication & Authorization

- Registry pulls use the phase-1 access seam: credential helpers, plus
  `grim login` in the job for private sources. Login runs only for
  same-repo PR heads in the verification job.
- Repo writes:
  - GitHub: a down-scoped App installation token (preferred) or
    `GITHUB_TOKEN`.
  - GitLab: a project access token, Developer for MR mode.
  - The token is scoped to the marketplace repo. No credential for it exists
    anywhere else, since the index never dispatches to the marketplace.

### Data Protection

- No secrets in any written file.
- Tokens are never printed (git credential helper / env only).
- Reports carry pins and paths only, and they are written outside the
  checkout.

### Security Considerations

| Threat | Control |
|---|---|
| Compromised upstream artifact | Out of scope (`adr_artifact_trust_model.md:83-85`). PR review shrinks the window and is not a control. Merge = publish (Copilot enterprise `autoUpdate`, Cursor refresh). Push mode documented as unreviewed |
| Symlinked owned dir or ancestor redirects writes/deletes outside the root | `symlink_metadata` on every ancestor; reparse points refused; tree roots never followed; plain `atomic_write` for marketplace files |
| Logo / `path:` member reads outside the repo | resolved inside the manifest dir, symlinks refused, reads capped by bytes read |
| Checkout breaks on another OS | case-collision and reserved-name refusal before placement |
| Concurrent runs on one root interleave placement | output-root advisory lock; job concurrency group / `resource_group` |
| `../` or absolute `source` | `source` built from `tree_rel` over validated names |
| Bot commits unrelated, LFS or oversize files | `-z` porcelain gate, exact allow-list from the export JSON, LFS + size checks |
| Hand-edited tree or stale file in a PR | verification job: in-place render + `git status` over the owned pathspecs (R2-1) |
| Fork PR steals registry credentials | `pull_request` only; credentialed steps same-repo only |
| Script injection via branch name, inputs or report fields | no `${{ }}` in `run:`; `--body-file` from re-validated fields, capped |
| Bot token over-privilege | App down-scoped, no `workflows`, key in a default-branch Environment; GitLab Developer for MR mode; keepalive job `actions: write` only |
| Review bypass | documented prerequisites: protected branch, required check, CODEOWNERS, dismiss stale approvals, bot not a bypass actor |
| Compromised grim download | sha256 literal in the workflow; attestations (ADR open question 3) |
| Compromised action/component | pin by full SHA |
| Landing-page injection | `owner/repo` only for exact `github.com` two-segment paths; escaped text; no `set:html` |
| Name impersonation of an official marketplace | reserved-name rule in `[marketplace].name` |

---

## 8. Non-Functional Requirements

### Performance

| Path | Target | Mechanism |
|---|---|---|
| Marketplace job, any run | ~1–3 min for tens of plugins | full render, fetch once per plugin, render per client; no commit when bytes are equal |
| Index enrich, 400 unchanged | ~1–3 min (from ~13 min extrapolated) | ~800 HEAD probes + ~58 slice describes, 8 workers |

### Scalability

The marketplace is a curated set: tens of plugins × ≤4 clients, rendered in
the marketplace repo's CI, never in the index build. Beyond about 200
plugins, measure a run before adding the deferred skip. The index holds 400+
artifacts, which means about 800 probe spawns in a bounded pool, with
concurrency tunable for rate-limited registries.

### Availability & Reliability

- Served by the git forge; grim runs only in CI.
- A failed job leaves the last commit valid.
- A partial placement self-heals on the next run, because every run
  re-renders everything.
- GitHub's 60-day disable of a public repo's schedule is countered by the
  keepalive job. It is unverified, so there is a validation item plus a
  documented manual re-enable.

### Observability

- The `--format json` reports from update and export feed the PR/MR body and
  the job log.
- Exit codes classify failures per `quality-rust-exit_codes.md`.

---

## 9. Infrastructure

### Deployment Architecture

No new infrastructure. The marketplace repo lives on GitHub
(`grimoire-rs/marketplace`, the public seed, documented workflow) or on
GitLab (internal marketplaces, component). The index site is unchanged apart
from the new route.

### Environment Configuration

| Setting | Where |
|---|---|
| grim version + sha256 | component inputs / workflow env literal |
| token | CI secret (App private key in an Environment, or project access token) |
| schedule | GitLab pipeline schedule / workflow `cron` |
| marketplace URL for the index | `index.config.json` `marketplace` |

### CI/CD Pipeline

Landing order (ADR Migration):

1. indexer enrich;
2. grim release 1 (version rule);
3. Cursor probe spike;
4. grim release 2 (`export marketplace`);
5. component;
6. seed repo;
7. indexer landing page;
8. index config.

---

## 10. Dependencies

### Internal Dependencies

| Consumer | Depends on |
|---|---|
| component / workflow | grim release 2 |
| `grimoire-rs/marketplace` | documented workflow, grim release 2 |
| index landing page | a live marketplace repo URL |

### External Dependencies

- Harness marketplace readers: paths per ADR D2, research-dated 2026-09-28;
  Cursor pending its probe.
- GitHub Actions / GitLab CI, and `actions/create-github-app-token`.
- `claude` CLI, for `claude plugin validate` in the verification job.

### Dependency Diagram

```
grim release 1 ──▶ grim release 2 ──▶ components (GitLab) ──▶ internal marketplaces
                        └─────────▶ docs workflow ──▶ grimoire-rs/marketplace ──▶ index config ──▶ /marketplace/
indexer enrich (independent)                     indexer landing page ─┘
```

---

## 11. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Rendered bytes differ across OS, versions flap | low | high | Linux-only job; Linux vs Windows version test |
| Cursor rejects the Agent Plugins tree | medium | medium | probe first; `.cursor-plugin/plugin.json` if required; out of the default set until it passes |
| Fallback harnesses (Qoder, Junie, OpenClaw, Droid) mis-handle the Claude tree | medium | medium | best-effort wording; fallback smoke tests; own files deferred (ADR D13) |
| Codex double-lists via `.claude-plugin` import | medium | low | smoke on ≥0.157; document |
| 60-day disable not reset by the enable call | medium | medium | validation on a throwaway repo; documented manual re-enable |
| README/`ONRAMP` wording change re-versions everything | certain per change | low | golden test; release note |
| Registry rate limits on full renders | low | medium | curated set, daily schedule, optional login |
| GitLab token expiry (365 d) | certain | medium | expiry noted in component docs; job fails loudly on 401 |

---

## 12. Implementation Phases

Phases by repository, not tasks (the plan decomposes):

1. **Indexer enrich** — checkpoint seed fix, probe-first `enrichOne`, pool,
   rotating slice, `describedAt` stripping.
2. **grim release 1** — D7 version rule:
   - canonical inventory hash and source exec bit;
   - the codex counterexample and the README/`ONRAMP` golden;
   - `test_s006`, both `_suffix` helpers and `test_s019` updated;
   - docs and release note.
3. **Plan spike** — Cursor probe.
4. **grim release 2** — `[marketplace]` in the shared loader,
   `marketplace_export.rs`, the stage/place split, containment hardening,
   report, docs, catalog drift review.
5. **Components + workflow** — GitLab component (regenerate + verify);
   GitHub workflow in the guide.
6. **Seed + landing page** — `grimoire-rs/marketplace`, indexer route,
   `grimoire-index` config.

---

## 13. Open Questions

See ADR › Open questions:

1. whether v1 ships stateless and defers the incremental skip;
2. a GitHub App or `GITHUB_TOKEN` for the public seed;
3. release attestations.

Everything else is decided in the ADR.

---

## Appendix

### Glossary

| Term | Meaning |
|---|---|
| Marketplace file | A harness's catalog JSON at its own path in the repo |
| Tree | `<client>/<plugin>/`, one plugin rendered for one client |
| Owned path | A selected client's `./<client>/`, and a marketplace file that is absent or named `[marketplace].name` |
| Inventory | Sorted `(entry name, exec bit, sha256)` of a tree's regular files; the compare key and the version input |
| Fallback client | A harness that reads `.claude-plugin/marketplace.json` and installs the Claude tree (Qoder, Junie, OpenClaw, Droid) |

### References

- `.agents/research/research_marketplace_manifest_schemas.md`
- `.agents/research/research_marketplace_multi_harness_root.md`
- `.agents/research/research_derived_version_compat.md`
- `.agents/research/research_marketplace_ci_write_path.md`
- `.agents/research/research_marketplace_phase2_recon.md`
- `.agents/research/research_incremental_pages_builds.md`

## Approval

| Role | Name | Date |
|---|---|---|
| Owner | — | pending |

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-09-28 | Architect (hex-architect xhigh) | Initial draft |
| 2026-09-29 | hex-plan (planning pass) | Accepted; Round-2 amendments (ADR R2-1…R2-27): in-place verification, `--no-renames` gate, Qoder table client and default, Cursor opt-in with Claude-shape `.cursor-plugin/plugin.json`, files-first order, pruning and rename rules, landing-page `clients` |
| 2026-09-29 | Architect (review-fix round 1) | Stateless regeneration; state file, `--client`, `NoMarketplaceFile`, Qoder mapping, mode normalization removed; canonical inventory hash; containment hardening; job/verification/security rewrite; rotating indexer slice (ADR Amendments 1–33) |
