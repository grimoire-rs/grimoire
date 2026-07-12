// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim config` — git-style CLI to read and write `grimoire.toml`.
//!
//! Hybrid surface: explicit `get`/`set`/`unset`/`list` over dotted keys,
//! plus nested `config registry add|rm|use|show|list` for registry
//! lifecycle.  All under one `config` umbrella (see
//! `adr_grim_config_command.md`).
//!
//! Scope is selected by the root `--global` / `--config` flags, read off
//! [`Context`] and passed to `scope_resolution::resolve` — the same
//! pattern every scope-aware command (`lock`, `install`) follows.

use clap::{Args, Subcommand};
use unicode_width::UnicodeWidthChar as _;

use crate::api::config_report::{
    ConfigEntry, ConfigGetReport, ConfigListReport, ConfigReport, ConfigWriteReport, Origin, RegistryListReport,
    RegistryRow, RegistryShowReport, WriteAction,
};
use crate::cli::exit_code::ExitCode;
use crate::config::declaration::{ConfigOptions, DefaultView, RegistryConfig};
use crate::config::project_config::validate_registries;
use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::lock::file_lock::ConfigFileLock;

use super::config_keys::{ConfigKey, KeySpec, RegistryField};
use super::scope_resolution::{self, lockable_config_path};

/// `grim config` arguments.
///
/// The root `--global` / `--config` scope flags apply to the whole command
/// tree and work positionally before or after the subcommand: `grim config
/// --global get <key>` or `grim config get <key> --global`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// The `config` subcommand tree.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the value of a single dotted key.
    Get {
        /// Dotted key, e.g. `options.clients` or `registry.acme.oci`.
        key: String,
    },
    /// Set a dotted key to a value.
    Set {
        /// Dotted key to set.
        key: String,
        /// New value (parsed to the field's type).
        value: String,
    },
    /// Remove a dotted key (or a whole registry entry when the key names
    /// a `registry.<alias>` without a trailing field).
    Unset {
        /// Dotted key to unset.
        key: String,
    },
    /// List all effective key=value pairs for the scope.
    ///
    /// Each invocation reads from exactly one scope, so origin information
    /// is implicit (use `--global` or `--config` to select the scope).
    List {
        /// Include every supported key, including unset ones.
        #[arg(long)]
        all: bool,
    },
    /// Manage `[[registries]]` entries.
    #[command(subcommand_value_name = "REGISTRY_COMMAND")]
    Registry(RegistryArgs),
}

/// `grim config registry` arguments.
#[derive(Debug, Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    pub command: RegistryCommand,
}

/// The `config registry` subcommand tree.
#[derive(Debug, Subcommand)]
pub enum RegistryCommand {
    /// Add a registry or package-index entry (exactly one of --oci / --index).
    Add {
        /// Alias to assign (must be non-empty, no `/`, no surrounding whitespace).
        alias: String,
        /// Plain OCI registry ref (lists packages via the OCI `_catalog`
        /// endpoint). `--url` is accepted as a hidden pre-0.7.0 alias.
        #[arg(long, alias = "url")]
        oci: Option<String>,
        /// Package-index locator (http(s):// static base, or a git repository);
        /// replaces the `_catalog` listing — index entries carry their own
        /// registry refs.
        #[arg(long, conflicts_with = "oci")]
        index: Option<String>,
        /// Mark this registry as the default (clears any prior default).
        #[arg(long)]
        default: bool,
    },
    /// Remove a registry entry by alias.
    Rm {
        /// Alias of the registry to remove.
        alias: String,
    },
    /// Mark a registry as the default (clears any prior default).
    Use {
        /// Alias of the registry to make the default.
        alias: String,
    },
    /// Show all fields for a single registry.
    Show {
        /// Alias of the registry to show.
        alias: String,
    },
    /// List all registries in the scope (default marked).
    List,
}

/// Run `grim config`.
///
/// `get` of a valid-but-unset key returns `(ConfigReport::Get, ExitCode::Failure)`
/// with no stdout — git-compatible so `grim config get <key> || default`
/// works in scripts. This is a non-error exit, not a `Result::Err`.
///
/// # Errors
///
/// Unknown key (UsageError 64), invalid value (DataError 65), config parse
/// failure (ConfigError 78), missing config (NotFound 79), write / lock
/// failure (IoError 74), or alias not found (UsageError 64).
pub async fn run(ctx: &Context, args: &ConfigArgs) -> anyhow::Result<(ConfigReport, ExitCode)> {
    match &args.command {
        ConfigCommand::Get { key } => run_get(ctx, key),
        ConfigCommand::Set { key, value } => run_set(ctx, key, value),
        ConfigCommand::Unset { key } => run_unset(ctx, key),
        ConfigCommand::List { all } => run_list(ctx, *all),
        ConfigCommand::Registry(r) => match &r.command {
            RegistryCommand::Add {
                alias,
                oci,
                index,
                default,
            } => run_registry_add(ctx, alias, oci.as_deref(), index.as_deref(), *default),
            RegistryCommand::Rm { alias } => run_registry_rm(ctx, alias),
            RegistryCommand::Use { alias } => run_registry_use(ctx, alias),
            RegistryCommand::Show { alias } => run_registry_show(ctx, alias),
            RegistryCommand::List => run_registry_list(ctx),
        },
    }
}

// ── Key parsing ──────────────────────────────────────────────────────────────

/// A parsed dotted config key.
#[derive(Debug, PartialEq, Eq)]
enum ParsedKey {
    /// One of the 7 fixed `options.*` keys — see [`ConfigKey`].
    Fixed(ConfigKey),
    /// `registry.<alias>` — valid only for `unset` (removes the whole entry).
    RegistryAlias { alias: String },
    /// `registry.<alias>.<field>`.
    RegistryAliasField { alias: String, field: RegistryField },
}

fn parse_key(key: &str) -> anyhow::Result<ParsedKey> {
    if let Some(k) = ConfigKey::parse(key) {
        return Ok(ParsedKey::Fixed(k));
    }
    if let Some(rest) = key.strip_prefix("registry.") {
        // FIX 2: split at the RIGHTMOST dot so aliases containing dots
        // (e.g. `a.b`) are addressable: `registry.a.b.oci` → alias=`a.b`,
        // field=`oci`.  The field must be exactly `oci`, `index`, or `default`
        // (`url` accepted as the pre-0.7.0 alias for `oci`).
        if let Some(dot_pos) = rest.rfind('.') {
            let alias = &rest[..dot_pos];
            let field_str = &rest[dot_pos + 1..];
            if !alias.is_empty() && !field_str.is_empty() {
                let field = match field_str {
                    "oci" | "url" => RegistryField::Oci,
                    "index" => RegistryField::Index,
                    "default" => RegistryField::Default,
                    other => {
                        return Err(super::config_usage(format!(
                            "unknown registry field '{other}'; valid fields: oci, index, default"
                        )));
                    }
                };
                // FIX 1: validate alias format at CLI boundary (exit 64) so
                // a bad alias never reaches validate_registries (exit 78).
                validate_alias_format(alias)?;
                return Ok(ParsedKey::RegistryAliasField {
                    alias: alias.to_string(),
                    field,
                });
            }
        } else if !rest.is_empty() {
            return Ok(ParsedKey::RegistryAlias {
                alias: rest.to_string(),
            });
        }
    }
    Err(super::config_usage(format!(
        "unknown config key '{key}'; valid keys: {}",
        super::config_keys::valid_keys()
    )))
}

fn scope_to_origin(scope: ConfigScope) -> Origin {
    match scope {
        ConfigScope::Global => Origin::Global,
        ConfigScope::Project => Origin::Project,
    }
}

// ── Value getters ─────────────────────────────────────────────────────────────

/// The effective value of a fixed `options.*` key, or `None` when unset —
/// including the None-when-default collapse (`false` bools, empty lists)
/// so a value indistinguishable from its default on disk reads back as
/// unset, consistent across `get` / `list` / `unset`.
fn fixed_value(key: ConfigKey, options: &ConfigOptions) -> Option<String> {
    match key {
        ConfigKey::Clients => {
            if options.clients.is_empty() {
                None
            } else {
                Some(options.clients.join(","))
            }
        }
        ConfigKey::DefaultRegistry => options.default_registry.clone(),
        ConfigKey::ShowDeprecated => {
            // `false` is the default and indistinguishable from unset on disk —
            // return None so `get` exits 1 and `list` omits the key, consistent
            // with `group_by_type`. Setting to `false` removes the key from the
            // written config (see `apply_unset`).
            if options.show_deprecated {
                Some("true".to_string())
            } else {
                None
            }
        }
        ConfigKey::TuiDefaultView => options.tui.default_view.map(|v| match v {
            DefaultView::Flat => "flat".to_string(),
            DefaultView::Tree => "tree".to_string(),
        }),
        ConfigKey::TuiGroupByType => {
            // `false` is the default and indistinguishable from unset on disk —
            // return None so `get` exits 1 and `list` omits the key, consistent
            // with all other default-valued keys.  Setting to `false` removes the
            // key from the written config (see `apply_unset`).
            if options.tui.group_by_type {
                Some("true".to_string())
            } else {
                None
            }
        }
        ConfigKey::TuiTreeSeparators => {
            if options.tui.tree_separators.is_empty() {
                None
            } else {
                Some(options.tui.tree_separators.join(","))
            }
        }
        ConfigKey::TuiExpandLevels => options.tui.expand_levels.map(|n| n.to_string()),
    }
}

fn get_value(
    parsed: &ParsedKey,
    options: &ConfigOptions,
    registries: &[RegistryConfig],
) -> anyhow::Result<Option<String>> {
    Ok(match parsed {
        ParsedKey::Fixed(k) => fixed_value(*k, options),
        ParsedKey::RegistryAlias { alias } => {
            return Err(super::config_usage(format!(
                "no registry field specified for '{alias}'; use registry.<alias>.oci or registry.<alias>.default"
            )));
        }
        ParsedKey::RegistryAliasField { alias, field } => {
            let rc = find_registry(registries, alias).ok_or_else(|| {
                super::config_usage(format!("no registry '{alias}'; add it with `grim config registry add`"))
            })?;
            match field {
                RegistryField::Oci => rc.oci.clone(),
                RegistryField::Index => rc.index.clone(),
                RegistryField::Default => Some(rc.default.to_string()),
            }
        }
    })
}

// ── Value setters ─────────────────────────────────────────────────────────────

fn apply_set(
    parsed: &ParsedKey,
    value_str: &str,
    options: &mut ConfigOptions,
    registries: &mut [RegistryConfig],
) -> anyhow::Result<String> {
    match parsed {
        ParsedKey::Fixed(k) => match k {
            ConfigKey::Clients => {
                if value_str.is_empty() {
                    options.clients.clear();
                    Ok(String::new())
                } else {
                    let clients: Vec<String> = value_str.split(',').map(|s| s.trim().to_string()).collect();
                    for c in &clients {
                        // FIX 3: empty/whitespace-only segment (e.g. "claude, ,opencode"
                        // after split+trim) → exit 65 so the config never holds a blank
                        // client name that silently installs nothing.
                        if c.is_empty() {
                            return Err(super::config_value(
                                "options.clients: empty or whitespace-only segment; \
                                 each client name must be non-empty"
                                    .to_string(),
                            ));
                        }
                        reject_control_chars(c, "options.clients")?;
                    }
                    options.clients.clone_from(&clients);
                    Ok(clients.join(","))
                }
            }
            ConfigKey::DefaultRegistry => {
                reject_control_chars(value_str, "options.default_registry")?;
                options.default_registry = Some(value_str.to_string());
                Ok(value_str.to_string())
            }
            ConfigKey::ShowDeprecated => {
                options.show_deprecated = parse_bool(value_str, "options.show_deprecated")?;
                Ok(value_str.to_string())
            }
            ConfigKey::TuiDefaultView => {
                options.tui.default_view = Some(parse_default_view(value_str)?);
                Ok(value_str.to_string())
            }
            ConfigKey::TuiGroupByType => {
                options.tui.group_by_type = parse_bool(value_str, "options.tui.group_by_type")?;
                Ok(value_str.to_string())
            }
            ConfigKey::TuiTreeSeparators => {
                let seps = parse_tree_separators(value_str)?;
                let stored = seps.join(",");
                options.tui.tree_separators = seps;
                Ok(stored)
            }
            ConfigKey::TuiExpandLevels => {
                let levels = parse_u32(value_str, "options.tui.expand_levels")?;
                options.tui.expand_levels = Some(levels);
                Ok(levels.to_string())
            }
        },
        ParsedKey::RegistryAlias { alias } => Err(super::config_usage(format!(
            "cannot set registry '{alias}' without a field; \
             use registry.<alias>.oci or registry.<alias>.default"
        ))),
        ParsedKey::RegistryAliasField { alias, field } => {
            if find_registry(registries, alias).is_none() {
                return Err(super::config_usage(format!(
                    "no registry '{alias}'; add it with `grim config registry add`"
                )));
            }
            match field {
                RegistryField::Oci => {
                    reject_control_chars(value_str, &format!("registry.{alias}.oci"))?;
                    if find_registry(registries, alias).is_some_and(|rc| rc.index.is_some()) {
                        return Err(super::config_value(format!(
                            "registry '{alias}' is an index entry; oci and index are mutually \
                             exclusive — unset registry.{alias}.index first"
                        )));
                    }
                    set_registry_field(registries, alias, |rc| rc.oci = Some(value_str.to_string()));
                    Ok(value_str.to_string())
                }
                RegistryField::Index => {
                    reject_control_chars(value_str, &format!("registry.{alias}.index"))?;
                    if find_registry(registries, alias).is_some_and(|rc| rc.oci.is_some()) {
                        return Err(super::config_value(format!(
                            "registry '{alias}' is a registry entry; oci and index are mutually \
                             exclusive — unset registry.{alias}.oci first"
                        )));
                    }
                    if crate::config::registry_resolve::classify_index(value_str).is_none() {
                        return Err(super::config_value(format!(
                            "invalid index locator '{value_str}': must be an http(s):// base or a \
                             git repository (git+…, ssh://, git@…, or ending in .git)"
                        )));
                    }
                    set_registry_field(registries, alias, |rc| rc.index = Some(value_str.to_string()));
                    Ok(value_str.to_string())
                }
                RegistryField::Default => {
                    let b = parse_bool(value_str, &format!("registry.{alias}.default"))?;
                    if b {
                        clear_all_defaults(registries);
                    }
                    set_registry_default(registries, alias, b);
                    Ok(value_str.to_string())
                }
            }
        }
    }
}

fn apply_unset(
    parsed: &ParsedKey,
    options: &mut ConfigOptions,
    registries: &mut Vec<RegistryConfig>,
) -> anyhow::Result<()> {
    match parsed {
        ParsedKey::Fixed(k) => {
            match k {
                ConfigKey::Clients => options.clients.clear(),
                ConfigKey::DefaultRegistry => options.default_registry = None,
                ConfigKey::ShowDeprecated => options.show_deprecated = false,
                ConfigKey::TuiDefaultView => options.tui.default_view = None,
                ConfigKey::TuiGroupByType => options.tui.group_by_type = false,
                ConfigKey::TuiTreeSeparators => options.tui.tree_separators.clear(),
                ConfigKey::TuiExpandLevels => options.tui.expand_levels = None,
            }
            Ok(())
        }
        ParsedKey::RegistryAlias { alias } => {
            if !registries.iter().any(|r| r.alias.as_deref() == Some(alias.as_str())) {
                return Err(super::config_usage(format!(
                    "no registry '{alias}'; cannot remove a registry that does not exist"
                )));
            }
            registries.retain(|r| r.alias.as_deref() != Some(alias.as_str()));
            Ok(())
        }
        ParsedKey::RegistryAliasField { alias, field } => match field {
            RegistryField::Oci => {
                let Some(rc) = find_registry(registries, alias) else {
                    return Err(super::config_usage(format!(
                        "no registry '{alias}'; cannot unset a field on a registry that does not exist"
                    )));
                };
                if rc.index.is_none() {
                    return Err(super::config_usage(format!(
                        "cannot unset registry.{alias}.oci: the entry would have no source; \
                         set registry.{alias}.index first or use `grim config registry rm {alias}`"
                    )));
                }
                set_registry_field(registries, alias, |rc| rc.oci = None);
                Ok(())
            }
            RegistryField::Index => {
                let Some(rc) = find_registry(registries, alias) else {
                    return Err(super::config_usage(format!(
                        "no registry '{alias}'; cannot unset a field on a registry that does not exist"
                    )));
                };
                if rc.oci.is_none() {
                    return Err(super::config_usage(format!(
                        "cannot unset registry.{alias}.index: the entry would have no source; \
                         set registry.{alias}.oci first or use `grim config registry rm {alias}`"
                    )));
                }
                set_registry_field(registries, alias, |rc| rc.index = None);
                Ok(())
            }
            RegistryField::Default => {
                if find_registry(registries, alias).is_none() {
                    return Err(super::config_usage(format!(
                        "no registry '{alias}'; cannot unset default on a registry that does not exist"
                    )));
                }
                set_registry_default(registries, alias, false);
                Ok(())
            }
        },
    }
}

// ── List collector ────────────────────────────────────────────────────────────

/// Build one [`ConfigEntry`] from a resolved key/value pair and its static
/// [`KeySpec`] — the sole adapter between the command layer (which knows
/// about `KeySpec`) and the API layer (which stays ignorant of it).
fn entry(key: String, value: Option<String>, spec: &'static KeySpec) -> ConfigEntry {
    ConfigEntry::new(key, value, spec.value_type, spec.title, spec.description, spec.default)
}

/// Collect the rows for `grim config list`. `all` widens the row set to
/// include supported-but-unset keys (fixed keys always unset-eligible;
/// registry `oci`/`index` locator rows only for existing aliased entries);
/// it never changes the row shape (see `ConfigEntry`).
fn collect_entries(all: bool, options: &ConfigOptions, registries: &[RegistryConfig]) -> Vec<ConfigEntry> {
    let mut entries = Vec::new();
    for k in ConfigKey::ALL {
        let value = fixed_value(k, options);
        if value.is_some() || all {
            entries.push(entry(k.spec().key.to_string(), value, k.spec()));
        }
    }
    for rc in registries {
        if let Some(alias) = &rc.alias {
            let oci_spec = RegistryField::Oci.spec();
            if rc.oci.is_some() || all {
                entries.push(entry(format!("registry.{alias}.oci"), rc.oci.clone(), oci_spec));
            }
            let index_spec = RegistryField::Index.spec();
            if rc.index.is_some() || all {
                entries.push(entry(format!("registry.{alias}.index"), rc.index.clone(), index_spec));
            }
            // `default` always has an effective value — no unset state.
            entries.push(entry(
                format!("registry.{alias}.default"),
                Some(rc.default.to_string()),
                RegistryField::Default.spec(),
            ));
        }
    }
    entries
}

// ── Value-parsing helpers ─────────────────────────────────────────────────────

fn parse_default_view(s: &str) -> anyhow::Result<DefaultView> {
    match s {
        "flat" => Ok(DefaultView::Flat),
        "tree" => Ok(DefaultView::Tree),
        _ => Err(super::config_value(format!(
            "invalid value for options.tui.default_view: '{s}'; valid values: flat, tree"
        ))),
    }
}

fn parse_bool(s: &str, key: &str) -> anyhow::Result<bool> {
    match s {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(super::config_value(format!(
            "invalid value for {key}: '{s}'; must be true or false"
        ))),
    }
}

fn parse_u32(s: &str, key: &str) -> anyhow::Result<u32> {
    s.trim().parse::<u32>().map_err(|_| {
        super::config_value(format!(
            "invalid value for {key}: '{s}'; must be a non-negative integer"
        ))
    })
}

fn parse_tree_separators(s: &str) -> anyhow::Result<Vec<String>> {
    let seps: Vec<String> = s.split(',').map(str::to_string).collect();
    for sep in &seps {
        // Mirror validate_tree_separators exactly: require exactly one char,
        // non-control, non-whitespace, and display width == 1.
        // The width check rejects zero-width chars (U+200B, U+202E, U+FEFF,
        // Default_Ignorable) that pass the control/whitespace tests but would
        // cause every subsequent config load to fail (ConfigError 78) with no
        // CLI recovery path.
        let valid = {
            let mut chars = sep.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => !ch.is_control() && !ch.is_whitespace() && ch.width() == Some(1),
                _ => false,
            }
        };
        if !valid {
            return Err(super::config_value(format!(
                "invalid tree separator '{sep}': must be exactly one \
                 non-control, non-whitespace, single-column character"
            )));
        }
    }
    Ok(seps)
}

/// Reject values containing control characters (including newline) at exit 65.
///
/// All string values written into TOML are TOML-escaped in `write_config`, but
/// control characters produce confusing invisible input; reject them early so
/// the TOML layer never sees them.
fn reject_control_chars(value: &str, key: &str) -> anyhow::Result<()> {
    if value.chars().any(char::is_control) {
        return Err(super::config_value(format!(
            "value for {key} must not contain control characters (including newline)"
        )));
    }
    Ok(())
}

// ── Registry mutation helpers ─────────────────────────────────────────────────

/// Validate a registry alias at the CLI boundary (exit 64).
///
/// Rules mirror [`validate_registries`] in `project_config.rs`: non-empty,
/// no leading/trailing whitespace, no `/`, `"`, `\`, or control characters.
/// Called in `run_registry_add` and `parse_key` so bad aliases exit 64 rather
/// than reaching `validate_registries` → exit 78 (config error).
fn validate_alias_format(alias: &str) -> anyhow::Result<()> {
    if alias.is_empty() {
        return Err(super::config_usage("registry alias must not be empty".to_string()));
    }
    if alias != alias.trim() {
        return Err(super::config_usage(format!(
            "registry alias '{alias}' must not have leading or trailing whitespace"
        )));
    }
    if alias.contains('/') {
        return Err(super::config_usage(format!(
            "registry alias '{alias}' must not contain '/'"
        )));
    }
    if alias.contains('"') || alias.contains('\\') {
        return Err(super::config_usage(format!(
            "registry alias '{alias}' must not contain '\"' or '\\'"
        )));
    }
    if alias.chars().any(char::is_control) {
        return Err(super::config_usage(format!(
            "registry alias '{alias}' must not contain control characters"
        )));
    }
    Ok(())
}

fn find_registry<'a>(registries: &'a [RegistryConfig], alias: &str) -> Option<&'a RegistryConfig> {
    registries.iter().find(|r| r.alias.as_deref() == Some(alias))
}

fn set_registry_field(registries: &mut [RegistryConfig], alias: &str, mutate: impl FnOnce(&mut RegistryConfig)) {
    if let Some(rc) = registries.iter_mut().find(|r| r.alias.as_deref() == Some(alias)) {
        mutate(rc);
    }
}

fn clear_all_defaults(registries: &mut [RegistryConfig]) {
    for r in registries.iter_mut() {
        r.default = false;
    }
}

fn set_registry_default(registries: &mut [RegistryConfig], alias: &str, value: bool) {
    if let Some(rc) = registries.iter_mut().find(|r| r.alias.as_deref() == Some(alias)) {
        rc.default = value;
    }
}

// ── Shared write helpers ──────────────────────────────────────────────────────

/// Acquire the config-file advisory lock, or return `None` when the file does
/// not yet exist (new global config). The returned guard must remain alive for
/// the entire read-modify-write sequence.
fn acquire_config_lock(scope: &scope_resolution::ResolvedScope) -> anyhow::Result<Option<ConfigFileLock>> {
    match lockable_config_path(scope) {
        Some(path) => Ok(Some(super::grim(ConfigFileLock::try_acquire(&path))?)),
        None => Ok(None),
    }
}

/// Validate then atomically write the config for the given scope. Callers
/// must hold the lock returned by [`acquire_config_lock`] for the duration.
fn commit_config(
    scope: &scope_resolution::ResolvedScope,
    options: &ConfigOptions,
    registries: &[RegistryConfig],
) -> anyhow::Result<()> {
    super::grim(validate_registries(registries, &scope.config_path))?;
    super::grim(crate::command::add::write_config(
        &scope.config_path,
        options,
        registries,
        &scope.set,
    ))
}

// ── Sub-command handlers ──────────────────────────────────────────────────────

fn run_get(ctx: &Context, key: &str) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let parsed = parse_key(key)?;
    if matches!(parsed, ParsedKey::RegistryAlias { .. }) {
        return Err(super::config_usage(
            "cannot get registry without a field; \
             use registry.<alias>.oci or registry.<alias>.default",
        ));
    }
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let value = get_value(&parsed, &scope.options, &scope.registries)?;
    let exit_code = if value.is_some() {
        ExitCode::Success
    } else {
        ExitCode::Failure
    };
    Ok((
        ConfigReport::Get(ConfigGetReport {
            key: key.to_string(),
            value,
            scope: scope_to_origin(scope.scope),
        }),
        exit_code,
    ))
}

fn run_set(ctx: &Context, key: &str, value: &str) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let parsed = parse_key(key)?;
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = acquire_config_lock(&scope)?;

    let mut options = scope.options.clone();
    let mut registries = scope.registries.clone();
    let stored = apply_set(&parsed, value, &mut options, &mut registries)?;
    commit_config(&scope, &options, &registries)?;

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::Set,
            key: key.to_string(),
            value: Some(stored),
            scope: origin,
        }),
        ExitCode::Success,
    ))
}

fn run_unset(ctx: &Context, key: &str) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let parsed = parse_key(key)?;
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = acquire_config_lock(&scope)?;

    let mut options = scope.options.clone();
    let mut registries = scope.registries.clone();
    apply_unset(&parsed, &mut options, &mut registries)?;
    commit_config(&scope, &options, &registries)?;

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::Unset,
            key: key.to_string(),
            value: None,
            scope: origin,
        }),
        ExitCode::Success,
    ))
}

fn run_list(ctx: &Context, all: bool) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let items = collect_entries(all, &scope.options, &scope.registries);
    Ok((ConfigReport::List(ConfigListReport { items }), ExitCode::Success))
}

fn run_registry_add(
    ctx: &Context,
    alias: &str,
    oci: Option<&str>,
    index: Option<&str>,
    make_default: bool,
) -> anyhow::Result<(ConfigReport, ExitCode)> {
    // FIX 1: pre-validate alias at the CLI boundary (exit 64) so a bad alias
    // exits UsageError rather than ConfigError after write → validate_registries.
    validate_alias_format(alias)?;

    // Exactly one source locator (clap already rejects both via
    // `conflicts_with`; neither is checked here).
    let (locator, is_index) = match (oci, index) {
        (Some(u), None) => (u, false),
        (None, Some(i)) => (i, true),
        _ => {
            return Err(super::config_usage(
                "exactly one of --oci / --index must be given".to_string(),
            ));
        }
    };
    reject_control_chars(locator, if is_index { "registry.index" } else { "registry.oci" })?;
    if is_index && crate::config::registry_resolve::classify_index(locator).is_none() {
        return Err(super::config_value(format!(
            "invalid index locator '{locator}': must be an http(s):// base or a \
             git repository (git+…, ssh://, git@…, or ending in .git)"
        )));
    }

    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = acquire_config_lock(&scope)?;

    let mut registries = scope.registries.clone();

    if registries.iter().any(|r| r.alias.as_deref() == Some(alias)) {
        return Err(super::config_usage(format!(
            "registry '{alias}' already exists; use `grim config set registry.{alias}.oci <ref>` \
             to update or `grim config registry rm {alias}` to remove"
        )));
    }

    if make_default {
        clear_all_defaults(&mut registries);
    }
    registries.push(RegistryConfig {
        alias: Some(alias.to_string()),
        oci: (!is_index).then(|| locator.to_string()),
        index: is_index.then(|| locator.to_string()),
        default: make_default,
    });

    commit_config(&scope, &scope.options, &registries)?;

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::RegistryAdded,
            key: format!("registry.{alias}"),
            value: Some(locator.to_string()),
            scope: origin,
        }),
        ExitCode::Success,
    ))
}

fn run_registry_rm(ctx: &Context, alias: &str) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = acquire_config_lock(&scope)?;

    let mut registries = scope.registries.clone();
    if !registries.iter().any(|r| r.alias.as_deref() == Some(alias)) {
        return Err(super::config_usage(format!(
            "no registry '{alias}'; cannot remove a registry that does not exist"
        )));
    }
    registries.retain(|r| r.alias.as_deref() != Some(alias));

    commit_config(&scope, &scope.options, &registries)?;

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::RegistryRemoved,
            key: format!("registry.{alias}"),
            value: None,
            scope: origin,
        }),
        ExitCode::Success,
    ))
}

fn run_registry_use(ctx: &Context, alias: &str) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = acquire_config_lock(&scope)?;

    let mut registries = scope.registries.clone();
    if !registries.iter().any(|r| r.alias.as_deref() == Some(alias)) {
        return Err(super::config_usage(format!(
            "no registry '{alias}'; add it with `grim config registry add`"
        )));
    }
    clear_all_defaults(&mut registries);
    set_registry_default(&mut registries, alias, true);

    commit_config(&scope, &scope.options, &registries)?;

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::RegistryDefault,
            key: format!("registry.{alias}"),
            value: None,
            scope: origin,
        }),
        ExitCode::Success,
    ))
}

fn run_registry_show(ctx: &Context, alias: &str) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let rc = find_registry(&scope.registries, alias)
        .ok_or_else(|| super::config_usage(format!("no registry '{alias}'; add it with `grim config registry add`")))?;
    Ok((
        ConfigReport::RegistryShow(RegistryShowReport {
            alias: alias.to_string(),
            oci: rc.oci.clone(),
            index: rc.index.clone(),
            default: rc.default,
        }),
        ExitCode::Success,
    ))
}

fn run_registry_list(ctx: &Context) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let items = scope
        .registries
        .iter()
        .map(|rc| RegistryRow {
            alias: rc.alias.clone(),
            oci: rc.oci.clone(),
            index: rc.index.clone(),
            default: rc.default,
        })
        .collect();
    Ok((
        ConfigReport::RegistryList(RegistryListReport { items }),
        ExitCode::Success,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    /// Minimal parse harness so the arg tree can be exercised in isolation.
    #[derive(clap::Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: Sub,
    }

    #[derive(clap::Subcommand)]
    enum Sub {
        Config(ConfigArgs),
    }

    fn parse(args: &[&str]) -> Result<ConfigArgs, clap::Error> {
        let mut argv = vec!["grim", "config"];
        argv.extend_from_slice(args);
        Harness::try_parse_from(argv).map(|h| match h.cmd {
            Sub::Config(a) => a,
        })
    }

    #[test]
    fn get_subcommand_parses() {
        let a = parse(&["get", "options.clients"]).expect("get parses");
        assert!(matches!(a.command, ConfigCommand::Get { key } if key == "options.clients"));
    }

    #[test]
    fn set_subcommand_parses() {
        let a = parse(&["set", "options.clients", "claude,opencode"]).expect("set parses");
        assert!(matches!(
            a.command,
            ConfigCommand::Set { key, value }
            if key == "options.clients" && value == "claude,opencode"
        ));
    }

    #[test]
    fn unset_subcommand_parses() {
        parse(&["unset", "options.clients"]).expect("unset parses");
    }

    #[test]
    fn list_without_flags_parses() {
        // --show-origin was removed (FIX 4: dead surface — list reads one scope,
        // origin would always be the same constant value).
        let a = parse(&["list"]).expect("list parses");
        assert!(matches!(a.command, ConfigCommand::List { all: false }));
    }

    #[test]
    fn list_all_flag_parses() {
        let a = parse(&["list", "--all"]).expect("list --all parses");
        assert!(matches!(a.command, ConfigCommand::List { all: true }));
    }

    #[test]
    fn registry_add_parses() {
        let a = parse(&["registry", "add", "acme", "--oci", "ghcr.io/acme"]).expect("registry add parses");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Add {
                    alias,
                    oci,
                    index,
                    default,
                } => {
                    assert_eq!(alias, "acme");
                    assert_eq!(oci.as_deref(), Some("ghcr.io/acme"));
                    assert_eq!(index, None);
                    assert!(!default);
                }
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Registry"),
        }
    }

    #[test]
    fn registry_add_legacy_url_flag_is_oci_alias() {
        // Back-compat: `--url` stays a hidden alias for `--oci`.
        let a = parse(&["registry", "add", "acme", "--url", "ghcr.io/acme"]).expect("legacy --url parses");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Add { oci, .. } => assert_eq!(oci.as_deref(), Some("ghcr.io/acme")),
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Registry"),
        }
    }

    #[test]
    fn registry_add_with_default_flag_parses() {
        let a = parse(&["registry", "add", "acme", "--oci", "ghcr.io/acme", "--default"]).expect("parses");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Add { default, .. } => assert!(default),
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Registry"),
        }
    }

    #[test]
    fn registry_rm_parses() {
        parse(&["registry", "rm", "acme"]).expect("registry rm parses");
    }

    #[test]
    fn registry_use_parses() {
        parse(&["registry", "use", "acme"]).expect("registry use parses");
    }

    #[test]
    fn registry_show_parses() {
        parse(&["registry", "show", "acme"]).expect("registry show parses");
    }

    #[test]
    fn registry_list_parses() {
        parse(&["registry", "list"]).expect("registry list parses");
    }

    #[test]
    fn get_missing_key_arg_fails() {
        assert!(parse(&["get"]).is_err());
    }

    #[test]
    fn set_missing_value_arg_fails() {
        assert!(parse(&["set", "options.clients"]).is_err());
    }

    #[test]
    fn registry_add_source_arg_combinations() {
        // Neither --oci nor --index parses at the clap level (exactly-one is
        // a runtime usage error, 64, so the message can explain the choice);
        // both together conflict at the clap level; each alone parses.
        assert!(parse(&["registry", "add", "acme"]).is_ok());
        assert!(
            parse(&[
                "registry",
                "add",
                "acme",
                "--oci",
                "ghcr.io/acme",
                "--index",
                "https://idx"
            ])
            .is_err()
        );
        assert!(parse(&["registry", "add", "acme", "--oci", "ghcr.io/acme"]).is_ok());
        let a = parse(&["registry", "add", "hub", "--index", "https://index.grimoire.rs"]).expect("parses");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Add { oci, index, .. } => {
                    assert_eq!(oci, None);
                    assert_eq!(index.as_deref(), Some("https://index.grimoire.rs"));
                }
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Registry"),
        }
    }

    // ── F3: parse_key, value-parser, and registry mutation unit tests ────────

    #[test]
    fn parse_key_all_seven_valid_keys() {
        // Loop over every fixed key (closes the latent expand_levels gap:
        // the original hand-written list never exercised it).
        for k in ConfigKey::ALL {
            assert_eq!(parse_key(k.spec().key).ok(), Some(ParsedKey::Fixed(k)));
        }
        assert!(matches!(
            parse_key("registry.acme.oci"),
            Ok(ParsedKey::RegistryAliasField { alias, field: RegistryField::Oci })
            if alias == "acme"
        ));
        // Back-compat: the pre-0.7.0 field name `url` maps to Oci.
        assert!(matches!(
            parse_key("registry.acme.url"),
            Ok(ParsedKey::RegistryAliasField { alias, field: RegistryField::Oci })
            if alias == "acme"
        ));
        assert!(matches!(
            parse_key("registry.acme.default"),
            Ok(ParsedKey::RegistryAliasField { alias, field: RegistryField::Default })
            if alias == "acme"
        ));
    }

    #[test]
    fn parse_key_registry_alias_without_field() {
        assert!(matches!(
            parse_key("registry.acme"),
            Ok(ParsedKey::RegistryAlias { alias }) if alias == "acme"
        ));
    }

    #[test]
    fn parse_key_unknown_returns_err() {
        assert!(parse_key("unknown.key").is_err());
        assert!(parse_key("optins.clients").is_err());
    }

    #[test]
    fn parse_key_unknown_error_names_every_key() {
        let msg = parse_key("bogus.key").unwrap_err().to_string();
        for k in ConfigKey::ALL {
            assert!(
                msg.contains(k.spec().key),
                "error must name '{}'; got: {msg}",
                k.spec().key
            );
        }
        for f in RegistryField::ALL {
            assert!(
                msg.contains(f.spec().key),
                "error must name '{}'; got: {msg}",
                f.spec().key
            );
        }
    }

    #[test]
    fn parse_default_view_valid_and_invalid() {
        use crate::config::declaration::DefaultView;
        assert!(matches!(parse_default_view("flat"), Ok(DefaultView::Flat)));
        assert!(matches!(parse_default_view("tree"), Ok(DefaultView::Tree)));
        assert!(parse_default_view("bogus").is_err());
        assert!(parse_default_view("Flat").is_err());
    }

    #[test]
    fn parse_bool_valid_and_invalid() {
        assert!(matches!(parse_bool("true", "k"), Ok(true)));
        assert!(matches!(parse_bool("false", "k"), Ok(false)));
        assert!(parse_bool("yes", "k").is_err());
        assert!(parse_bool("1", "k").is_err());
        assert!(parse_bool("True", "k").is_err());
    }

    #[test]
    fn parse_tree_separators_valid_and_invalid() {
        let r = parse_tree_separators("/,-").unwrap();
        assert_eq!(r, vec!["/", "-"]);
        // Multi-character entry rejected.
        assert!(parse_tree_separators("::").is_err());
        // Empty entry rejected.
        assert!(parse_tree_separators("").is_err());
        // Control character rejected.
        assert!(parse_tree_separators("\n").is_err());
    }

    #[test]
    fn parse_tree_separators_zero_width_char_rejected() {
        // FIX A regression lock: U+200B ZERO WIDTH SPACE passes the single-char
        // and control/whitespace checks but has display width 0, not 1. Without
        // the width check the CLI accepts it, writes a config that fails every
        // load (ConfigError 78), and `unset` also fails — complete lockout.
        // This mirrors validate_tree_separators in project_config.rs exactly.
        assert!(
            parse_tree_separators("\u{200b}").is_err(),
            "U+200B ZWSP must be rejected"
        );
        // Bidi override and BOM also have width 0.
        assert!(
            parse_tree_separators("\u{202e}").is_err(),
            "U+202E RLO must be rejected"
        );
        assert!(
            parse_tree_separators("\u{feff}").is_err(),
            "U+FEFF BOM must be rejected"
        );
        // Existing valid single-column chars still pass.
        assert!(parse_tree_separators("/").is_ok());
        assert!(parse_tree_separators("-").is_ok());
        assert!(parse_tree_separators("/,-").is_ok());
    }

    #[test]
    fn parse_u32_valid_and_invalid() {
        assert_eq!(parse_u32("0", "k").unwrap(), 0);
        assert_eq!(parse_u32("3", "k").unwrap(), 3);
        assert_eq!(parse_u32("  2 ", "k").unwrap(), 2, "surrounding whitespace tolerated");
        assert!(parse_u32("-1", "k").is_err(), "negative rejected");
        assert!(parse_u32("x", "k").is_err(), "non-numeric rejected");
        assert!(parse_u32("", "k").is_err(), "empty rejected");
        assert!(parse_u32("1.5", "k").is_err(), "non-integer rejected");
    }

    #[test]
    fn expand_levels_set_get_unset_round_trip() {
        use crate::config::declaration::{ConfigOptions, RegistryConfig};
        let key = parse_key("options.tui.expand_levels").unwrap();
        let mut options = ConfigOptions::default();
        let mut registries: Vec<RegistryConfig> = vec![];

        // Unset by default → get returns None (so `get` exits 1, `list` omits).
        assert_eq!(get_value(&key, &options, &registries).unwrap(), None);

        // Set stores the value; get echoes it back.
        let stored = apply_set(&key, "2", &mut options, &mut registries).unwrap();
        assert_eq!(stored, "2");
        assert_eq!(options.tui.expand_levels, Some(2));
        assert_eq!(get_value(&key, &options, &registries).unwrap(), Some("2".to_string()));
        assert!(
            collect_entries(false, &options, &registries)
                .iter()
                .any(|e| e.key == "options.tui.expand_levels" && e.value.as_deref() == Some("2")),
            "list must surface a set expand_levels"
        );

        // A bad value is rejected (config_value → exit 65).
        assert!(apply_set(&key, "nope", &mut options, &mut registries).is_err());

        // Unset clears it back to None.
        apply_unset(&key, &mut options, &mut registries).unwrap();
        assert_eq!(options.tui.expand_levels, None);
        assert_eq!(get_value(&key, &options, &registries).unwrap(), None);
    }

    // ── STEP A: collect_entries --all semantics ──────────────────────────────

    #[test]
    fn collect_entries_all_emits_unset_fixed_keys_with_null_value() {
        let options = ConfigOptions::default();
        let registries: Vec<RegistryConfig> = vec![];

        let without_all = collect_entries(false, &options, &registries);
        assert_eq!(without_all.len(), 0, "flagless list on empty config must emit 0 rows");

        let with_all = collect_entries(true, &options, &registries);
        assert_eq!(
            with_all.len(),
            7,
            "--all on empty config must emit exactly the 7 fixed keys"
        );
        for e in &with_all {
            assert_eq!(
                e.value, None,
                "unset fixed key must serialize a null value; key={}",
                e.key
            );
            assert!(!e.set, "unset fixed key must have set=false; key={}", e.key);
        }
    }

    #[test]
    fn collect_entries_all_emits_registry_locator_null_rows() {
        let options = ConfigOptions::default();
        let registries = vec![RegistryConfig {
            alias: Some("acme".to_string()),
            oci: None,
            index: Some("https://index.example".to_string()),
            default: false,
        }];

        let without_all = collect_entries(false, &options, &registries);
        assert!(
            !without_all.iter().any(|e| e.key == "registry.acme.oci"),
            "flagless list must omit the unset oci locator"
        );
        assert!(
            without_all.iter().any(|e| e.key == "registry.acme.default"),
            "registry.<alias>.default is always a row, even without --all"
        );

        let with_all = collect_entries(true, &options, &registries);
        let oci_row = with_all
            .iter()
            .find(|e| e.key == "registry.acme.oci")
            .expect("--all must add the unset oci locator row");
        assert_eq!(oci_row.value, None);
        let default_row = with_all
            .iter()
            .find(|e| e.key == "registry.acme.default")
            .expect("registry.<alias>.default row must be present with --all too");
        assert_eq!(default_row.value.as_deref(), Some("false"));
    }

    #[test]
    fn registry_use_enforces_at_most_one_default() {
        use crate::config::declaration::RegistryConfig;
        let mut registries = vec![
            RegistryConfig {
                alias: Some("a".to_string()),
                oci: Some("u1".to_string()),
                index: None,
                default: true,
            },
            RegistryConfig {
                alias: Some("b".to_string()),
                oci: Some("u2".to_string()),
                index: None,
                default: false,
            },
        ];
        // Simulate `registry use b`.
        clear_all_defaults(&mut registries);
        set_registry_default(&mut registries, "b", true);
        let defaults: Vec<_> = registries.iter().filter(|r| r.default).collect();
        assert_eq!(defaults.len(), 1, "exactly one default after use");
        assert_eq!(defaults[0].alias.as_deref(), Some("b"));
    }

    // ── FIX 1: alias pre-validation at CLI boundary ──────────────────────────

    #[test]
    fn validate_alias_format_rejects_slash() {
        assert!(
            validate_alias_format("a/b").is_err(),
            "alias with '/' must be rejected (exit 64)"
        );
    }

    #[test]
    fn validate_alias_format_rejects_empty() {
        assert!(validate_alias_format("").is_err(), "empty alias must be rejected");
    }

    #[test]
    fn validate_alias_format_rejects_leading_whitespace() {
        assert!(
            validate_alias_format(" acme").is_err(),
            "alias with leading whitespace must be rejected"
        );
    }

    #[test]
    fn validate_alias_format_rejects_control_char() {
        assert!(
            validate_alias_format("a\nb").is_err(),
            "alias with control char must be rejected"
        );
    }

    #[test]
    fn validate_alias_format_allows_dots() {
        // Dots are allowed in aliases (FIX 2 addressability).
        assert!(validate_alias_format("a.b").is_ok(), "alias with dot must be allowed");
        assert!(
            validate_alias_format("a.b.c").is_ok(),
            "alias with multiple dots must be allowed"
        );
    }

    // ── FIX 2: parse_key uses rightmost dot ──────────────────────────────────

    #[test]
    fn parse_key_dotted_alias_oci() {
        // `registry.a.b.oci` → alias=`a.b`, field=Oci
        let result = parse_key("registry.a.b.oci");
        assert!(result.is_ok(), "parse_key registry.a.b.oci must succeed");
        match result.unwrap() {
            ParsedKey::RegistryAliasField {
                alias,
                field: RegistryField::Oci,
            } => assert_eq!(alias, "a.b"),
            _ => panic!("expected RegistryAliasField(a.b, Oci)"),
        }
    }

    #[test]
    fn parse_key_dotted_alias_default() {
        // `registry.a.b.default` → alias=`a.b`, field=Default
        let result = parse_key("registry.a.b.default");
        assert!(result.is_ok(), "parse_key registry.a.b.default must succeed");
        match result.unwrap() {
            ParsedKey::RegistryAliasField {
                alias,
                field: RegistryField::Default,
            } => assert_eq!(alias, "a.b"),
            _ => panic!("expected RegistryAliasField(a.b, Default)"),
        }
    }

    #[test]
    fn parse_key_slash_in_alias_exits_64() {
        // FIX 1: `registry.a/b.url` → alias `a/b` is invalid → usage error.
        // The error message must reference the bad character, confirming the
        // alias was caught at the CLI boundary (not at validate_registries).
        let result = parse_key("registry.a/b.url");
        assert!(result.is_err(), "slash in alias must be rejected");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("'/'") || msg.contains('/'),
            "error must name the offending character; got: {msg}"
        );
    }

    // ── FIX 3: empty/whitespace segment in options.clients ───────────────────

    #[test]
    fn apply_set_clients_rejects_whitespace_segment() {
        use crate::config::declaration::{ConfigOptions, TuiOptions};
        let mut options = ConfigOptions {
            clients: vec![],
            default_registry: None,
            show_deprecated: false,
            tui: TuiOptions::default(),
        };
        let mut registries = vec![];
        let result = apply_set(
            &ParsedKey::Fixed(ConfigKey::Clients),
            "claude, ,opencode",
            &mut options,
            &mut registries,
        );
        // FIX 3: empty segment must be rejected with an error (exit 65).
        assert!(result.is_err(), "whitespace segment in clients must be rejected");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("empty") || msg.contains("segment"),
            "error must describe the empty segment; got: {msg}"
        );
    }

    #[test]
    fn set_registry_alias_default_true_at_most_one() {
        use crate::config::declaration::RegistryConfig;
        let mut registries = vec![
            RegistryConfig {
                alias: Some("x".to_string()),
                oci: Some("u1".to_string()),
                index: None,
                default: true,
            },
            RegistryConfig {
                alias: Some("y".to_string()),
                oci: Some("u2".to_string()),
                index: None,
                default: false,
            },
        ];
        // Simulate `set registry.y.default true`.
        clear_all_defaults(&mut registries);
        set_registry_default(&mut registries, "y", true);
        assert_eq!(registries.iter().filter(|r| r.default).count(), 1);
    }
}
