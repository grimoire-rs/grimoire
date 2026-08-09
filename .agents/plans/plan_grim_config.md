# Plan: `grim config` command

## Status

- **Plan:** plan_grim_config
- **Active phase:** 6 — swarm-review (final gate)
- **Step:** finalized
- **Last update:** 2026-06-30 (after 36b23a5: swarm-review Approved; all in-scope findings fixed; task verify green)

---

## Overview

**Status:** Approved
**Author:** /swarm-plan (autonomous)
**Date:** 2026-06-30
**Related ADR:** [`adr_grim_config_command.md`](../adr/adr_grim_config_command.md)

## Objective

Add a `grim config` command — a git-style CLI to read and write `grimoire.toml`
**settings and registries** at project or global scope. Hybrid surface under one
umbrella: explicit `get`/`set`/`unset`/`list` over dotted keys, plus a nested
`config registry add|rm|use|show|list` verb group. Migration-script-friendly:
explicit verbs, `--format json`, stable exit codes.

## Scope

### In Scope

- New `grim config` subcommand with nested subcommands (clap derive).
- Dotted-key get/set/unset/list over `[options]` (`clients`, `default_registry`,
  `tui.default_view`, `tui.group_by_type`, `tui.tree_separators`) and
  `registry.<alias>.{url,default}`.
- `config registry add|rm|use|show|list`.
- `--global` scope flag (else project, walk-up / `--config`); `--format json`.
- Report types in `src/api/config_report.rs`.
- Unit + acceptance tests; docs (`commands.md`, `configuration.md`) + CHANGELOG;
  catalog drift review (`grim-usage`, `grim-authoring`).

### Out of Scope

- `[skills]` / `[rules]` / `[agents]` / `[bundles]` declarations (owned by
  `add`/`remove`/`install`/`lock` — lockfile coupling).
- Comment / `#:schema` preservation (reuse existing lossy `write_config`;
  `toml_edit` upgrade is a separate future change).
- Auth (stays in `login`/`logout`). Implicit positional get/set form (rejected).
- `--type` validation, `--show-scope`, multi-valued keys.

## Research

Covered by the ADR's "Industry Context & Research" (git/cargo/gh/npm/docker/
kubectl/aws survey). No further research needed. Key takeaways encoded in design:
explicit verbs (git deprecated implicit), `use` verb for set-default, `--global`
scope flag, `key=value` + JSON output.

## Technical Approach

### Architecture Changes

```
src/main.rs            Command::Config(ConfigArgs)                          [EDIT]
src/command.rs         pub mod config;                                      [EDIT]
src/app.rs             dispatch arm: command::config::run -> render         [EDIT]
src/command/config.rs  ConfigArgs { #[command(subcommand)] ConfigCommand }  [NEW]
                       ConfigCommand::{Get,Set,Unset,List,Registry}
                       RegistryCommand::{Add,Rm,Use,Show,List}
                       async fn run(ctx,args) -> Result<(ConfigReport,ExitCode)>
src/api/config_report.rs  report enums + Printable impls                    [NEW]
src/api.rs             pub mod config_report; re-exports                    [EDIT]
src/config/project_config.rs  expose validate_registries as pub(crate)      [EDIT]
```

Reuse seams (do NOT reinvent — verified signatures):
- `crate::command::scope_resolution::resolve(ctx, global, config) -> Result<ResolvedScope, ConfigError>`
  — gives `.options`, `.registries`, `.set`, `.config_path`.
- `crate::command::add::write_config(path: &Path, options: &ConfigOptions,
  registries: &[RegistryConfig], set: &DesiredSet) -> Result<(), ConfigError>`
  — atomic, preserves declarations + legacy field.
- `crate::config::project_config::validate_registries(registries: &[RegistryConfig],
  path: &Path) -> Result<(), ConfigError>` — **make `pub(crate)`**; call before every write.
- `crate::lock::file_lock::ConfigFileLock::try_acquire(&path)` — flock the RMW.
- `super::grim(result)` (error→classified wrap), usage-error helper pattern (cf.
  `super::login_usage`); add a `config_usage(&'static str)` sibling in `command.rs`
  or reuse the existing usage helper.
- Output: `crate::cli::printer::{Printable, print_table}`; `ExitCode` from
  `crate::cli::exit_code`.

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| Nested `config registry` (not top-level `registry`) | Maintainer steer; registries are `grimoire.toml` content (unlike auth) |
| Explicit verbs only | git deprecated implicit; unambiguous; best for scripts |
| Reuse `write_config` (lossy) | No new regression (add/remove already lossy); `toml_edit` deferred |
| Validate before every write | `write_config` doesn't validate; invariants (alias rules, at-most-one-default) must hold |
| Dotted `set registry.x.*` requires existing entry; create only via `registry add` | Keeps url-required + validation in one path; no half-built entries |
| `get` of unset key → exit 1, no stdout | git-compatible script contract (`grim config get x || default`) |
| `registry use` / `set …default true` clears prior default | enforce at-most-one-default atomically |

## Implementation Steps

> Contract-First TDD: Stub → Verify → Specify → Implement → Review.

### Phase 1: Stubs

- [ ] **Step 1.1:** clap arg tree in `src/command/config.rs`.
  - Files: `src/command/config.rs` (new), `src/command.rs`, `src/main.rs`, `src/app.rs`
  - Public API:
    ```rust
    #[derive(Debug, Args)] pub struct ConfigArgs {
        #[command(subcommand)] pub command: ConfigCommand,
        /// Operate on the global config ($GRIM_HOME/grimoire.toml).
        #[arg(long, global = true)] pub global: bool,
    }
    #[derive(Debug, Subcommand)] pub enum ConfigCommand {
        Get   { key: String },
        Set   { key: String, value: String },
        Unset { key: String },
        List  { #[arg(long)] show_origin: bool },
        Registry(RegistryArgs),
    }
    #[derive(Debug, Args)] pub struct RegistryArgs {
        #[command(subcommand)] pub command: RegistryCommand,
    }
    #[derive(Debug, Subcommand)] pub enum RegistryCommand {
        Add  { alias: String, #[arg(long)] url: String, #[arg(long)] default: bool },
        Rm   { alias: String },
        Use  { alias: String },
        Show { alias: String },
        List,
    }
    pub async fn run(ctx: &Context, args: &ConfigArgs)
        -> anyhow::Result<(ConfigReport, ExitCode)>;  // body: unimplemented!()
    ```
  - Note: `--global`/`--config` already exist on `GlobalOptions` (`ctx`/global flags);
    resolve scope via `scope_resolution::resolve(ctx, args.global || ctx.global, args.config)`.
    Confirm whether `--global` should be read from `GlobalOptions` (as `lock`/`install` do)
    rather than re-declared on `ConfigArgs` — prefer the existing global flag for consistency;
    drop the local `global` field if so.

- [ ] **Step 1.2:** Report types in `src/api/config_report.rs` (+ `src/api.rs`).
  - Public API (Printable each; follow `login_report.rs`):
    ```rust
    pub enum ConfigReport {                  // dispatched in app.rs render arm
        Get(ConfigGetReport),
        Write(ConfigWriteReport),            // set/unset/registry add/rm/use (arch-verify A1)
        List(ConfigListReport),
        RegistryList(RegistryListReport),
        RegistryShow(RegistryShowReport),
    }
    pub struct ConfigGetReport { pub key: String, pub value: Option<String> }  // None=unset
    pub enum WriteAction { Set, Unset, RegistryAdded, RegistryRemoved, RegistryDefault }  // typed
    pub struct ConfigWriteReport { pub action: WriteAction, pub key: String,
                                   pub value: Option<String>, pub scope: Origin }
    pub struct ConfigListReport { pub entries: Vec<ConfigEntry>, pub show_origin: bool }
    pub struct ConfigEntry { pub key: String, pub value: String, pub origin: Origin }
    pub struct RegistryListReport { pub rows: Vec<RegistryRow> }  // alias,url,default marker
    pub struct RegistryShowReport { pub alias: String, pub url: String, pub default: bool }
    ```
    `Origin` and `WriteAction` are `Serialize + Display` enums, never Strings
    (`subsystem-cli-api.md` typed-enum rule). One unified write-confirmation
    report (`Write`) covers set/unset/registry add/rm/use — resolved arch-verify
    findings A1 (missing registry-write variants) and S1 (`scope` was `&str`).
  - Gate: `cargo check` passes.

### Phase 2: Architecture Review

Reviewer (`spec-compliance`, `post-stub`): arg tree ↔ ADR surface; report types
cover all subcommands; `get` value-only contract representable; scope flag wired
once. >3 files touched, so this phase runs (not optional).

### Phase 3: Specification Tests

- [ ] **Step 3.1:** Unit tests (inline `#[cfg(test)]`).
  - `src/command/config.rs`: clap parse harness (cf. login tests) — every
    subcommand + flag parses; bad arity rejected. Key-mapping helper: dotted key
    → field for each supported key; unknown key → UsageError; bad enum/bool/sep →
    DataError. `registry use`/`set default true` produces a vec with exactly one
    default. Dotted `set registry.x.url` on missing alias → UsageError.
  - `src/api/config_report.rs`: plain (`get` bare value when set; empty when
    unset; `list`/`registry list` one `print_table`) + JSON shapes.
  - `src/config/project_config.rs`: `validate_registries` reachable as `pub(crate)`.
- [ ] **Step 3.2:** Acceptance tests.
  - `test/tests/test_config.py` (new): `set`→`get` round-trip for
    `options.clients`, `options.tui.default_view`, `options.default_registry` at
    project and `--global`; `unset`; `list` and `list --show-origin`; JSON output;
    exit codes (unset get→1, unknown key→64, bad enum→65).
  - `test/tests/test_config_registry.py` (new): `registry add`/`list`/`show`/
    `use`/`rm`; at-most-one-default after `use`; dup add→64; missing-alias ops→64;
    `--global registry add` then a short-id `add` resolves end-to-end.
  - Gate: tests compile/parse and fail against stubs (`unimplemented!()`).

### Phase 4: Implementation

- [ ] **Step 4.1:** Key parsing + value codecs (clients list, tui enum/bool/
  separators, registry fields). Map unknown key→`UsageError`, bad value→`DataError`.
- [ ] **Step 4.2:** Read path: `get`, `list` (+`--show-origin`), `registry list`,
  `registry show` from the resolved scope.
- [ ] **Step 4.3:** Write path: load → `ConfigFileLock::try_acquire` → mutate the
  `ConfigOptions`/`Vec<RegistryConfig>` (keep `set` untouched) → `validate_registries`
  → `write_config`. Covers `set`, `unset`, `registry add|rm|use`.
- [ ] **Step 4.4:** `run` dispatch + report construction; `app.rs` render arm.
  - Gate: all unit + acceptance tests pass; `task verify`.

### Phase 5: Review & Documentation

- [ ] **Step 5.1:** Spec-compliance review (ADR ↔ tests ↔ impl).
- [ ] **Step 5.2:** Quality review (`subsystem-cli-api.md` single-table rule;
  exit-code map; error message style `quality-rust-errors.md`).
- [ ] **Step 5.3:** Docs: `docs/src/commands.md` (new `config` section incl.
  `{#config}` anchor), `docs/src/configuration.md` (point hand-edit guidance at
  `grim config`; note the `login`/`logout` known limitation interplay), CHANGELOG;
  `subsystem-cli-commands.md` index row. Catalog drift review (`catalog/README.md`):
  `grim-usage`, `grim-authoring`; `task catalog:verify`.

## Files to Modify

| File | Action | Description |
|------|--------|-------------|
| `src/command/config.rs` | Create | command, arg tree, run dispatch |
| `src/api/config_report.rs` | Create | report types + Printable |
| `src/command.rs` | Modify | `pub mod config;` (+ optional `config_usage` helper) |
| `src/main.rs` | Modify | `Command::Config(ConfigArgs)` |
| `src/app.rs` | Modify | dispatch + render arm |
| `src/api.rs` | Modify | `pub mod config_report;` + re-exports |
| `src/config/project_config.rs` | Modify | `validate_registries` → `pub(crate)` |
| `test/tests/test_config.py` | Create | settings acceptance tests |
| `test/tests/test_config_registry.py` | Create | registry acceptance tests |
| `docs/src/commands.md` | Modify | `config` command section |
| `docs/src/configuration.md` | Modify | reference `grim config` |
| `.claude/rules/subsystem-cli-commands.md` | Modify | index row |
| `CHANGELOG.md` | Modify | Added entry |
| `catalog/skills/grim-usage/**`, `grim-authoring/**` | Modify | drift review |

## Dependencies

### Code Dependencies

| Package | Version | Purpose |
|---------|---------|---------|
| (none new) | — | reuse `toml`, clap, serde already present |

### Service Dependencies

| Service | Status | Notes |
|---------|--------|-------|
| OCI registry (acceptance tests) | Available | env runs full `task verify` (317 tests pass) |

## Testing Strategy

### Unit Tests (from component contracts)

| Component | Behavior | Expected | Edge Cases |
|-----------|----------|----------|------------|
| key parser | dotted key → field | correct field | unknown key→64; `registry.<a>.<f>` split |
| value codec | parse value to typed | typed value | bad enum/bool/sep→65; clients comma-split; empty⇒unset |
| registry mutate | add/rm/use | valid vec | dup alias→64; missing alias→64; at-most-one-default |
| write path | mutate→validate→write | file round-trips | validate rejects bad set before write |
| reports | plain + json | one table; bare get value | unset get empty + exit 1 |

### Acceptance Tests (from user experience)

| User Action | Expected Outcome | Error Cases |
|-------------|------------------|-------------|
| `config set options.clients claude,opencode` | written; `get` returns it | unknown key→64 |
| `config get options.tui.default_view` (unset) | no stdout, exit 1 | bad value on set→65 |
| `config --global set …` | writes `$GRIM_HOME/grimoire.toml` | — |
| `config registry add acme --url ghcr.io/acme --default` | entry added, default | dup alias→64 |
| `config registry use acme` | acme default, others cleared | missing alias→64 |
| `config list --show-origin` | key=value + origin, one table | — |

### Manual Testing

- [ ] `grim config --help` and `grim config registry --help` enumerate verbs.
- [ ] JSON output parses (`grim config list --format json | jq`).

## Risks

| Risk | Mitigation |
|------|------------|
| Write an invalid registry set | `validate_registries` before every `write_config`; unit + acceptance lock it |
| Comment/`#:schema` loss on write | documented; pre-existing behavior; `toml_edit` deferred follow-up |
| `--global` flag double-declared (ConfigArgs vs GlobalOptions) | prefer existing `GlobalOptions.global`; drop local field (Step 1.1 note) |
| Scope confusion (which file written) | `ConfigSetReport.scope` echoes target; acceptance asserts file path |

## Checklist

### Before Starting
- [x] ADR approved
- [x] Dependencies available
- [x] Branch created (`feat/grim-config-command`)

### Before PR
- [ ] All tests passing
- [ ] No linting errors
- [ ] Documentation updated
- [ ] Catalog drift review done (`task catalog:verify`)

## Notes

- `validate_registries` is currently private to `project_config.rs` (line 184) —
  exposing it `pub(crate)` is the one cross-module change; alternative (re-parse
  after write) validates too late and is rejected.
- `write_config` preserves `set` (declarations) and the legacy `default_registry`;
  the config command passes `scope.set` through untouched.

---

## Progress Log

| Date | Update |
|------|--------|
| 2026-06-30 | Plan authored from accepted ADR; tier=high; ready for /swarm-execute |
| 2026-06-30 | Execute: stub→arch-verify→specify→implement; 3-round review-fix loop (TOML-injection Block + 14 findings fixed); committed |
| 2026-06-30 | swarm-review: architect+cli-ux+spec+Codex panel; Approved after Warn fixes (drop --show-origin, alias→64, dotted-alias, empty-client); amended 36b23a5; awaiting /finalize |

## Deferred Findings (human judgment / follow-ups)

1. **(Codex Block, pre-existing) binding-name TOML KEY injection in `write_config`** — the artifact-table *keys* (`[skills]`/`[rules]`/`[agents]`/`[bundles]` binding names) are still written raw; a `grim add --name` with TOML-special chars corrupts the file (mostly self-rejecting on next load, not a clean value injection). Out of `grim config` scope (declarations path). Fix: validate binding names in `grim add`, or move `write_config` to a serializer/`toml_edit`.
2. **TOCTOU read-before-lock** — `run_set`/handlers resolve scope before acquiring the flock; matches grim's existing single-writer pattern (lock.rs) + fail-fast exit 75. Broader concurrency hardening = separate change.
3. **tree-separator codec duplication** — `parse_tree_separators` (command) mirrors `validate_tree_separators` (load); no current hole (zero-width closed + tested), drift risk only. Follow-up: lift `validate_tree_separators` into `commit_config`.
4. **merge-vs-literal reads** — `config get`/`list` are scope-literal, not effective-resolution (diverge from git `get`). Possible `config get --effective` — ADR follow-up.
5. **toml_edit comment/`#:schema` preservation** — config writes reuse the lossy `write_config` (drops comments). ADR-tracked follow-up benefiting all writers.
6. **cli-ux nits** — `registry list` default `*`-marker (D2), `--url` positional vs flag (D3) — deferred per ADR consistency.
