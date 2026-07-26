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

use crate::api::config_report::{
    ConfigEntry, ConfigGetReport, ConfigListReport, ConfigReport, ConfigWriteReport, Origin, RegistryFieldEntry,
    RegistryFieldsReport, RegistryListReport, RegistryRow, RegistryShowReport, WriteAction,
};
use crate::cli::exit_code::ExitCode;
use crate::config::declaration::{ConfigOptions, DefaultView, RegistryConfig};
use crate::config::project_config::validate_registries;
use crate::config::scope::ConfigScope;
use crate::context::Context;
use crate::install::client_target::ClientTarget;
use crate::lock::file_lock::ConfigFileLock;

use super::config_keys::{ConfigKey, KeySpec, RegistryField, VENDOR_FIELD_NAME, VENDOR_SHARED_SKILLS};
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
        /// Validate and report without writing the config file.
        #[arg(long)]
        dry_run: bool,
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
    /// List the addressable per-registry fields and their metadata.
    Fields,
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
        ConfigCommand::Set { key, value, dry_run } => run_set(ctx, key, value, *dry_run),
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
            // Static metadata — no ctx, no scope resolve, no lock; must
            // work outside any project (unlike every other registry verb).
            RegistryCommand::Fields => run_registry_fields(),
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
    /// `options.vendors.<name>.shared_skills` — the dynamic per-vendor key.
    VendorField { vendor: String },
}

fn parse_key(key: &str) -> anyhow::Result<ParsedKey> {
    if let Some(k) = ConfigKey::parse(key) {
        return Ok(ParsedKey::Fixed(k));
    }
    if let Some(rest) = key.strip_prefix("options.vendors.") {
        // Dynamic key: one instance per client name, so it is parsed from the
        // remainder after the fixed prefix rather than matched against
        // `ConfigKey::ALL`. Split at the RIGHTMOST dot for the same reason the
        // registry branch does — the field name is the last segment.
        // Both messages quote a user-supplied segment, so both render it
        // escaped — a raw ESC echoed to stderr is a control-sequence-injection
        // vector, the same one `ClientsInvalid::ControlChar` guards against.
        let Some((vendor, field)) = rest.rsplit_once('.') else {
            return Err(super::config_usage(format!(
                "no vendor field specified for '{}'; use options.vendors.<name>.{VENDOR_FIELD_NAME}",
                rest.escape_debug()
            )));
        };
        if field != VENDOR_FIELD_NAME {
            return Err(super::config_usage(format!(
                "unknown vendor field '{}'; valid fields: {VENDOR_FIELD_NAME}",
                field.escape_debug()
            )));
        }
        // The client name is part of the KEY, so an unknown one is an unknown
        // key (exit 64) — not a bad value. Shares its accepted set with
        // load-time validation via `check_vendor_name`.
        crate::config::project_config::check_vendor_name(vendor).map_err(vendor_key_error)?;
        return Ok(ParsedKey::VendorField {
            vendor: vendor.to_string(),
        });
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
        ConfigKey::TuiDefaultView => options.tui.default_view.map(|v| v.as_str().to_string()),
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

/// The effective value of a per-vendor key, or `None` when unset.
///
/// `false` is the built-in default and indistinguishable from an absent
/// table entry, so it collapses to unset across `get` / `list` / `unset` —
/// the same rule `show_deprecated` and `tui.group_by_type` follow.
fn vendor_value(vendor: &str, options: &ConfigOptions) -> Option<String> {
    options
        .vendors
        .get(vendor)
        .filter(|v| v.shared_skills)
        .map(|_| "true".to_string())
}

fn get_value(
    parsed: &ParsedKey,
    options: &ConfigOptions,
    registries: &[RegistryConfig],
) -> anyhow::Result<Option<String>> {
    Ok(match parsed {
        ParsedKey::Fixed(k) => fixed_value(*k, options),
        ParsedKey::VendorField { vendor } => vendor_value(vendor, options),
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
                    // Reject control characters before any error message could echo one
                    // into the terminal (the unknown-value message quotes the segment).
                    for c in &clients {
                        reject_control_chars(c, "options.clients")?;
                    }
                    // Shared closed-set + uniqueness + non-blank check: the single source
                    // of truth with load-time `validate_clients`, so `config set` and a
                    // hand-edited config accept exactly the same set. Set-time keeps its
                    // exit-65 (DataError) mapping and its own message wording.
                    crate::config::project_config::check_clients(&clients).map_err(clients_set_error)?;
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
        ParsedKey::VendorField { vendor } => {
            let enabled = parse_bool(value_str, &format!("options.vendors.{vendor}.{VENDOR_FIELD_NAME}"))?;
            if enabled {
                options.vendors.entry(vendor.clone()).or_default().shared_skills = true;
            } else {
                // `false` is the default, so it is stored as absence rather
                // than as a table that means nothing — same collapse as
                // `vendor_value` and `apply_unset`. `VendorOptions` carries a
                // single field today; a second one would make this a
                // field-level clear instead of an entry removal.
                options.vendors.remove(vendor);
            }
            Ok(value_str.to_string())
        }
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
                        // Set-time twin of the load-time `index '{locator}'`
                        // message: `reject_control_chars` above stops ESC, but
                        // U+202E is not `char::is_control` and reaches here.
                        return Err(super::config_value(format!(
                            "invalid index locator '{}': must be an http(s):// base or a \
                             git repository (git+…, ssh://, git@…, or ending in .git)",
                            value_str.escape_debug()
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
        ParsedKey::VendorField { vendor } => {
            // Single-field `VendorOptions`: clearing the field and removing
            // the entry are the same thing, and an empty table would
            // round-trip as an unset key anyway.
            options.vendors.remove(vendor);
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
    ConfigEntry::new(
        key,
        value,
        spec.value_type,
        spec.title,
        spec.description,
        spec.constraints,
    )
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
    // Per-vendor rows exist only for a client the config actually names —
    // the same rule the registry rows follow. `--all` widens a named entry
    // to its unset (default-valued) row; it never enumerates the whole
    // client set, which would change the row set of an existing command.
    for name in options.vendors.keys() {
        let value = vendor_value(name, options);
        if value.is_some() || all {
            entries.push(entry(
                format!("options.vendors.{name}.{VENDOR_FIELD_NAME}"),
                value,
                &VENDOR_SHARED_SKILLS,
            ));
        }
    }
    entries
}

// ── Value-parsing helpers ─────────────────────────────────────────────────────

/// Render a shared [`ClientsInvalid`] verdict as a set-time data error
/// (exit 65). Load-time validation renders its own message and uses the
/// config-error class (exit 78); the accepted set is shared via
/// [`crate::config::project_config::check_clients`] so the two cannot drift.
fn clients_set_error(reason: crate::config::project_config::ClientsInvalid) -> anyhow::Error {
    use crate::config::project_config::ClientsInvalid;
    match reason {
        ClientsInvalid::Blank => super::config_value(
            "options.clients: empty or whitespace-only segment; each client name must be non-empty".to_string(),
        ),
        // Unreachable at set time — `reject_control_chars` runs first in the
        // Clients arm of `apply_set` and rejects the value before it reaches
        // `check_clients`. Kept for exhaustiveness and defense in depth;
        // renders the value escaped so no raw control byte reaches stderr.
        ClientsInvalid::ControlChar(c) => super::config_value(format!(
            "options.clients: client name contains control characters: '{}'",
            c.escape_debug()
        )),
        // Escaped like the arm above, and for a reason `reject_control_chars`
        // does not cover: `char::is_control` is false for the bidi and
        // zero-width format characters (U+202E, U+200B), so those pass the
        // pre-check and reach this arm intact.
        ClientsInvalid::Unknown(c) => super::config_value(format!(
            "invalid value for options.clients: '{}'; valid values: {}",
            c.escape_debug(),
            ClientTarget::VALUE_NAMES.join(", ")
        )),
        // `Duplicate` can only carry a name from the closed set today —
        // `check_clients` returns `Unknown` first — so the escape is a no-op
        // here. Kept so reordering those two checks cannot reopen the hole.
        ClientsInvalid::Duplicate(c) => super::config_value(format!(
            "options.clients: duplicate client '{}'; each client may appear once",
            c.escape_debug()
        )),
    }
}

/// Render a shared [`crate::config::project_config::ClientsInvalid`] verdict
/// as a key-parse usage error (exit 64).
///
/// In `options.vendors.<name>.…` the client name is part of the **key**, so
/// an unknown one is an unknown key — not a bad value (65) and not a config
/// error (78). Load-time validation renders its own message on the
/// config-error class; the accepted set is shared via
/// [`crate::config::project_config::check_vendor_name`] so the two cannot
/// drift.
fn vendor_key_error(reason: crate::config::project_config::ClientsInvalid) -> anyhow::Error {
    use crate::config::project_config::ClientsInvalid;
    match reason {
        ClientsInvalid::Blank => super::config_usage(format!(
            "empty client name in config key; use options.vendors.<name>.{VENDOR_FIELD_NAME}"
        )),
        // Renders the name escaped so no raw control byte reaches stderr.
        ClientsInvalid::ControlChar(c) => super::config_usage(format!(
            "client name in config key contains control characters: '{}'",
            c.escape_debug()
        )),
        // `Duplicate` is unreachable for a single-name check; folded in for
        // exhaustiveness rather than panicking on an impossible verdict.
        // Escaped like the arm above: `char::is_control` does not cover the
        // bidi and zero-width format characters, which a key segment can
        // still carry into stderr.
        ClientsInvalid::Unknown(c) | ClientsInvalid::Duplicate(c) => super::config_usage(format!(
            "unknown config key: no client named '{}'; valid clients: {}",
            c.escape_debug(),
            ClientTarget::VALUE_NAMES.join(", ")
        )),
    }
}

fn parse_default_view(s: &str) -> anyhow::Result<DefaultView> {
    DefaultView::ALL.into_iter().find(|v| v.as_str() == s).ok_or_else(|| {
        super::config_value(format!(
            "invalid value for options.tui.default_view: '{s}'; valid values: {}",
            DefaultView::VALUE_NAMES.join(", ")
        ))
    })
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
        // Shares its accepted-character predicate with validate_tree_separators
        // via is_valid_tree_separator — the width check rejects zero-width chars
        // (U+200B, U+202E, U+FEFF, Default_Ignorable) that pass the
        // control/whitespace tests but would cause every subsequent config load
        // to fail (ConfigError 78) with no CLI recovery path.
        if !crate::config::project_config::is_valid_tree_separator(sep) {
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
    // Escaped for the same reason as `validate_registries`, which this
    // function mirrors: the control-character check is the LAST arm, so three
    // messages quote the alias before anything has rejected a control byte in
    // it — and `char::is_control` never matches U+202E at all, so the last arm
    // would still echo a bidi override raw.
    let shown = alias.escape_debug();
    if alias.is_empty() {
        return Err(super::config_usage("registry alias must not be empty".to_string()));
    }
    if alias != alias.trim() {
        return Err(super::config_usage(format!(
            "registry alias '{shown}' must not have leading or trailing whitespace"
        )));
    }
    if alias.contains('/') {
        return Err(super::config_usage(format!(
            "registry alias '{shown}' must not contain '/'"
        )));
    }
    if alias.contains('"') || alias.contains('\\') {
        return Err(super::config_usage(format!(
            "registry alias '{shown}' must not contain '\"' or '\\'"
        )));
    }
    if alias.chars().any(char::is_control) {
        return Err(super::config_usage(format!(
            "registry alias '{shown}' must not contain control characters"
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

/// `--dry-run` validates and reports exactly what a real `set` would do —
/// same `parse_key` / scope resolution / `apply_set` / registry validation —
/// but skips the advisory lock and the write, so error parity with the real
/// path is by construction (same validators, same 64/65/79 envelopes).
fn run_set(ctx: &Context, key: &str, value: &str, dry_run: bool) -> anyhow::Result<(ConfigReport, ExitCode)> {
    let parsed = parse_key(key)?;
    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = if dry_run { None } else { acquire_config_lock(&scope)? };

    let mut options = scope.options.clone();
    let mut registries = scope.registries.clone();
    let stored = apply_set(&parsed, value, &mut options, &mut registries)?;

    if dry_run {
        // The validate half of `commit_config`, without the write.
        super::grim(validate_registries(&registries, &scope.config_path))?;
    } else {
        commit_config(&scope, &options, &registries)?;
    }

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::Set,
            key: key.to_string(),
            value: Some(stored),
            scope: origin,
            dry_run,
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
            dry_run: false,
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
        // Escaped like its `apply_set` twin above: the control-char guard on
        // the line before does not match the bidi and zero-width format
        // characters, which reach this message intact.
        return Err(super::config_value(format!(
            "invalid index locator '{}': must be an http(s):// base or a \
             git repository (git+…, ssh://, git@…, or ending in .git)",
            locator.escape_debug()
        )));
    }

    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    let _guard = acquire_config_lock(&scope)?;

    let mut registries = scope.registries.clone();

    if registries.iter().any(|r| r.alias.as_deref() == Some(alias)) {
        // Set-time twin of the load-time `duplicate alias '{shown}'` message.
        // `validate_alias_format` above does not stop U+202E — nothing rejects
        // a format character in an alias — so it reaches this message intact.
        let shown = alias.escape_debug();
        return Err(super::config_usage(format!(
            "registry '{shown}' already exists; use `grim config set registry.{shown}.oci <ref>` \
             to update or `grim config registry rm {shown}` to remove"
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
            dry_run: false,
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
            dry_run: false,
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
            dry_run: false,
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

/// `grim config registry fields` — static metadata for the 3 addressable
/// per-registry fields (`oci`, `index`, `default`). Unlike every other
/// `config` subcommand this takes no [`Context`], resolves no scope, and
/// acquires no lock: the field set and its type/title/description are
/// fixed at compile time (see [`RegistryField::spec`]), so the command
/// works identically inside or outside a project.
fn run_registry_fields() -> anyhow::Result<(ConfigReport, ExitCode)> {
    let items = RegistryField::ALL
        .into_iter()
        .map(|f| {
            let spec = f.spec();
            RegistryFieldEntry {
                key: f.field_name(),
                value_type: spec.value_type,
                title: spec.title,
                description: spec.description,
            }
        })
        .collect();
    Ok((
        ConfigReport::RegistryFields(RegistryFieldsReport { items }),
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
            ConfigCommand::Set { key, value, dry_run }
            if key == "options.clients" && value == "claude,opencode" && !dry_run
        ));
    }

    #[test]
    fn set_dry_run_flag_parses() {
        let a = parse(&["set", "options.clients", "claude", "--dry-run"]).expect("set --dry-run parses");
        assert!(matches!(
            a.command,
            ConfigCommand::Set { dry_run, .. } if dry_run
        ));
    }

    #[test]
    fn unset_rejects_dry_run_flag() {
        // `--dry-run` is `set`-only; `unset` has no such surface.
        assert!(parse(&["unset", "options.clients", "--dry-run"]).is_err());
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
    fn registry_fields_parses() {
        let a = parse(&["registry", "fields"]).expect("registry fields parses");
        match a.command {
            ConfigCommand::Registry(r) => assert!(matches!(r.command, RegistryCommand::Fields)),
            _ => panic!("expected Registry"),
        }
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
        // Pin the error text: it must enumerate the valid views so a variant
        // rename or a lost VALUE_NAMES entry is caught here.
        let msg = parse_default_view("bogus").unwrap_err().to_string();
        assert!(
            msg.contains("valid values: flat, tree"),
            "error must enumerate the valid views; got: {msg}"
        );
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
    fn collect_entries_emits_explicit_zero_expand_levels() {
        // u32 has no false-is-unset collapse: `expand_levels = Some(0)` is an
        // explicitly-set value, so `list` (even without --all) emits a SET row
        // with value "0" — unlike the bool / empty-list keys that collapse a
        // default-valued setting to unset.
        let mut options = ConfigOptions::default();
        options.tui.expand_levels = Some(0);
        let registries: Vec<RegistryConfig> = vec![];

        let row = collect_entries(false, &options, &registries)
            .into_iter()
            .find(|e| e.key == "options.tui.expand_levels")
            .expect("expand_levels = Some(0) must emit a row without --all");
        assert_eq!(row.value.as_deref(), Some("0"), "explicit zero is the emitted value");
        assert!(row.set, "explicit zero is a SET row, not unset");
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
    fn validate_alias_format_never_echoes_a_raw_hostile_byte() {
        // Mirror of `registries_messages_never_echo_raw_authored_bytes`: the
        // control-char arm is LAST, so the whitespace and '/' arms quote the
        // alias before anything rejects a control byte — and U+202E is not
        // `char::is_control`, so it reaches every arm including the last.
        // The whole message is pinned per case, not a substring: `shown` is one
        // binding shared by every arm, so a substring check would stay green if
        // a case fell through to a different arm than its comment claims.
        for (alias, raw, expected) in [
            (
                " \u{1b}[2Jx",
                '\u{1b}',
                r"registry alias ' \u{1b}[2Jx' must not have leading or trailing whitespace",
            ),
            (
                "/\u{1b}[2Jx",
                '\u{1b}',
                r"registry alias '/\u{1b}[2Jx' must not contain '/'",
            ),
            (
                "a\u{1b}[2Jb",
                '\u{1b}',
                r"registry alias 'a\u{1b}[2Jb' must not contain control characters",
            ),
            // U+202E alone is accepted by every arm (it is neither whitespace
            // nor a control character), so it only reaches a message paired
            // with something that does fail — here, leading whitespace.
            (
                " a\u{202e}b",
                '\u{202e}',
                r"registry alias ' a\u{202e}b' must not have leading or trailing whitespace",
            ),
        ] {
            let msg = validate_alias_format(alias)
                .expect_err("a hostile alias must be rejected")
                .to_string();
            assert!(
                !msg.contains(raw),
                "message for {alias:?} must not embed the raw {raw:?} byte: {msg:?}"
            );
            assert_eq!(msg, expected, "wrong arm or wrong rendering for {alias:?}");
        }

        // An alias with nothing to escape keeps its exact shipped message.
        let msg = validate_alias_format("a/b")
            .expect_err("a slash alias must be rejected")
            .to_string();
        assert_eq!(msg, "registry alias 'a/b' must not contain '/'");
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
            vendors: Default::default(),
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

    // ── options.clients: closed-set validation (StringSet) ───────────────────

    fn fresh_options() -> ConfigOptions {
        use crate::config::declaration::TuiOptions;
        ConfigOptions {
            clients: vec![],
            default_registry: None,
            show_deprecated: false,
            tui: TuiOptions::default(),
            vendors: Default::default(),
        }
    }

    #[test]
    fn clients_set_unknown_value_never_echoes_a_raw_format_byte() {
        // Set-time twin of the load-time escape. `reject_control_chars` runs
        // first and catches ESC, but U+202E is not `char::is_control`, so a
        // format character sails past it into the unknown-value arm — which
        // quotes the segment straight back to stderr.
        let key = ParsedKey::Fixed(ConfigKey::Clients);
        let mut options = fresh_options();
        let mut registries = vec![];
        let msg = apply_set(&key, "cursor\u{202e}evil", &mut options, &mut registries)
            .expect_err("an unknown client must be rejected")
            .to_string();
        assert!(
            !msg.contains('\u{202e}'),
            "message must not embed the raw override byte: {msg:?}"
        );
        assert!(
            msg.contains(r"cursor\u{202e}evil"),
            "the offending name must survive in escaped form: {msg:?}"
        );

        // A plain unknown name keeps its exact shipped message text.
        let msg = apply_set(&key, "vscode", &mut options, &mut registries)
            .expect_err("an unknown client must be rejected")
            .to_string();
        assert_eq!(
            msg,
            format!(
                "invalid value for options.clients: 'vscode'; valid values: {}",
                ClientTarget::VALUE_NAMES.join(", ")
            )
        );
    }

    #[test]
    fn apply_set_clients_rejects_unknown_client() {
        let mut options = fresh_options();
        let mut registries = vec![];
        let result = apply_set(
            &ParsedKey::Fixed(ConfigKey::Clients),
            "claude,vscode",
            &mut options,
            &mut registries,
        );
        let msg = result.expect_err("unknown client must be rejected").to_string();
        assert!(
            msg.contains("invalid value for options.clients: 'vscode'"),
            "error must name the offending value (parse_default_view template); got: {msg}"
        );
        assert!(
            msg.contains("valid values: claude, opencode, copilot"),
            "error must list the valid values; got: {msg}"
        );
    }

    #[test]
    fn apply_set_clients_rejects_duplicate_client() {
        let mut options = fresh_options();
        let mut registries = vec![];
        let result = apply_set(
            &ParsedKey::Fixed(ConfigKey::Clients),
            "claude,opencode,claude",
            &mut options,
            &mut registries,
        );
        let msg = result.expect_err("duplicate client must be rejected").to_string();
        assert!(
            msg.contains("duplicate client 'claude'"),
            "error must name the duplicated client; got: {msg}"
        );
        assert!(
            msg.contains("each client may appear once"),
            "error must carry the remediation hint; got: {msg}"
        );
    }

    #[test]
    fn apply_set_clients_valid_multi_round_trips_in_input_order() {
        let mut options = fresh_options();
        let mut registries = vec![];
        let stored = apply_set(
            &ParsedKey::Fixed(ConfigKey::Clients),
            "opencode,claude",
            &mut options,
            &mut registries,
        )
        .expect("valid multi-value set must succeed");
        // Input order preserved on store — not ClientTarget::ALL canonical order.
        assert_eq!(stored, "opencode,claude");
        assert_eq!(options.clients, vec!["opencode".to_string(), "claude".to_string()]);
        assert_eq!(
            get_value(&ParsedKey::Fixed(ConfigKey::Clients), &options, &registries).unwrap(),
            Some("opencode,claude".to_string())
        );
    }

    #[test]
    fn apply_set_clients_empty_value_still_clears() {
        let mut options = fresh_options();
        options.clients = vec!["claude".to_string()];
        let mut registries = vec![];
        let stored = apply_set(&ParsedKey::Fixed(ConfigKey::Clients), "", &mut options, &mut registries)
            .expect("empty value must clear, not error");
        assert_eq!(stored, "");
        assert!(options.clients.is_empty());
    }

    // ── options.vendors.<name>.shared_skills: the dynamic per-vendor key ─────

    #[test]
    fn parse_key_vendor_field_for_every_known_client() {
        // The key is dynamic: every name the closed client set accepts is
        // addressable, with no per-client registration.
        for name in ClientTarget::VALUE_NAMES {
            let key = format!("options.vendors.{name}.shared_skills");
            assert!(
                matches!(parse_key(&key), Ok(ParsedKey::VendorField { vendor }) if vendor == *name),
                "every known client must parse as a vendor key: {key}"
            );
        }
    }

    #[test]
    fn parse_key_unknown_vendor_name_is_a_usage_error() {
        // The name is part of the KEY, so this is 64 (unknown key), not 65.
        let msg = parse_key("options.vendors.vscode.shared_skills")
            .expect_err("unknown vendor must be rejected")
            .to_string();
        assert!(
            msg.contains("vscode"),
            "error must name the offending client; got: {msg}"
        );
        assert!(
            msg.contains("valid clients: claude, opencode, copilot"),
            "error must list the valid clients; got: {msg}"
        );
    }

    #[test]
    fn parse_key_unknown_vendor_field_is_a_usage_error() {
        let msg = parse_key("options.vendors.cursor.bogus")
            .expect_err("unknown vendor field must be rejected")
            .to_string();
        assert!(
            msg.contains("unknown vendor field 'bogus'") && msg.contains("shared_skills"),
            "error must name the offending and the valid field; got: {msg}"
        );
    }

    #[test]
    fn parse_key_vendor_without_field_is_a_usage_error() {
        let msg = parse_key("options.vendors.cursor")
            .expect_err("a vendor key without a field must be rejected")
            .to_string();
        assert!(
            msg.contains("no vendor field specified"),
            "error must explain the missing field; got: {msg}"
        );
        // The bare prefix falls through to the generic unknown-key message.
        assert!(parse_key("options.vendors").is_err());

        // An empty name reaches the shared checker's `Blank` verdict.
        let msg = parse_key("options.vendors..shared_skills")
            .expect_err("an empty client name must be rejected")
            .to_string();
        assert!(
            msg.contains("empty client name"),
            "error must name the empty segment; got: {msg}"
        );
    }

    #[test]
    fn parse_key_vendor_messages_never_echo_a_raw_control_byte() {
        // Every arm of the vendor key parse quotes a user-supplied segment
        // back to stderr, so every arm must escape it — a raw ESC is a
        // control-sequence-injection vector.
        for key in [
            "options.vendors.\u{1b}[2Jcursor.shared_skills",
            "options.vendors.cursor.\u{1b}[2Jbogus",
            "options.vendors.\u{1b}[2Jcursor",
        ] {
            let msg = parse_key(key)
                .expect_err("a control character in the key must be rejected")
                .to_string();
            assert!(
                !msg.contains('\u{1b}'),
                "message for {key:?} must not embed the raw ESC byte: {msg:?}"
            );
        }

        // U+202E is not `char::is_control`, so it lands in the unknown-client
        // arm instead — which quotes the name and must escape it as well.
        let msg = parse_key("options.vendors.cursor\u{202e}evil.shared_skills")
            .expect_err("an unknown client name must be rejected")
            .to_string();
        assert!(
            !msg.contains('\u{202e}'),
            "message must not embed the raw override byte: {msg:?}"
        );
    }

    #[test]
    fn parse_key_unknown_error_names_the_vendor_pattern_key() {
        let msg = parse_key("bogus.key").unwrap_err().to_string();
        assert!(
            msg.contains("options.vendors.<name>.shared_skills"),
            "the unknown-key message must advertise the vendor pattern; got: {msg}"
        );
    }

    #[test]
    fn vendor_set_get_unset_round_trip() {
        let key = parse_key("options.vendors.cursor.shared_skills").unwrap();
        let mut options = fresh_options();
        let mut registries: Vec<RegistryConfig> = vec![];

        // Unset by default → get returns None (so `get` exits 1, `list` omits).
        assert_eq!(get_value(&key, &options, &registries).unwrap(), None);

        let stored = apply_set(&key, "true", &mut options, &mut registries).unwrap();
        assert_eq!(stored, "true");
        assert!(options.vendors["cursor"].shared_skills);
        assert_eq!(
            get_value(&key, &options, &registries).unwrap(),
            Some("true".to_string())
        );
        assert!(
            collect_entries(false, &options, &registries)
                .iter()
                .any(|e| e.key == "options.vendors.cursor.shared_skills" && e.value.as_deref() == Some("true")),
            "list must surface a set vendor key"
        );

        // A bad value is rejected (config_value → exit 65), and the message
        // names the fully-qualified key rather than the pattern.
        let msg = apply_set(&key, "yes", &mut options, &mut registries)
            .expect_err("a non-boolean must be rejected")
            .to_string();
        assert!(
            msg.contains("options.vendors.cursor.shared_skills") && msg.contains("true or false"),
            "error must name the key and the accepted values; got: {msg}"
        );

        apply_unset(&key, &mut options, &mut registries).unwrap();
        assert!(options.vendors.is_empty(), "unset must drop the entry");
        assert_eq!(get_value(&key, &options, &registries).unwrap(), None);
    }

    #[test]
    fn vendor_set_false_collapses_to_unset() {
        // `false` is the built-in default and indistinguishable from an
        // absent entry — the same collapse `show_deprecated` performs.
        let key = parse_key("options.vendors.cursor.shared_skills").unwrap();
        let mut options = fresh_options();
        let mut registries: Vec<RegistryConfig> = vec![];

        apply_set(&key, "true", &mut options, &mut registries).unwrap();
        apply_set(&key, "false", &mut options, &mut registries).unwrap();

        assert!(
            options.vendors.is_empty(),
            "a default-valued vendor entry must not be stored"
        );
        assert_eq!(get_value(&key, &options, &registries).unwrap(), None);
    }

    #[test]
    fn vendor_set_preserves_other_clients_entries() {
        let mut options = fresh_options();
        let mut registries: Vec<RegistryConfig> = vec![];
        apply_set(
            &parse_key("options.vendors.cursor.shared_skills").unwrap(),
            "true",
            &mut options,
            &mut registries,
        )
        .unwrap();
        apply_set(
            &parse_key("options.vendors.zed.shared_skills").unwrap(),
            "true",
            &mut options,
            &mut registries,
        )
        .unwrap();
        apply_unset(
            &parse_key("options.vendors.cursor.shared_skills").unwrap(),
            &mut options,
            &mut registries,
        )
        .unwrap();

        assert_eq!(
            options.vendors.keys().collect::<Vec<_>>(),
            vec!["zed"],
            "a per-client edit must not disturb another client's entry"
        );
    }

    #[test]
    fn collect_entries_vendor_rows_only_for_named_clients() {
        // `--all` must not enumerate the whole client set: a dynamic key's
        // rows follow the config's own entries, exactly like registry rows.
        let options = fresh_options();
        let registries: Vec<RegistryConfig> = vec![];
        assert!(
            !collect_entries(true, &options, &registries)
                .iter()
                .any(|e| e.key.starts_with("options.vendors.")),
            "--all on a vendorless config must emit no vendor rows"
        );

        // A declared-but-default entry is an unset row: hidden without
        // `--all`, present with it.
        let mut options = fresh_options();
        options.vendors.insert(
            "cursor".to_string(),
            crate::config::declaration::VendorOptions::default(),
        );
        assert!(
            !collect_entries(false, &options, &registries)
                .iter()
                .any(|e| e.key.starts_with("options.vendors.")),
            "a default-valued vendor row is omitted without --all"
        );
        let row = collect_entries(true, &options, &registries)
            .into_iter()
            .find(|e| e.key == "options.vendors.cursor.shared_skills")
            .expect("--all must surface the declared entry");
        assert_eq!(row.value, None, "a default-valued row is unset");
        assert!(!row.set);
        assert_eq!(row.title, VENDOR_SHARED_SKILLS.title);
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
