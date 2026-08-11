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
    ConfigEntry, ConfigGetReport, ConfigListReport, ConfigReport, ConfigWriteReport, Origin, RegistryFieldChange,
    RegistryFieldChangeAction, RegistryFieldEntry, RegistryFieldValue, RegistryFieldsReport, RegistryListReport,
    RegistryRow, RegistryShowReport, WriteAction,
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
        /// Browse-filter glob narrowing what this registry shows in `grim
        /// search`, the TUI, and `grim_search`. Affects browsing only — a
        /// direct reference to a hidden package still resolves and installs.
        /// Repeatable, never comma-separated — a comma is glob alternation
        /// syntax, so `--include '{platform,tools}/**'` is one pattern.
        /// Tested against TWO strings: the row's repository path
        /// ('acme/tools') and its fully-qualified reference
        /// ('ghcr.io/acme/tools'). A hit on either counts, so a bare pattern
        /// matches on every host and a host-qualified pattern matches on that
        /// host only. Under `--oci ghcr.io/acme` and under `--index`, alike,
        /// this entry's own locator never changes what a pattern means — it is
        /// part of neither candidate. Every pattern anchors at the START of
        /// whichever candidate it is tested against — a wildcard-free one
        /// expands downward only ('hex' means 'hex{,/**}'), so to match a name
        /// wherever it sits write '**/hex'.
        #[arg(long)]
        include: Vec<String>,
        /// Browse-filter glob hiding matching repositories from this
        /// registry. Affects browsing only — a direct reference to a hidden
        /// package still resolves and installs. Repeatable, never
        /// comma-separated, and tested against the same two candidates,
        /// anchored the same way (see `--include`); wins over `--include`
        /// where both match.
        #[arg(long)]
        exclude: Vec<String>,
        /// Mark this registry as the default (clears any prior default).
        #[arg(long)]
        default: bool,
    },
    /// Edit an existing registry entry in place, leaving unnamed fields alone.
    Set {
        /// Alias of the registry to edit (must already exist).
        alias: String,
        /// Replace the OCI registry ref, clearing any `index` on the entry.
        #[arg(long)]
        oci: Option<String>,
        /// Replace the package-index locator, clearing any `oci` on the entry.
        #[arg(long, conflicts_with = "oci")]
        index: Option<String>,
        /// Replace the whole include list with these patterns. Repeatable and
        /// never comma-separated, exactly as on `add`. Given zero times the
        /// list is left untouched — empty it with `--clear-include` or with
        /// `grim config unset registry.<alias>.include`.
        #[arg(long)]
        include: Vec<String>,
        /// Replace the whole exclude list with these patterns. Same repeat and
        /// clearing rules as `--include`.
        #[arg(long)]
        exclude: Vec<String>,
        /// Clear the include list, leaving the entry unfiltered on that
        /// side. Conflicts with `--include`.
        #[arg(long, conflicts_with = "include")]
        clear_include: bool,
        /// Clear the exclude list, leaving the entry unfiltered on that
        /// side. Conflicts with `--exclude`.
        #[arg(long, conflicts_with = "exclude")]
        clear_exclude: bool,
        /// Make this registry the default (clears any prior default). Absent
        /// leaves the current default flag as it is — this flag cannot unset
        /// one, because the default has to live somewhere; move it by naming
        /// another entry.
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
                include,
                exclude,
                default,
            } => run_registry_add(ctx, alias, oci.as_deref(), index.as_deref(), *default, include, exclude),
            RegistryCommand::Set {
                alias,
                oci,
                index,
                include,
                exclude,
                clear_include,
                clear_exclude,
                default,
            } => run_registry_set(
                ctx,
                alias,
                oci.as_deref(),
                index.as_deref(),
                *default,
                include,
                *clear_include,
                exclude,
                *clear_exclude,
            ),
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
        // field=`oci`.  The field must be one of `RegistryField::ALL`'s own
        // names (`url` accepted as the pre-0.7.0 alias for `oci`).
        if let Some(dot_pos) = rest.rfind('.') {
            let alias = &rest[..dot_pos];
            let field_str = &rest[dot_pos + 1..];
            if !alias.is_empty() && !field_str.is_empty() {
                // Matched against `RegistryField::ALL` rather than a
                // hand-written arm list, for the reason C-021 gives for
                // `collect_entries`: a field added to that array must become
                // addressable without a second edit here. `include` and
                // `exclude` (plan C-011) arrive for free through it.
                let field = match field_str {
                    "url" => RegistryField::Oci,
                    other => RegistryField::ALL
                        .into_iter()
                        .find(|f| f.field_name() == other)
                        .ok_or_else(|| {
                            // Escaped like every other message quoting a
                            // user-supplied key segment: a raw ESC echoed to
                            // stderr is a control-sequence-injection vector.
                            super::config_usage(format!(
                                "unknown registry field '{}'; valid fields: {}",
                                other.escape_debug(),
                                RegistryField::ALL.map(RegistryField::field_name).join(", ")
                            ))
                        })?,
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
                "no registry field specified for '{alias}'; use registry.{alias}.<field>, one of: {}",
                RegistryField::ALL.map(RegistryField::field_name).join(", ")
            )));
        }
        ParsedKey::RegistryAliasField { alias, field } => {
            let rc = find_registry(registries, alias).ok_or_else(|| {
                super::config_usage(format!("no registry '{alias}'; add it with `grim config registry add`"))
            })?;
            registry_field_value(rc, *field)
        }
    })
}

/// The effective value of one registry field on one entry, or `None` when
/// unset.
///
/// The single accessor behind both `config get` ([`get_value`]) and
/// `config list` ([`collect_entries`]), so the two can never disagree
/// about a registry field (plan C-021).
fn registry_field_value(rc: &RegistryConfig, field: RegistryField) -> Option<String> {
    match field {
        RegistryField::Oci => rc.oci.clone(),
        RegistryField::Index => rc.index.clone(),
        RegistryField::Include => pattern_list_value(&rc.include),
        RegistryField::Exclude => pattern_list_value(&rc.exclude),
        // `default` always has an effective value — it has no unset state.
        RegistryField::Default => Some(rc.default.to_string()),
    }
}

/// An authored `include`/`exclude` list as a display value: `None` when
/// empty, so it reads as unset everywhere (`get` exits 1, `list` omits the
/// row) exactly like the other empty-list keys (plan C-012).
///
/// The comma join is **display only and not round-trippable**: `set` takes
/// exactly one pattern and never splits (a comma is glob alternation
/// syntax), so feeding a multi-element rendering back would store it as one
/// literal pattern. `--format json` carries the true array.
fn pattern_list_value(patterns: &[String]) -> Option<String> {
    (!patterns.is_empty()).then(|| patterns.join(","))
}

/// Which verb is writing the pattern. The two paths accept exactly the same
/// set — they differ only in which remedy is reachable from where the user
/// is standing (plan C-012/C-013).
#[derive(Clone, Copy)]
enum WriteSite {
    /// `grim config set registry.<alias>.<field>`, reachable only once the
    /// entry exists.
    Set,
    /// `grim config registry add --include`/`--exclude`, reachable only
    /// while it does not.
    Add,
}

/// Quote an authored filter pattern for a CLI error message: `escape_debug`d
/// and **capped**, the set-time twin of `project_config::quote_pattern`
/// (private to its module, hence the second copy).
///
/// The cap is not cosmetic — [`crate::config::project_config::validate_filter_pattern`]
/// rejects a pattern for being over 1024 bytes, so this message is exactly
/// where an arbitrarily long pattern arrives, and quoting one whole turns a
/// 12 000-byte pattern into a 12 000-byte error line. The cut happens on the
/// **raw** pattern, before escaping: escaping first and truncating after
/// could split a `\u{…}` sequence in half.
fn quote_pattern(pattern: &str) -> String {
    /// Chars of the authored pattern shown before truncation. Escaping can
    /// expand each one, so the rendered quote is longer — bounded, which is
    /// the point, not exact.
    const MAX_SHOWN_CHARS: usize = 80;
    match pattern.char_indices().nth(MAX_SHOWN_CHARS) {
        None => format!("'{}'", pattern.escape_debug()),
        Some((cut, _)) => format!("'{}…' ({} bytes total)", pattern[..cut].escape_debug(), pattern.len()),
    }
}

/// Validate one authored browse-filter pattern at the CLI write boundary
/// (exit 65, plan C-012/C-013/S-016).
///
/// Delegates to [`crate::config::project_config::validate_filter_pattern`],
/// the same predicate load-time validation uses (exit 78), so the accepted
/// set cannot drift between a hand-edited config and the CLI. `site` selects
/// only the remedy named in the bare-comma warning, never what is accepted.
fn check_filter_pattern(value: &str, key: &str, site: WriteSite) -> anyhow::Result<()> {
    crate::config::project_config::validate_filter_pattern(value).map_err(|reason| {
        // Quoted through `quote_pattern` for the same two reasons the load
        // path is: a pattern is user-authored text on its way to stderr and
        // `char::is_control` does not cover the bidi/zero-width format
        // characters that reach the glob compiler intact, and the
        // over-length rejection arrives here carrying the over-long value.
        // The KEY is escaped too, not just the value: it interpolates the
        // authored alias (`registry.<alias>.include`), and `parse_key`'s
        // control-char screen is false for U+202E. Same call
        // `warn_on_discarded_patterns` already makes on the same alias.
        super::config_value(format!(
            "invalid value for {}: {} {reason}",
            key.escape_debug(),
            quote_pattern(value)
        ))
    })?;
    if has_bare_comma(value) {
        // One pattern is stored verbatim (C-012), so a top-level comma is
        // almost always a list the user expected to be split — most often
        // the comma-joined output of `config get` fed straight back in.
        // That compiles to a valid glob matching nothing, which browses
        // empty with no other symptom, so it must not pass silently. A
        // warning, never an error: `a,b` is a legal repository name.
        let reachable = match site {
            // `registry add --include` is deliberately absent here: `config
            // set` is reachable only once the alias exists, and `registry
            // add` on an existing alias is exit 64, so naming it would close
            // a loop with no exit.
            WriteSite::Set => "",
            // On `add` the flag is still open, and repeating it is the only
            // remedy that writes a real multi-pattern list — so it leads.
            WriteSite::Add => " Repeat the flag instead — `--include a --include b` accumulates.",
        };
        tracing::warn!(
            "{key}: {} is stored as ONE pattern — a comma is glob alternation, never a separator.{reachable} \
             If these were meant as separate patterns, brace them into one glob (`{{a,b}}`) or \
             write the list by hand in `grimoire.toml`.",
            quote_pattern(value)
        );
    }
    Ok(())
}

/// [`check_filter_pattern`] for the `config set` path, where an empty value
/// has a next command that the `registry add` path does not.
///
/// The shared validator's emptiness reason stays generic because load-time
/// validation (exit 78) reports it for a hand-edited file, where no `grim
/// config` verb applies — so the remedy is attached here, at the one
/// boundary that knows `unset` is what the user meant.
fn check_set_filter_pattern(value: &str, key: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        // Escaped twice over, and the second one is the one that matters: this
        // message hands the user a command to copy and run, so an unescaped
        // U+202E in the alias reorders how `grim config unset …` renders.
        let key = key.escape_debug();
        return Err(super::config_value(format!(
            "invalid value for {key}: must not be empty or whitespace-only; \
             clear the filter with `grim config unset {key}`"
        )));
    }
    check_filter_pattern(value, key, WriteSite::Set)
}

/// Validate the repeatable `--include` / `--exclude` values that
/// `registry add` and `registry set` share (plan C-013).
///
/// Runs before the lock so a bad pattern is exit 65 with nothing written
/// (S-016). Each flag is repeatable and accumulates; values are never split
/// on a comma, which is glob alternation syntax — these two flags are the
/// only CLI path that writes a multi-pattern list.
fn check_filter_flags(alias: &str, include: &[String], exclude: &[String]) -> anyhow::Result<()> {
    for (field, patterns) in [("include", include), ("exclude", exclude)] {
        let key = format!("registry.{alias}.{field}");
        for pattern in patterns {
            check_filter_pattern(pattern, &key, WriteSite::Add)?;
        }
        // C-006's fourth check — the whole-list budget, which the per-pattern
        // loop structurally cannot see: no single pattern is over any
        // per-pattern cap. Without it the aggregate rejection reached the user
        // only from `commit_config` → `validate_registries`, as exit **78**
        // naming the `grimoire.toml` path for a value that arrived through
        // `--include`/`--exclude` flags on a file nobody edited. `compile_set`
        // is the same seam load-time validation uses, so what the two accept
        // cannot drift; `commit_config`'s check stays the backstop for a
        // hand-edited file.
        crate::config::registry_filter::compile_set(patterns)
            .map_err(|reason| super::config_value(format!("invalid value for {key}: {reason}")))?;
    }
    Ok(())
}

/// Warn when a one-pattern `set` discards a multi-pattern browse filter.
///
/// `set` writes exactly one pattern and replaces the whole list (C-012), so
/// on an entry that already carries several it destroys committed config at
/// exit 0 under a report that reads as an addition — and because the
/// surviving pattern leaves the filter *partially* correct, the
/// `filter admitted M of N` diagnostic stays silent too. Naming the count is
/// the point: "one of them worked" is exactly why the loss goes unnoticed.
fn warn_on_discarded_patterns(alias: &str, field: RegistryField, previous: &[String]) {
    if previous.len() > 1 {
        // Escaped like every other message quoting the alias — `parse_key`
        // screens control characters, but not U+202E.
        let shown = alias.escape_debug();
        tracing::warn!(
            "registry.{shown}.{}: `grim config set` writes ONE pattern and replaces the whole list — \
             the {} patterns already stored are discarded, not appended to. To write several, use \
             `grim config registry set {shown}` with repeated --include/--exclude flags, which edits \
             the entry in place; or edit `grimoire.toml` by hand.",
            field.field_name(),
            previous.len()
        );
    }
}

/// Whether `pattern` carries a comma outside every `{…}` group — the shape
/// of a list somebody expected to be split, rather than legitimate glob
/// alternation (`acme/{platform,tools}/**`, which must stay silent).
fn has_bare_comma(pattern: &str) -> bool {
    let mut depth = 0usize;
    for ch in pattern.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return true,
            _ => {}
        }
    }
    false
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
                // The client name is a valid KEY (checked in `parse_key`), so
                // a client that cannot host the pool is a bad VALUE — exit 65,
                // the same mapping `check_clients` gets at set time. Shares its
                // accepted set with load-time validation via
                // `check_pool_capable`.
                crate::config::project_config::check_pool_capable(vendor).map_err(super::config_value)?;
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
             use registry.{alias}.<field>, one of: {}",
            RegistryField::ALL.map(RegistryField::field_name).join(", ")
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
                        // Both interpolations escaped — the second is a command
                        // to copy and run. Pre-existing (`main` carries the
                        // same line), fixed here because it is the same edit.
                        let alias = alias.escape_debug();
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
                        let alias = alias.escape_debug();
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
                // Exactly ONE pattern, replacing the whole list (plan
                // C-012). Deliberately no comma split — the house
                // `StringList` style (`options.clients`,
                // `options.tui.tree_separators`) cannot apply here because a
                // comma is glob alternation syntax, and splitting would make
                // `acme/{platform,tools}/**` unwritable. Several patterns are
                // written with repeated `registry add --include` (C-013) or
                // by editing `grimoire.toml`.
                RegistryField::Include => {
                    check_set_filter_pattern(value_str, &format!("registry.{alias}.include"))?;
                    // After validation, so a rejected pattern never warns
                    // about a list it was never going to replace.
                    if let Some(rc) = find_registry(registries, alias) {
                        warn_on_discarded_patterns(alias, *field, &rc.include);
                    }
                    let replacement = vec![value_str.to_string()];
                    set_registry_field(registries, alias, |rc| rc.include = replacement);
                    Ok(value_str.to_string())
                }
                RegistryField::Exclude => {
                    check_set_filter_pattern(value_str, &format!("registry.{alias}.exclude"))?;
                    if let Some(rc) = find_registry(registries, alias) {
                        warn_on_discarded_patterns(alias, *field, &rc.exclude);
                    }
                    let replacement = vec![value_str.to_string()];
                    set_registry_field(registries, alias, |rc| rc.exclude = replacement);
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

/// The shared "no such registry" usage error for `unset`'s field arms.
///
/// One function rather than four copies of one string, and the alias is
/// escaped for the reason `validate_alias_format` gives: `char::is_control`
/// never matches U+202E, so a bidi override reaches every message that
/// quotes an alias intact.
fn no_such_registry_for_unset(alias: &str) -> anyhow::Error {
    super::config_usage(format!(
        "no registry '{}'; cannot unset a field on a registry that does not exist",
        alias.escape_debug()
    ))
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
                // The bare `registry.<alias>` form is the ONE key shape
                // `parse_key` does not run `validate_alias_format` over, so
                // this message is reached with control characters intact —
                // a raw ESC on stderr is a control-sequence-injection
                // vector, the same one every other quoted segment escapes.
                return Err(super::config_usage(format!(
                    "no registry '{}'; cannot remove a registry that does not exist",
                    alias.escape_debug()
                )));
            }
            registries.retain(|r| r.alias.as_deref() != Some(alias.as_str()));
            Ok(())
        }
        ParsedKey::RegistryAliasField { alias, field } => match field {
            RegistryField::Oci => {
                let Some(rc) = find_registry(registries, alias) else {
                    return Err(no_such_registry_for_unset(alias));
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
                    return Err(no_such_registry_for_unset(alias));
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
            // `unset` clears to empty (plan C-012) — the entry survives with
            // no filter, unlike `oci`/`index` where clearing the last
            // locator would leave the entry sourceless.
            RegistryField::Include => {
                if find_registry(registries, alias).is_none() {
                    return Err(no_such_registry_for_unset(alias));
                }
                set_registry_field(registries, alias, |rc| rc.include.clear());
                Ok(())
            }
            RegistryField::Exclude => {
                if find_registry(registries, alias).is_none() {
                    return Err(no_such_registry_for_unset(alias));
                }
                set_registry_field(registries, alias, |rc| rc.exclude.clear());
                Ok(())
            }
            RegistryField::Default => {
                if find_registry(registries, alias).is_none() {
                    return Err(super::config_usage(format!(
                        "no registry '{}'; cannot unset default on a registry that does not exist",
                        alias.escape_debug()
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
            // Iterate `RegistryField::ALL` rather than naming the fields one
            // at a time (plan C-021): `list [--all]` documents itself as
            // listing every supported key, so a field added to that array
            // has to appear here for free — hand-written branches silently
            // made that promise false. The value comes from the same
            // accessor `get` uses, so the two surfaces cannot disagree.
            // Row order follows `ALL`, whose first three entries are the
            // shipped `oci, index, default` sequence.
            for field in RegistryField::ALL {
                let value = registry_field_value(rc, field);
                if value.is_some() || all {
                    entries.push(entry(
                        format!("registry.{alias}.{}", field.field_name()),
                        value,
                        field.spec(),
                    ));
                }
            }
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
            fields: Vec::new(),
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
            fields: Vec::new(),
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
    include: &[String],
    exclude: &[String],
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

    // Before the pattern loop, so C-013's "duplicate alias → 64" holds
    // unconditionally: `registry add <existing> --include '<malformed>'` is a
    // usage error about the alias, not a value error about a pattern that
    // could never have been written anyway.
    if scope.registries.iter().any(|r| r.alias.as_deref() == Some(alias)) {
        // Set-time twin of the load-time `duplicate alias '{shown}'` message.
        // `validate_alias_format` above does not stop U+202E — nothing rejects
        // a format character in an alias — so it reaches this message intact.
        let shown = alias.escape_debug();
        // The filter clause is not decoration: this message is where a user
        // adding a second `--include` to an existing entry lands, and
        // without it they read "use `config set` instead" — which replaces
        // the whole list rather than appending to it (B-2). `registry set`
        // takes the same repeated flags this invocation already carries, so
        // naming it turns the error into a one-word edit.
        return Err(super::config_usage(format!(
            "registry '{shown}' already exists; edit it in place with `grim config registry set \
             {shown}`, which takes these same --oci/--index/--include/--exclude/--default flags, \
             or remove it with `grim config registry rm {shown}`. To change its browse filter use \
             that verb's repeated --include/--exclude flags, not `grim config set \
             registry.{shown}.include`, which writes ONE pattern and replaces the whole list"
        )));
    }

    check_filter_flags(alias, include, exclude)?;

    let _guard = acquire_config_lock(&scope)?;

    let mut registries = scope.registries.clone();

    if make_default {
        clear_all_defaults(&mut registries);
    }
    registries.push(RegistryConfig {
        alias: Some(alias.to_string()),
        oci: (!is_index).then(|| locator.to_string()),
        index: is_index.then(|| locator.to_string()),
        // `registry add` with neither flag declares an unfiltered entry
        // (plan C-013).
        include: include.to_vec(),
        exclude: exclude.to_vec(),
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
            fields: Vec::new(),
        }),
        ExitCode::Success,
    ))
}

/// Edit an existing registry entry **in place**, leaving unnamed fields alone.
///
/// Patch semantics, in three states rather than two: a flag given replaces its
/// field, a flag absent leaves it untouched, and a list flag absent *with its
/// `--clear-*` twin given* empties the field. That third state is the whole
/// reason the clear flags exist — "give me an empty list" is the one edit a
/// repeatable flag cannot express, since giving it zero times already means
/// "leave it alone" (design C-013…C-016).
///
/// A repeatable list flag replaces that whole list, so this is the one write
/// path that can grow a browse filter past a single pattern —
/// `config set registry.<alias>.include` writes exactly one and discards the
/// rest (**plan C-012** — `.agents/plans/plan_registry_browse_filters.md`;
/// the two numbering spaces cited in this file overlap in range, and
/// `design_registry_filter_candidate.md`'s own C-012 is a different
/// contract).
///
/// Holding the entry's index is the point of the verb. The old remedy for a
/// multi-pattern edit was `registry rm` + re-`add`, but `add` *pushes*, so
/// re-creating an entry moves it last, and `resolve_registries` falls back to
/// "first entry wins" when no entry declares `default` — the round-trip could
/// silently move the default out from under a config that never named one.
/// `set_registry_field` mutates the entry where it already sits, so it cannot.
///
/// `--default` only ever *sets* the flag (clearing every other entry's, like
/// `registry use`). There is deliberately no way to unset it here: the default
/// has to live somewhere, so it moves by naming another entry.
#[allow(
    clippy::too_many_arguments,
    reason = "design C-014: the two clear flags travel flat, exactly as `default` already does. Collapsing this signature into a params struct is deliberately deferred — mixing a refactor into a feature diff violates the Two Hats Rule (quality-core.md)"
)]
fn run_registry_set(
    ctx: &Context,
    alias: &str,
    oci: Option<&str>,
    index: Option<&str>,
    // The one edit patch semantics cannot express: a list flag given zero
    // times means "leave it alone", so emptying a list needs its own flag
    // (design C-013…C-016). Clap's `conflicts_with` rules out the same-side
    // pair, so `--clear-include` and `--include` never both arrive.
    //
    // Each clear flag sits BESIDE the list it clears rather than beside the
    // other clear flag. With no two `bool`s adjacent, every pairwise
    // transposition of the three becomes a type error at compile time —
    // otherwise a swap in the `run` dispatch arm is invisible to all 2570
    // unit tests, since every one of them calls this function directly and
    // none can observe how `run` forwards into it (design E-14, superseded).
    // `make_default`'s identical exposure on `registry use`'s side is a
    // released signature and stays as it is (Two Hats).
    make_default: bool,
    include: &[String],
    clear_include: bool,
    exclude: &[String],
    clear_exclude: bool,
) -> anyhow::Result<(ConfigReport, ExitCode)> {
    // Unlike `add`, neither locator flag is the "leave it alone" case; clap's
    // `conflicts_with` rejects both at once, so only three shapes reach here.
    let locator = match (oci, index) {
        (Some(u), None) => Some((u, false)),
        (None, Some(i)) => Some((i, true)),
        _ => None,
    };

    // A `set` naming no field would take the lock, rewrite the file and report
    // a change that never happened. Exit 64 instead — the same class as a
    // missing alias, and the one thing clap cannot express as an arg rule.
    if locator.is_none()
        && !make_default
        && include.is_empty()
        && exclude.is_empty()
        && !clear_include
        && !clear_exclude
    {
        return Err(super::config_usage(format!(
            "nothing to change for registry '{}'; name at least one of \
             --oci/--index, --include, --exclude, --clear-include, \
             --clear-exclude, --default. To clear a browse filter use \
             --clear-include/--clear-exclude, or with \
             `grim config unset registry.{}.include`",
            alias.escape_debug(),
            alias.escape_debug()
        )));
    }

    if let Some((locator, is_index)) = locator {
        reject_control_chars(locator, if is_index { "registry.index" } else { "registry.oci" })?;
        if is_index && crate::config::registry_resolve::classify_index(locator).is_none() {
            // Escaped like its `add` and `apply_set` twins: the control-char
            // guard on the line before does not match bidi and zero-width
            // format characters, which reach this message intact.
            return Err(super::config_value(format!(
                "invalid index locator '{}': must be an http(s):// base or a \
                 git repository (git+…, ssh://, git@…, or ending in .git)",
                locator.escape_debug()
            )));
        }
    }

    let scope = super::grim(scope_resolution::resolve(ctx, ctx.global(), ctx.config()))?;
    let origin = scope_to_origin(scope.scope);

    // Before the pattern checks, mirroring `add`'s duplicate-alias ordering:
    // `registry set <missing> --include '<malformed>'` is a usage error about
    // the alias, not a value error about a pattern with nowhere to go.
    let Some(existing) = find_registry(&scope.registries, alias) else {
        return Err(super::config_usage(format!(
            "no registry '{}'; add it with `grim config registry add`",
            alias.escape_debug()
        )));
    };
    check_filter_flags(alias, include, exclude)?;

    let _guard = acquire_config_lock(&scope)?;

    let mut registries = scope.registries.clone();

    if make_default {
        clear_all_defaults(&mut registries);
        set_registry_default(&mut registries, alias, true);
    }
    if let Some((locator, is_index)) = locator {
        // Swapping the kind clears the other side. `config set
        // registry.<a>.index` refuses this and tells the user to unset `oci`
        // first; here the locator flag *is* the whole declaration, so there is
        // nothing ambiguous to refuse — and an entry carrying both would fail
        // `validate_registries` on the way out anyway.
        set_registry_field(&mut registries, alias, |rc| {
            rc.oci = (!is_index).then(|| locator.to_string());
            rc.index = is_index.then(|| locator.to_string());
        });
    }
    // `else if` rather than a second `if`. Clap's `conflicts_with` rules the
    // "patterns given AND cleared" pair out — but only at the ONE production
    // call site (`run`'s dispatch arm); the unit tests call this function
    // directly and bypass clap entirely, so this is a call-site guarantee, not
    // a proof about the parameters. On a direct both-given call the patterns
    // simply win, which is the sane reading and needs no assertion.
    //
    // Neither arm may run on an empty slice with no clear flag — that is the
    // surviving mutant C-020 row 1 exists to kill, and it silently destroys a
    // committed filter on every unrelated edit.
    if !include.is_empty() {
        set_registry_field(&mut registries, alias, |rc| rc.include = include.to_vec());
    } else if clear_include {
        set_registry_field(&mut registries, alias, |rc| rc.include.clear());
    }
    if !exclude.is_empty() {
        set_registry_field(&mut registries, alias, |rc| rc.exclude = exclude.to_vec());
    } else if clear_exclude {
        set_registry_field(&mut registries, alias, |rc| rc.exclude.clear());
    }

    commit_config(&scope, &scope.options, &registries)?;

    Ok((
        ConfigReport::Write(ConfigWriteReport {
            action: WriteAction::RegistrySet,
            key: format!("registry.{alias}"),
            // The locator is the only single-valued field a caller can have
            // changed; a filter-only edit has no one value to report.
            value: locator.map(|(l, _)| l.to_string()),
            scope: origin,
            dry_run: false,
            fields: registry_set_fields(
                locator,
                existing,
                make_default,
                include,
                clear_include,
                exclude,
                clear_exclude,
            ),
        }),
        ExitCode::Success,
    ))
}

/// The `fields` array of a `registry set` write report — one element per field
/// the write touched, in `RegistryField::ALL` order (design C-021).
///
/// Built by iterating `ALL` and mapping each member through an **exhaustive**
/// match, so the order and every spelling come from `config_keys.rs`
/// structurally and a sixth `RegistryField` becomes a compile error here
/// rather than a row silently missing from a released JSON surface. Iterating
/// the enum instead would emit `RegistryField`'s *declaration* order, which
/// differs from `ALL`'s frozen positional order on purpose (design E-6).
///
/// It reports the write, not the invocation (design E-12): a field named with
/// a value it already held still emits its element — `fields` describes the
/// assignment performed and the resulting state, never a before/after diff.
///
/// `existing` is the entry as it stood **before** the write, and is read for
/// exactly one thing: which locator side actually held a value, so a kind swap
/// reports `cleared` for the side it emptied and nothing for a side that was
/// already absent. It is the caller's own `&RegistryConfig`, passed instead of
/// two derived `bool`s so the parameter list stays inside clippy's threshold
/// without an `allow`.
fn registry_set_fields(
    locator: Option<(&str, bool)>,
    existing: &RegistryConfig,
    make_default: bool,
    include: &[String],
    clear_include: bool,
    exclude: &[String],
    clear_exclude: bool,
) -> Vec<RegistryFieldChange> {
    let set_string = |value: &str| RegistryFieldChangeAction::Set {
        value: RegistryFieldValue::String(value.to_string()),
    };
    // Mirrors the write arms above: a non-empty list is a replacement, an
    // empty one with the clear flag is a clear, and neither is a no-op row.
    let list = |patterns: &[String], clear: bool| {
        if patterns.is_empty() {
            clear.then_some(RegistryFieldChangeAction::Cleared)
        } else {
            Some(RegistryFieldChangeAction::Set {
                value: RegistryFieldValue::List(patterns.to_vec()),
            })
        }
    };

    RegistryField::ALL
        .into_iter()
        .filter_map(|field| {
            let action = match field {
                RegistryField::Oci => match locator {
                    Some((url, false)) => Some(set_string(url)),
                    Some((_, true)) => existing.oci.is_some().then_some(RegistryFieldChangeAction::Cleared),
                    None => None,
                },
                RegistryField::Index => match locator {
                    Some((url, true)) => Some(set_string(url)),
                    Some((_, false)) => existing.index.is_some().then_some(RegistryFieldChangeAction::Cleared),
                    None => None,
                },
                // `--default` only ever sets; there is no way to unset it here.
                RegistryField::Default => make_default.then_some(RegistryFieldChangeAction::Set {
                    value: RegistryFieldValue::Bool(true),
                }),
                RegistryField::Include => list(include, clear_include),
                RegistryField::Exclude => list(exclude, clear_exclude),
            };
            action.map(|action| RegistryFieldChange {
                field: field.field_name(),
                action,
            })
        })
        .collect()
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
            fields: Vec::new(),
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
            fields: Vec::new(),
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
            include: rc.include.clone(),
            exclude: rc.exclude.clone(),
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
            include: rc.include.clone(),
            exclude: rc.exclude.clone(),
            default: rc.default,
        })
        .collect();
    Ok((
        ConfigReport::RegistryList(RegistryListReport { items }),
        ExitCode::Success,
    ))
}

/// `grim config registry fields` — static metadata for the 5 addressable
/// per-registry fields (`oci`, `index`, `default`, and the browse filters
/// `include` / `exclude`). Unlike every other
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

    /// P5: the pattern *value* was already escaped through `quote_pattern`;
    /// the *key* was not, and it interpolates the authored alias. Both
    /// messages below hand the user a `grim config unset …` command to copy
    /// and run, so an unescaped bidi override reorders how that command
    /// renders — and `parse_key`'s control screen is false for U+202E.
    #[test]
    fn filter_pattern_errors_escape_the_key_p5() {
        const BIDI_OVERRIDE: char = '\u{202e}';
        let key = format!("registry.ac{BIDI_OVERRIDE}me.include");

        for message in [
            // The empty-value arm, which carries the copy-pasteable command.
            check_set_filter_pattern("   ", &key).expect_err("whitespace-only is rejected"),
            // The glob-compile arm, which carries the key once.
            check_filter_pattern("acme{unclosed", &key, WriteSite::Set).expect_err("an unclosed group is rejected"),
        ] {
            let text = format!("{message:#}");
            assert!(
                !text.contains(BIDI_OVERRIDE),
                "no raw bidi override may reach stderr; got: {text:?}"
            );
            assert!(
                text.contains("registry.ac\\u{202e}me.include"),
                "the key must still be readable, escaped; got: {text:?}"
            );
        }
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
                    ..
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
            ..Default::default()
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
                ..Default::default()
            },
            RegistryConfig {
                alias: Some("b".to_string()),
                oci: Some("u2".to_string()),
                index: None,
                default: false,
                ..Default::default()
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

    // ── C-011…C-014, C-021: per-registry browse filters ──────────────────────

    /// One aliased registry carrying an authored filter on both sides.
    fn filtered_registries() -> Vec<RegistryConfig> {
        vec![RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            include: vec!["acme/platform/**".to_string(), "acme/tools/**".to_string()],
            exclude: vec!["acme/platform/legacy/**".to_string()],
            default: false,
        }]
    }

    /// The same entry with no filter authored on either side.
    fn unfiltered_registries() -> Vec<RegistryConfig> {
        vec![RegistryConfig {
            alias: Some("acme".to_string()),
            oci: Some("ghcr.io/acme".to_string()),
            ..Default::default()
        }]
    }

    #[test]
    fn parse_key_recognizes_include_and_exclude() {
        // Plan C-011: the two filter fields become addressable as
        // `registry.<alias>.<field>`, closing WP-A's interim state where
        // `config registry fields` advertised keys `parse_key` rejected.
        assert!(matches!(
            parse_key("registry.acme.include"),
            Ok(ParsedKey::RegistryAliasField { alias, field: RegistryField::Include })
            if alias == "acme"
        ));
        assert!(matches!(
            parse_key("registry.acme.exclude"),
            Ok(ParsedKey::RegistryAliasField { alias, field: RegistryField::Exclude })
            if alias == "acme"
        ));
        // A dotted alias stays addressable (the rightmost-dot split).
        assert!(matches!(
            parse_key("registry.a.b.include"),
            Ok(ParsedKey::RegistryAliasField { alias, field: RegistryField::Include })
            if alias == "a.b"
        ));
    }

    #[test]
    fn parse_key_registry_field_set_is_exactly_registry_field_all() {
        // Every declared field parses, and nothing else does — the match is
        // driven by `RegistryField::ALL`, so a future field is addressable
        // without editing `parse_key` and this test needs no edit either.
        for f in RegistryField::ALL {
            let key = format!("registry.acme.{}", f.field_name());
            assert!(
                matches!(parse_key(&key), Ok(ParsedKey::RegistryAliasField { field, .. }) if field == f),
                "every registry field must parse: {key}"
            );
        }
        let msg = parse_key("registry.acme.bogus")
            .expect_err("an unknown registry field must be rejected")
            .to_string();
        for f in RegistryField::ALL {
            assert!(
                msg.contains(f.field_name()),
                "the unknown-field error must name '{}'; got: {msg}",
                f.field_name()
            );
        }
    }

    #[test]
    fn filter_get_is_unset_when_the_list_is_empty() {
        // Plan C-012 / S-009: an empty list is unset — `get` exits 1 with
        // no output, exactly like the other empty-list keys.
        let options = fresh_options();
        let registries = unfiltered_registries();
        for field in ["include", "exclude"] {
            let key = parse_key(&format!("registry.acme.{field}")).unwrap();
            assert_eq!(
                get_value(&key, &options, &registries).unwrap(),
                None,
                "an empty {field} list must read as unset, not as an empty string"
            );
        }
    }

    #[test]
    fn filter_get_reads_each_side_from_its_own_field() {
        // Plan C-012: display-only comma join, and — the mutation that
        // matters — each key reads its OWN list. Swapping the two arms
        // inverts an allowlist into a denylist and fails right here.
        let options = fresh_options();
        let registries = filtered_registries();
        assert_eq!(
            get_value(&parse_key("registry.acme.include").unwrap(), &options, &registries).unwrap(),
            Some("acme/platform/**,acme/tools/**".to_string())
        );
        assert_eq!(
            get_value(&parse_key("registry.acme.exclude").unwrap(), &options, &registries).unwrap(),
            Some("acme/platform/legacy/**".to_string())
        );
    }

    #[test]
    fn filter_set_replaces_the_whole_list_with_exactly_one_pattern() {
        // Plan C-012: `set` takes exactly one pattern and replaces the whole
        // list. Writing several is `registry add --include` repeated (C-013)
        // or a hand edit — a deliberate, documented limitation.
        let mut options = fresh_options();
        let mut registries = filtered_registries();
        let stored = apply_set(
            &parse_key("registry.acme.include").unwrap(),
            "acme/next/**",
            &mut options,
            &mut registries,
        )
        .expect("a valid pattern must be accepted");
        assert_eq!(stored, "acme/next/**");
        assert_eq!(registries[0].include, vec!["acme/next/**".to_string()]);
        assert_eq!(
            registries[0].exclude,
            vec!["acme/platform/legacy/**".to_string()],
            "setting one side must not disturb the other"
        );
    }

    #[test]
    fn filter_set_never_splits_on_a_comma() {
        // Plan C-012/C-013: a comma is glob alternation syntax. Splitting on
        // it would make `acme/{platform,tools}/**` unwritable, so this path
        // deliberately diverges from the `StringList` house comma-split that
        // `options.clients` and `options.tui.tree_separators` use.
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        apply_set(
            &parse_key("registry.acme.include").unwrap(),
            "acme/{platform,tools}/**",
            &mut options,
            &mut registries,
        )
        .expect("brace alternation must survive intact");
        assert_eq!(
            registries[0].include,
            vec!["acme/{platform,tools}/**".to_string()],
            "the value must be stored as ONE pattern, never split into two"
        );
    }

    #[test]
    fn filter_set_rejects_an_invalid_pattern_as_a_data_error() {
        // Plan S-016 / the error taxonomy: a malformed pattern through
        // `config set` is exit 65 with nothing written — not the exit 78 a
        // hand-edited config gets at load. The shared predicate is
        // `project_config::validate_filter_pattern`, so the accepted set
        // cannot drift between the two paths.
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        // `"   "` is the silent one: it compiles to a valid glob matching
        // nothing, so accepting it would empty the browse set with no
        // diagnostic at all.
        for value in ["acme{unclosed", "", "   ", "a\tb"] {
            let err = apply_set(
                &parse_key("registry.acme.include").unwrap(),
                value,
                &mut options,
                &mut registries,
            )
            .expect_err("an invalid pattern must be rejected");
            assert_eq!(
                crate::error::classify_error(&err),
                ExitCode::DataError,
                "a bad pattern must exit 65, not 78; value: {value:?}"
            );
        }
        assert!(
            registries[0].include.is_empty(),
            "a rejected pattern must leave the list untouched"
        );
    }

    #[test]
    fn filter_set_message_never_echoes_a_raw_hostile_byte() {
        // Same discipline as every other message in this file: a pattern is
        // user-supplied and reaches stderr, so ESC must be escaped — and
        // U+202E is not `char::is_control`, so it sails past the control
        // check into the glob compiler's own error.
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        for (value, raw) in [("a\u{1b}[2Jb", '\u{1b}'), ("acme{\u{202e}", '\u{202e}')] {
            let msg = apply_set(
                &parse_key("registry.acme.exclude").unwrap(),
                value,
                &mut options,
                &mut registries,
            )
            .expect_err("a hostile pattern must be rejected")
            .to_string();
            assert!(
                !msg.contains(raw),
                "message for {value:?} must not embed the raw {raw:?} byte: {msg:?}"
            );
        }
    }

    #[test]
    fn bare_comma_flags_a_list_but_not_brace_alternation() {
        // The `get` → `set` round trip stores a comma-joined list as ONE
        // literal glob that matches nothing, so `set` warns on it. Legal
        // alternation must stay silent, or the warning trains users to
        // ignore it.
        for suspicious in ["a/**,b/**", "a,b", "{a,b},c", "acme/**,"] {
            assert!(has_bare_comma(suspicious), "must flag {suspicious:?}");
        }
        for legitimate in [
            "acme/{platform,tools}/**",
            "acme/**",
            "a/{b,c}",
            "a/{b,{c,d}}/**",
            "acme",
        ] {
            assert!(!has_bare_comma(legitimate), "must stay silent on {legitimate:?}");
        }
    }

    #[test]
    fn filter_set_warns_but_still_stores_a_comma_joined_value() {
        // Warning only, never an error: `a,b` is a legal repository name,
        // and C-012's one-pattern `set` is the design. What must not happen
        // is silence.
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        let stored = apply_set(
            &parse_key("registry.acme.include").unwrap(),
            "a/**,b/**",
            &mut options,
            &mut registries,
        )
        .expect("a comma is legal in a pattern — warn, do not reject");
        assert_eq!(stored, "a/**,b/**");
        assert_eq!(registries[0].include, vec!["a/**,b/**".to_string()]);
    }

    /// A `std::io::Write` sink over a shared buffer, so a `tracing` event can
    /// be asserted as text. Same shape as `catalog_service`'s capture helper.
    struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer is never poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `f` with `tracing` captured thread-locally, returning what it wrote.
    fn capture_logs(f: impl FnOnce()) -> String {
        crate::log_switch::tracing_capture::arm();
        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = std::sync::Arc::clone(&logs);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || SharedBuf(std::sync::Arc::clone(&sink)))
                .with_ansi(false)
                .without_time()
                .finish(),
        );
        f();
        drop(guard);
        String::from_utf8(logs.lock().expect("log buffer is never poisoned").clone()).expect("tracing writes UTF-8")
    }

    #[test]
    fn filter_set_comma_warning_names_only_remedies_that_work() {
        // H2/H3: `config set registry.<alias>.include` is reachable only when
        // the alias already exists, and `registry add` on an existing alias is
        // exit 64 by design — so the warning must not route the user there.
        // Both remedies it names work from either call site that fires it.
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        let logs = capture_logs(|| {
            apply_set(
                &parse_key("registry.acme.include").unwrap(),
                "acme/platform/**,acme/tools/**",
                &mut options,
                &mut registries,
            )
            .expect("a comma is legal in a pattern — warn, do not reject");
        });
        assert!(
            logs.contains("a comma is glob alternation, never a separator"),
            "the warning must keep its point: {logs}"
        );
        assert!(logs.contains("{a,b}"), "brace alternation must be offered: {logs}");
        assert!(
            logs.contains("grimoire.toml"),
            "writing the list by hand must be offered: {logs}"
        );
        assert!(
            !logs.contains("registry add"),
            "`registry add` is exit 64 on an existing alias — never a remedy here: {logs}"
        );
    }

    #[test]
    fn filter_set_warns_when_it_discards_an_existing_multi_pattern_list() {
        // B-2: `set` replaces the whole list (C-012), so on an entry that
        // already carries several patterns it destroys committed config at
        // exit 0 with a report that reads as an addition — and the filter
        // stays PARTIALLY correct, so no downstream diagnostic fires either.
        // The count is named because "one of them survived" is the whole
        // reason this is silent.
        let mut options = fresh_options();
        let mut registries = filtered_registries();
        let logs = capture_logs(|| {
            apply_set(
                &parse_key("registry.acme.include").unwrap(),
                "acme/tools/**",
                &mut options,
                &mut registries,
            )
            .expect("a valid pattern must be accepted");
        });
        assert!(
            logs.contains("registry.acme.include"),
            "the warning must name the key it overwrote: {logs}"
        );
        assert!(logs.contains('2'), "the discarded count must be named: {logs}");
        assert!(
            logs.contains("grim config registry set acme"),
            "the verb that writes a multi-pattern list must be named: {logs}"
        );
        assert!(
            logs.contains("--include"),
            "and the repeated flags that do it, since the verb alone is not the fix: {logs}"
        );
        assert_eq!(
            registries[0].include,
            vec!["acme/tools/**".to_string()],
            "the warning does not change what `set` writes"
        );
    }

    #[test]
    fn filter_set_stays_silent_when_it_replaces_at_most_one_pattern() {
        // The warning must fire on data loss, not on every `set` — a first
        // write and a one-for-one overwrite discard nothing worth naming.
        for mut registries in [unfiltered_registries(), {
            let mut one = unfiltered_registries();
            one[0].include = vec!["acme/platform/**".to_string()];
            one
        }] {
            let mut options = fresh_options();
            let logs = capture_logs(|| {
                apply_set(
                    &parse_key("registry.acme.include").unwrap(),
                    "acme/tools/**",
                    &mut options,
                    &mut registries,
                )
                .expect("a valid pattern must be accepted");
            });
            assert!(
                !logs.contains("discard"),
                "replacing 0 or 1 patterns is not data loss: {logs}"
            );
        }
    }

    #[test]
    fn add_path_comma_warning_names_repeating_the_flag() {
        // W-6: the warning is shared by both write paths, but the remedies
        // are not. On `registry add` the flag is still open, so repeating it
        // is the one remedy that writes a real multi-pattern list — and it
        // was the one clause missing, because the text was worded for the
        // `config set` path (where `registry add` is exit 64).
        let (_tmp, _config_path, ctx) = project_scope();
        let logs = capture_logs(|| {
            run_registry_add(
                &ctx,
                "acme",
                Some("ghcr.io/acme"),
                None,
                false,
                &["platform/**,tools/**".to_string()],
                &[],
            )
            .map(|_| ())
            .expect("a comma is legal in a pattern — warn, do not reject");
        });
        assert!(
            logs.contains("a comma is glob alternation, never a separator"),
            "the warning must keep its point: {logs}"
        );
        assert!(
            logs.contains("--include"),
            "the add path must name repeating the flag: {logs}"
        );
    }

    #[test]
    fn filter_set_message_caps_a_huge_pattern() {
        // S-4: `validate_filter_pattern` rejects a pattern for being too
        // long, so this message is exactly where an arbitrarily long pattern
        // arrives. The load path already caps it (`quote_pattern`); the CLI
        // path interpolated it whole, turning a 2 000-byte pattern into a
        // 2 000-byte error line.
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        let huge = "a".repeat(2000);
        let msg = apply_set(
            &parse_key("registry.acme.include").unwrap(),
            &huge,
            &mut options,
            &mut registries,
        )
        .expect_err("an over-long pattern must be rejected")
        .to_string();
        assert!(
            !msg.contains(&huge),
            "the whole pattern must not reach stderr; message was {} bytes",
            msg.len()
        );
        assert!(
            msg.contains("2000 bytes total"),
            "the true byte count must survive the cut: {msg}"
        );
    }

    #[test]
    fn unset_messages_never_echo_a_raw_hostile_alias() {
        // S-9: `parse_key` screens control characters through
        // `validate_alias_format`, but `char::is_control` never matches
        // U+202E, so a bidi override reaches these messages intact. Every
        // arm of `apply_unset` quotes the alias, so every arm escapes it —
        // fixing four and leaving the fifth is the shape that rots.
        let hostile = "ac\u{202e}me";
        for field in RegistryField::ALL {
            let key = format!("registry.{hostile}.{}", field.field_name());
            let parsed = parse_key(&key).expect("a bidi override is a legal alias character");
            let mut options = fresh_options();
            let mut registries: Vec<RegistryConfig> = vec![];
            let msg = apply_unset(&parsed, &mut options, &mut registries)
                .expect_err("no such registry")
                .to_string();
            assert!(
                !msg.contains('\u{202e}'),
                "the {} arm must not embed the raw override: {msg:?}",
                field.field_name()
            );
        }
        // The bare-alias form is the one `parse_key` does NOT validate, so a
        // raw ESC reaches its message too.
        let parsed = parse_key("registry.a\u{1b}[2Jb").expect("a bare alias is parsed unvalidated");
        let msg = apply_unset(&parsed, &mut fresh_options(), &mut vec![])
            .expect_err("no such registry")
            .to_string();
        assert!(
            !msg.contains('\u{1b}'),
            "the bare-alias arm must not embed a raw ESC: {msg:?}"
        );
    }

    #[test]
    fn filter_set_on_an_empty_value_names_unset() {
        // W16: the plan's taxonomy — an empty value is exit 65 and `unset` is
        // the clear path. The shared validator's reason stays generic because
        // it also serves load-time validation, where no `grim config` verb
        // applies.
        for value in ["", "   "] {
            let mut options = fresh_options();
            let mut registries = unfiltered_registries();
            let err = apply_set(
                &parse_key("registry.acme.include").unwrap(),
                value,
                &mut options,
                &mut registries,
            )
            .expect_err("an empty pattern must be rejected");
            assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
            let msg = err.to_string();
            assert!(
                msg.contains("grim config unset registry.acme.include"),
                "an empty value must name the command that clears the filter: {msg}"
            );
        }
    }

    #[test]
    fn a_bare_registry_alias_hint_lists_every_field() {
        // S5: both the `get` and the `set` hint named 2 of the 5 fields.
        // Written against `RegistryField::ALL` for the same reason the
        // unknown-field message is — a sixth field cannot be silently omitted.
        let key = parse_key("registry.acme").unwrap();
        let get_msg = get_value(&key, &fresh_options(), &unfiltered_registries())
            .expect_err("a bare alias is not a readable key")
            .to_string();
        let mut options = fresh_options();
        let mut registries = unfiltered_registries();
        let set_msg = apply_set(&key, "x", &mut options, &mut registries)
            .expect_err("a bare alias is not a writable key")
            .to_string();
        for field in RegistryField::ALL {
            let name = field.field_name();
            assert!(get_msg.contains(name), "the `get` hint must list {name}: {get_msg}");
            assert!(set_msg.contains(name), "the `set` hint must list {name}: {set_msg}");
        }
    }

    #[test]
    fn filter_set_and_unset_on_a_missing_alias_are_usage_errors() {
        // Mirrors the `oci` / `index` arms: the alias is part of the key, so
        // a missing one is exit 64, never a silent no-op.
        let mut options = fresh_options();
        let mut registries: Vec<RegistryConfig> = vec![];
        let key = parse_key("registry.ghost.include").unwrap();
        let err = apply_set(&key, "acme/**", &mut options, &mut registries).expect_err("no such registry");
        assert_eq!(crate::error::classify_error(&err), ExitCode::UsageError);
        let err = apply_unset(&key, &mut options, &mut registries).expect_err("no such registry");
        assert_eq!(crate::error::classify_error(&err), ExitCode::UsageError);
    }

    #[test]
    fn filter_unset_clears_only_its_own_side() {
        // Plan C-012: `unset` clears to empty, and the other side survives.
        let mut options = fresh_options();
        let mut registries = filtered_registries();
        apply_unset(
            &parse_key("registry.acme.include").unwrap(),
            &mut options,
            &mut registries,
        )
        .expect("unset must succeed");
        assert!(registries[0].include.is_empty(), "unset must clear the list");
        assert_eq!(
            registries[0].exclude,
            vec!["acme/platform/legacy/**".to_string()],
            "unsetting one side must not disturb the other"
        );
        assert_eq!(
            get_value(&parse_key("registry.acme.include").unwrap(), &options, &registries).unwrap(),
            None,
            "a cleared list reads back as unset"
        );
    }

    #[test]
    fn collect_entries_all_covers_every_registry_field() {
        // Plan C-021: `list --all` builds its registry rows by iterating
        // `RegistryField::ALL` rather than naming fields one at a time, so a
        // field added to that array cannot be silently omitted. This test is
        // written against `ALL` for the same reason.
        let options = fresh_options();
        let registries = unfiltered_registries();
        let keys: Vec<String> = collect_entries(true, &options, &registries)
            .into_iter()
            .map(|e| e.key)
            .collect();
        for f in RegistryField::ALL {
            let expected = format!("registry.acme.{}", f.field_name());
            assert!(
                keys.contains(&expected),
                "--all must list every registry field; missing {expected}; got: {keys:?}"
            );
        }
    }

    #[test]
    fn collect_entries_filter_rows_match_get_value_exactly() {
        // Plan C-021 / S-020: `list` and `get` can never disagree about one
        // registry field — both render through the same accessor.
        let options = fresh_options();
        let registries = filtered_registries();
        let rows = collect_entries(true, &options, &registries);
        for f in RegistryField::ALL {
            let key = format!("registry.acme.{}", f.field_name());
            let row = rows
                .iter()
                .find(|e| e.key == key)
                .unwrap_or_else(|| panic!("--all must emit {key}"));
            let got = get_value(&parse_key(&key).unwrap(), &options, &registries).unwrap();
            assert_eq!(row.value, got, "list and get must agree on {key}");
        }
        // And the filter rows really carry the patterns (not just agree on
        // `None`), including without `--all`.
        let plain_rows = collect_entries(false, &options, &registries);
        let include_row = plain_rows
            .iter()
            .find(|e| e.key == "registry.acme.include")
            .expect("a set include list must be listed without --all");
        assert_eq!(include_row.value.as_deref(), Some("acme/platform/**,acme/tools/**"));
        assert!(include_row.set);
    }

    #[test]
    fn collect_entries_omits_unset_filter_rows_without_all() {
        // An empty list is unset, so it renders exactly like every other
        // unset key: absent without `--all`, null-valued with it.
        let options = fresh_options();
        let registries = unfiltered_registries();
        assert!(
            !collect_entries(false, &options, &registries)
                .iter()
                .any(|e| e.key.ends_with(".include") || e.key.ends_with(".exclude")),
            "an unfiltered registry emits no filter rows without --all"
        );
        let row = collect_entries(true, &options, &registries)
            .into_iter()
            .find(|e| e.key == "registry.acme.exclude")
            .expect("--all must surface the unset row");
        assert_eq!(row.value, None);
        assert!(!row.set);
    }

    #[test]
    fn registry_add_filter_flags_are_repeatable_and_never_comma_split() {
        // Plan C-013 / S-008: repeatable flags that accumulate; a comma in a
        // value is glob alternation and must survive verbatim — this is the
        // one CLI path that writes a multi-pattern list.
        let a = parse(&[
            "registry",
            "add",
            "acme",
            "--index",
            "https://index.acme.internal",
            "--include",
            "acme/platform/**",
            "--include",
            "acme/{tools,labs}/**",
            "--exclude",
            "acme/platform/legacy/**",
        ])
        .expect("repeated filter flags must parse");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Add { include, exclude, .. } => {
                    assert_eq!(
                        include,
                        vec!["acme/platform/**".to_string(), "acme/{tools,labs}/**".to_string()],
                        "repeated --include must accumulate, and the comma must not split"
                    );
                    assert_eq!(exclude, vec!["acme/platform/legacy/**".to_string()]);
                }
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Registry"),
        }
    }

    #[test]
    fn registry_add_without_filter_flags_declares_an_unfiltered_entry() {
        let a = parse(&["registry", "add", "acme", "--oci", "ghcr.io/acme"]).expect("parses");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Add { include, exclude, .. } => {
                    assert!(include.is_empty() && exclude.is_empty());
                }
                _ => panic!("expected Add"),
            },
            _ => panic!("expected Registry"),
        }
    }

    /// A project scope on disk: an empty `grimoire.toml` plus a hermetic
    /// `$GRIM_HOME`, addressed through `--config` so no walk-up escapes the
    /// temp dir.
    fn project_scope() -> (tempfile::TempDir, std::path::PathBuf, Context) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("grimoire.toml");
        std::fs::write(&config_path, "[skills]\n\n[rules]\n").expect("write config");
        let grim_home = tmp.path().join("grim-home");
        std::fs::create_dir_all(&grim_home).expect("grim home");
        let ctx = Context::hermetic_scoped(grim_home, false, Some(config_path.clone()));
        (tmp, config_path, ctx)
    }

    #[test]
    fn registry_add_writes_both_pattern_lists_to_disk() {
        // Plan C-013 / S-008 end to end: the flags reach the entry, the
        // emitter writes them, and the config parses back with each list on
        // its own side — a swap of the two arguments fails here.
        let (_tmp, config_path, ctx) = project_scope();
        let include = vec!["acme/platform/**".to_string(), "acme/tools/**".to_string()];
        let exclude = vec!["acme/platform/legacy/**".to_string()];
        run_registry_add(
            &ctx,
            "acme",
            None,
            Some("https://index.acme.internal"),
            false,
            &include,
            &exclude,
        )
        .expect("registry add with filters must succeed");

        let written = std::fs::read_to_string(&config_path).expect("config written");
        assert!(
            written.contains(r#"include = ["acme/platform/**", "acme/tools/**"]"#),
            "both include patterns must reach the file; got:\n{written}"
        );
        let scope = scope_resolution::resolve(&ctx, false, Some(&config_path)).expect("re-parse");
        let rc = find_registry(&scope.registries, "acme").expect("entry declared");
        assert_eq!(rc.include, include);
        assert_eq!(rc.exclude, exclude);
    }

    #[test]
    fn registry_add_rejects_an_invalid_pattern_before_writing() {
        // Plan C-013 / S-016: the same exit-65 gate as `config set`, and the
        // config file must be untouched — validation runs before the lock.
        let (_tmp, config_path, ctx) = project_scope();
        let before = std::fs::read_to_string(&config_path).expect("config readable");
        let err = run_registry_add(
            &ctx,
            "acme",
            Some("ghcr.io/acme"),
            None,
            false,
            &["acme{unclosed".to_string()],
            &[],
        )
        // `ConfigReport` is not `Debug`; drop the Ok payload so `expect_err`
        // has a printable `T`.
        .map(|_| ())
        .expect_err("an invalid pattern must be rejected");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("config readable"),
            before,
            "nothing may be written when a pattern is rejected"
        );
    }

    #[test]
    fn registry_add_on_an_existing_alias_is_a_usage_error_whatever_the_pattern() {
        // S9: C-013 promises 64 for a duplicate alias unconditionally, so the
        // alias check must run before the pattern loop. Both directions are
        // pinned — a well-formed pattern and a malformed one — because only
        // the malformed one exposed the ordering.
        for pattern in ["acme/platform/**", "acme{unclosed"] {
            let (_tmp, config_path, ctx) = project_scope();
            run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &[], &[])
                .map(|_| ())
                .expect("the first add must succeed");
            let before = std::fs::read_to_string(&config_path).expect("config readable");
            let err = run_registry_add(
                &ctx,
                "acme",
                Some("ghcr.io/acme"),
                None,
                false,
                &[pattern.to_string()],
                &[],
            )
            .map(|_| ())
            .expect_err("a duplicate alias must be refused");
            assert_eq!(
                crate::error::classify_error(&err),
                ExitCode::UsageError,
                "the duplicate alias must win over the pattern check for {pattern:?}"
            );
            assert_eq!(
                std::fs::read_to_string(&config_path).expect("config readable"),
                before,
                "a refused add must write nothing"
            );
        }
    }

    #[test]
    fn duplicate_alias_message_names_the_browse_filter_path() {
        // B-2: the message named only `oci` and `rm`, so a user adding a
        // second `--include` to an existing entry read it as "use `config
        // set` instead" — and that path replaces the whole list. The verb
        // that rebuilds a multi-pattern filter has to be here, because this
        // is where the user lands. It now names `registry set`, which takes
        // the very flags the refused invocation already carried.
        let (_tmp, _config_path, ctx) = project_scope();
        run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &[], &[])
            .map(|_| ())
            .expect("the first add must succeed");
        let msg = run_registry_add(
            &ctx,
            "acme",
            Some("ghcr.io/acme"),
            None,
            false,
            &["tools/**".to_string()],
            &[],
        )
        .map(|_| ())
        .expect_err("a duplicate alias must be refused")
        .to_string();
        assert!(
            msg.contains("filter"),
            "the message must name the browse-filter path: {msg}"
        );
        assert!(
            msg.contains("--include"),
            "repeated flags are the only sequence that writes a multi-pattern list: {msg}"
        );
        assert!(
            msg.contains("registry set"),
            "the in-place edit verb is where the user has to be sent: {msg}"
        );
    }

    /// Three aliased entries, none of them declaring `default` — the shape
    /// where position is load-bearing, since `resolve_registries` falls back
    /// to "first entry wins".
    fn three_entries(ctx: &Context) {
        for alias in ["first", "second", "third"] {
            run_registry_add(ctx, alias, Some(&format!("ghcr.io/{alias}")), None, false, &[], &[])
                .map(|_| ())
                .expect("seed add must succeed");
        }
    }

    #[test]
    fn registry_set_keeps_the_entry_where_it_was() {
        // The reason this verb exists. The old remedy was `rm` + re-`add`,
        // but `add` pushes, so editing the FIRST of three would land it last
        // — and with no entry declaring `default`, "first entry wins" would
        // hand the default to a registry the user never named. Editing in
        // place cannot, so the assertion is on the index, not the value.
        let (_tmp, config_path, ctx) = project_scope();
        three_entries(&ctx);
        run_registry_set(
            &ctx,
            "first",
            None,
            None,
            false,
            &["platform/**".to_string()],
            false,
            &[],
            false,
        )
        .map(|_| ())
        .expect("editing the first entry must succeed");

        let scope = scope_resolution::resolve(&ctx, false, Some(&config_path)).expect("re-parse");
        let order: Vec<_> = scope.registries.iter().filter_map(|r| r.alias.as_deref()).collect();
        assert_eq!(
            order,
            vec!["first", "second", "third"],
            "an in-place edit must not reorder `[[registries]]`"
        );
        assert_eq!(scope.registries[0].include, vec!["platform/**".to_string()]);
    }

    #[test]
    fn registry_set_leaves_every_field_it_was_not_given() {
        // Patch semantics: the locator, the default flag and the untouched
        // filter side all survive an edit that names only `include`.
        let (_tmp, config_path, ctx) = project_scope();
        run_registry_add(
            &ctx,
            "acme",
            Some("ghcr.io/acme"),
            None,
            true,
            &["old/**".to_string()],
            &["legacy/**".to_string()],
        )
        .map(|_| ())
        .expect("seed add must succeed");

        run_registry_set(
            &ctx,
            "acme",
            None,
            None,
            false,
            &["platform/**".to_string(), "{tools,libs}/**".to_string()],
            false,
            &[],
            false,
        )
        .map(|_| ())
        .expect("a filter-only edit must succeed");

        let scope = scope_resolution::resolve(&ctx, false, Some(&config_path)).expect("re-parse");
        let rc = find_registry(&scope.registries, "acme").expect("entry survives");
        assert_eq!(
            rc.include,
            vec!["platform/**".to_string(), "{tools,libs}/**".to_string()],
            "the whole include list is replaced, and the comma must not split"
        );
        assert_eq!(rc.exclude, vec!["legacy/**".to_string()], "the other side is untouched");
        assert_eq!(rc.oci.as_deref(), Some("ghcr.io/acme"), "the locator is untouched");
        assert!(rc.default, "an absent --default must not clear the flag");
    }

    #[test]
    fn registry_set_swaps_the_locator_kind() {
        // `config set registry.<a>.index` refuses this and tells the user to
        // unset `oci` first. Here the flag is the whole declaration, so the
        // other side is cleared rather than refused — an entry carrying both
        // would fail `validate_registries` on the way out.
        let (_tmp, config_path, ctx) = project_scope();
        run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &[], &[])
            .map(|_| ())
            .expect("seed add must succeed");
        run_registry_set(
            &ctx,
            "acme",
            None,
            Some("https://index.acme.internal"),
            false,
            &[],
            false,
            &[],
            false,
        )
        .map(|_| ())
        .expect("swapping to an index must succeed");

        let scope = scope_resolution::resolve(&ctx, false, Some(&config_path)).expect("re-parse");
        let rc = find_registry(&scope.registries, "acme").expect("entry survives");
        assert_eq!(rc.index.as_deref(), Some("https://index.acme.internal"));
        assert!(rc.oci.is_none(), "the previous locator kind must be cleared");
    }

    #[test]
    fn registry_set_default_clears_every_other_entry() {
        // Same invariant `registry use` holds: exactly one default.
        let (_tmp, config_path, ctx) = project_scope();
        three_entries(&ctx);
        run_registry_set(&ctx, "second", None, None, true, &[], false, &[], false)
            .map(|_| ())
            .expect("promoting an entry must succeed");
        run_registry_set(&ctx, "third", None, None, true, &[], false, &[], false)
            .map(|_| ())
            .expect("promoting another entry must succeed");

        let scope = scope_resolution::resolve(&ctx, false, Some(&config_path)).expect("re-parse");
        let defaults: Vec<_> = scope
            .registries
            .iter()
            .filter(|r| r.default)
            .filter_map(|r| r.alias.as_deref())
            .collect();
        assert_eq!(defaults, vec!["third"], "exactly one entry may carry the flag");
    }

    #[test]
    fn registry_set_on_a_missing_alias_is_a_usage_error_whatever_the_pattern() {
        // The mirror of `add`'s duplicate-alias ordering: the alias check
        // runs before the pattern loop, so a malformed pattern cannot turn a
        // 64 into a 65. Both directions pinned, as there.
        for pattern in ["platform/**", "acme{unclosed"] {
            let (_tmp, config_path, ctx) = project_scope();
            let before = std::fs::read_to_string(&config_path).expect("config readable");
            let err = run_registry_set(
                &ctx,
                "ghost",
                None,
                None,
                false,
                &[pattern.to_string()],
                false,
                &[],
                false,
            )
            .map(|_| ())
            .expect_err("a missing alias must be refused");
            assert_eq!(
                crate::error::classify_error(&err),
                ExitCode::UsageError,
                "the missing alias must win over the pattern check for {pattern:?}"
            );
            assert_eq!(
                std::fs::read_to_string(&config_path).expect("config readable"),
                before,
                "a refused set must write nothing"
            );
        }
    }

    #[test]
    fn registry_set_rejects_an_invalid_pattern_before_writing() {
        // The same exit-65 gate `add` and `config set` share, and the file
        // must be untouched — validation runs before the lock.
        let (_tmp, config_path, ctx) = project_scope();
        run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &[], &[])
            .map(|_| ())
            .expect("seed add must succeed");
        let before = std::fs::read_to_string(&config_path).expect("config readable");
        let err = run_registry_set(
            &ctx,
            "acme",
            None,
            None,
            false,
            &["acme{unclosed".to_string()],
            false,
            &[],
            false,
        )
        .map(|_| ())
        .expect_err("an invalid pattern must be rejected");
        assert_eq!(crate::error::classify_error(&err), ExitCode::DataError);
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("config readable"),
            before,
            "nothing may be written when a pattern is rejected"
        );
    }

    #[test]
    fn registry_set_naming_no_field_is_a_usage_error() {
        // A no-op `set` would take the lock, rewrite the file and report a
        // change that never happened. Clap cannot express "at least one of
        // these", so the check lives in the handler — and the message has to
        // name `unset`, since "give me an empty list" is the one edit the
        // patch semantics deliberately cannot express.
        let (_tmp, config_path, ctx) = project_scope();
        run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &[], &[])
            .map(|_| ())
            .expect("seed add must succeed");
        let before = std::fs::read_to_string(&config_path).expect("config readable");
        let err = run_registry_set(&ctx, "acme", None, None, false, &[], false, &[], false)
            .map(|_| ())
            .expect_err("a set naming no field must be refused");
        assert_eq!(crate::error::classify_error(&err), ExitCode::UsageError);
        assert!(
            err.to_string().contains("grim config unset"),
            "clearing a filter is the adjacent intent, so the message must route it: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("config readable"),
            before,
            "a refused set must write nothing"
        );
    }

    #[test]
    fn registry_set_reports_the_locator_the_call_named() {
        // `value` tracks *naming*, not *changing*: it is `locator.map(..)`,
        // derived from the flags with no read of the stored entry. A
        // filter-only edit has nothing honest to put in a single-valued field,
        // so it reports null — but a locator flag echoes back even when it
        // names the value the entry already held. The third case below is the
        // one that pins that, and the docs say the same.
        let (_tmp, _config_path, ctx) = project_scope();
        run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &[], &[])
            .map(|_| ())
            .expect("seed add must succeed");

        let (report, code) = run_registry_set(
            &ctx,
            "acme",
            None,
            None,
            false,
            &["a/**".to_string()],
            false,
            &[],
            false,
        )
        .expect("a filter-only edit must succeed");
        assert_eq!(code, ExitCode::Success);
        match report {
            ConfigReport::Write(w) => {
                assert!(matches!(w.action, WriteAction::RegistrySet));
                assert_eq!(w.key, "registry.acme");
                assert_eq!(w.value, None, "a filter-only edit reports no value");
                assert!(!w.dry_run);
            }
            _ => panic!("expected a write report"),
        }

        let (report, _) = run_registry_set(&ctx, "acme", Some("ghcr.io/other"), None, false, &[], false, &[], false)
            .expect("a locator edit must succeed");
        match report {
            ConfigReport::Write(w) => assert_eq!(w.value.as_deref(), Some("ghcr.io/other")),
            _ => panic!("expected a write report"),
        }

        // Re-naming the locator the entry already holds. No field changed, and
        // the report still carries it: `value` answers "what did the call
        // name", never "what moved".
        let (report, _) = run_registry_set(&ctx, "acme", Some("ghcr.io/other"), None, false, &[], false, &[], false)
            .expect("re-naming the current locator must succeed");
        match report {
            ConfigReport::Write(w) => assert_eq!(
                w.value.as_deref(),
                Some("ghcr.io/other"),
                "value echoes the flag; it is not a before/after diff"
            ),
            _ => panic!("expected a write report"),
        }
    }

    #[test]
    fn registry_set_parses_repeated_filter_flags() {
        // The clap half of the contract, mirroring `registry add`'s: repeated
        // flags accumulate and a comma inside a value is glob alternation,
        // never a separator.
        let a = parse(&[
            "registry",
            "set",
            "acme",
            "--include",
            "acme/platform/**",
            "--include",
            "acme/{tools,labs}/**",
            "--exclude",
            "acme/platform/legacy/**",
        ])
        .expect("repeated filter flags must parse");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Set {
                    alias,
                    oci,
                    index,
                    include,
                    exclude,
                    clear_include,
                    clear_exclude,
                    default,
                } => {
                    assert_eq!(alias, "acme");
                    assert!(oci.is_none() && index.is_none() && !default);
                    assert!(!clear_include && !clear_exclude);
                    assert_eq!(
                        include,
                        vec!["acme/platform/**".to_string(), "acme/{tools,labs}/**".to_string()],
                        "repeated --include must accumulate, and the comma must not split"
                    );
                    assert_eq!(exclude, vec!["acme/platform/legacy/**".to_string()]);
                }
                _ => panic!("expected Set"),
            },
            _ => panic!("expected Registry"),
        }
    }

    #[test]
    fn registry_set_rejects_both_locator_flags() {
        // `conflicts_with`, same as `add` — an entry may carry only one.
        assert!(
            parse(&[
                "registry",
                "set",
                "acme",
                "--oci",
                "ghcr.io/acme",
                "--index",
                "https://x.test"
            ])
            .is_err(),
            "--oci and --index are mutually exclusive"
        );
    }

    /// The rendered `grim config registry add` help text.
    fn registry_add_help() -> String {
        use clap::CommandFactory as _;
        let mut root = Harness::command();
        root.find_subcommand_mut("config")
            .and_then(|c| c.find_subcommand_mut("registry"))
            .and_then(|c| c.find_subcommand_mut("add"))
            .expect("`config registry add` is a subcommand")
            .render_help()
            .to_string()
    }

    #[test]
    fn registry_add_help_states_the_browse_only_boundary() {
        // W13: the platform lead the ADR worries about meets this feature
        // through `--help`, where "narrows what this registry shows" reads
        // just as well as "restricts". Both flags state the boundary.
        // Collapsed first, because clap wraps to the terminal width.
        let help = registry_add_help();
        let collapsed = help.split_whitespace().collect::<Vec<_>>().join(" ");
        let clause = "Affects browsing only — a direct reference to a hidden package still resolves and installs.";
        assert_eq!(
            collapsed.matches(clause).count(),
            2,
            "both --include and --exclude must carry the boundary clause; got:\n{help}"
        );
    }

    #[test]
    fn registry_add_help_states_how_a_pattern_is_anchored() {
        // W-16 / H-2 / design C-012: what a pattern is matched against is the
        // rule users get wrong most, so `--help` has to state it rather than
        // defer to the docs. Both candidates are pinned by a worked example —
        // `registry_filter::qualified_candidate` produces the second one — and
        // the rule is the SAME for an `--oci` and an `--index` entry; the
        // asymmetry that used to exist here is exactly what sent people to a
        // pattern matching nothing.
        let collapsed = registry_add_help().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            collapsed.contains("'acme/tools'"),
            "the help must show a worked bare candidate, not just name the rule:\n{collapsed}"
        );
        assert!(
            collapsed.contains("'ghcr.io/acme/tools'"),
            "the help must show a worked fully-qualified candidate too — naming one \
             candidate is how the superseded single-candidate rule read:\n{collapsed}"
        );
        assert!(
            !collapsed.contains("with the registry host removed"),
            "the superseded single-candidate wording must be gone, not merely \
             supplemented:\n{collapsed}"
        );
        assert!(
            collapsed.contains("--index"),
            "both source kinds must be named, or the reader assumes one differs:\n{collapsed}"
        );
        assert!(
            collapsed.contains("locator never changes what a pattern means"),
            "locator-independence is the property people rely on when editing:\n{collapsed}"
        );
        // The other half of "anchored", and the one a bare name hides: the
        // wildcard-free expansion is a SUFFIX, so `hex` never matches
        // `acme/arcana/hex` — nor the qualified `ghcr.io/acme/arcana/hex`,
        // since both candidates are anchored at their own first segment. A
        // bare name therefore matches nothing at all, which reads as the
        // filter being broken rather than mis-anchored.
        assert!(
            collapsed.contains("'**/hex'"),
            "the help must give the leading-`**/` form for 'match it wherever it sits':\n{collapsed}"
        );
    }

    #[test]
    fn registry_add_help_carries_no_markdown_emphasis() {
        // W15: `--help` is plain text, not markdown. The only `**` left in it
        // is glob syntax, which is never followed by a word character.
        let help = registry_add_help();
        for (i, _) in help.match_indices("**") {
            assert!(
                !help[i + 2..].chars().next().is_some_and(char::is_alphanumeric),
                "`**` at byte {i} reads as markdown emphasis, not glob syntax:\n{help}"
            );
        }
    }

    #[test]
    fn registry_show_and_list_report_each_side_from_its_own_field() {
        // Plan C-014 / S-010: the reports carry the authored patterns, per
        // side. A swap at either producer fails here.
        let (_tmp, _config_path, ctx) = project_scope();
        let include = vec!["acme/platform/**".to_string()];
        let exclude = vec!["acme/legacy/**".to_string()];
        run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &include, &exclude)
            .expect("registry add must succeed");

        let (report, _) = run_registry_show(&ctx, "acme").expect("show must succeed");
        match report {
            ConfigReport::RegistryShow(r) => {
                assert_eq!(r.include, include);
                assert_eq!(r.exclude, exclude);
            }
            _ => panic!("expected RegistryShow"),
        }
        let (report, _) = run_registry_list(&ctx).expect("list must succeed");
        match report {
            ConfigReport::RegistryList(r) => {
                assert_eq!(r.items[0].include, include);
                assert_eq!(r.items[0].exclude, exclude);
            }
            _ => panic!("expected RegistryList"),
        }
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
                ..Default::default()
            },
            RegistryConfig {
                alias: Some("y".to_string()),
                oci: Some("u2".to_string()),
                index: None,
                default: false,
                ..Default::default()
            },
        ];
        // Simulate `set registry.y.default true`.
        clear_all_defaults(&mut registries);
        set_registry_default(&mut registries, "y", true);
        assert_eq!(registries.iter().filter(|r| r.default).count(), 1);
    }

    // ── `registry set --clear-include` / `--clear-exclude` (C-013 … C-021) ──

    /// An `acme` entry carrying BOTH filter lists plus a locator — the fixture
    /// every clear witness below edits one field of. Both sides are populated
    /// on purpose: `run_registry_set` takes two adjacent `bool` parameters
    /// whose positional swap compiles silently, so a witness that seeded only
    /// one side could not tell the two flags apart.
    fn acme_with_both_filters(ctx: &Context) {
        run_registry_add(
            ctx,
            "acme",
            Some("ghcr.io/acme"),
            None,
            false,
            &["a/**".to_string(), "b/**".to_string()],
            &["legacy/**".to_string()],
        )
        .map(|_| ())
        .expect("seed add must succeed");
    }

    /// Re-parse the written config and hand back `acme`'s entry, cloned so the
    /// scope it borrows from can be dropped at the end of the call.
    fn reread_acme(ctx: &Context, config_path: &std::path::Path) -> RegistryConfig {
        let scope = scope_resolution::resolve(ctx, false, Some(config_path)).expect("re-parse");
        find_registry(&scope.registries, "acme")
            .expect("the entry survives every edit")
            .clone()
    }

    #[test]
    fn registry_set_clear_include_conflicts_with_include() {
        // C-013 / C-020 row 4: `--clear-include` and `--include` are the same
        // field from opposite directions, so clap refuses the pair before the
        // handler runs. Deleting `conflicts_with = "include"` turns this red —
        // and would also make C-016's `else if` reachable with both set, which
        // is the shape the contract declares unreachable.
        assert!(
            parse(&["registry", "set", "acme", "--clear-include", "--include", "a/**"]).is_err(),
            "--clear-include and --include are mutually exclusive"
        );
    }

    #[test]
    fn registry_set_clear_exclude_conflicts_with_exclude() {
        // C-013 / C-020 row 4, the exclude half — pinned separately because
        // the two attributes are two independent edits.
        assert!(
            parse(&["registry", "set", "acme", "--clear-exclude", "--exclude", "legacy/**"]).is_err(),
            "--clear-exclude and --exclude are mutually exclusive"
        );
    }

    #[test]
    fn registry_set_clears_one_side_while_writing_the_other() {
        // C-013 / C-016: only the SAME-side pair conflicts. Clearing one list
        // while writing the other is one legal invocation, and it is the shape
        // that proves the two `else if` arms are independent rather than one
        // shared branch — a `conflicts_with` widened to the cross pair would
        // fail here.
        let a = parse(&["registry", "set", "acme", "--clear-include", "--exclude", "legacy/**"])
            .expect("clearing one side while writing the other must parse");
        match a.command {
            ConfigCommand::Registry(r) => match r.command {
                RegistryCommand::Set {
                    include,
                    exclude,
                    clear_include,
                    clear_exclude,
                    ..
                } => {
                    assert!(clear_include, "--clear-include must reach the handler as true");
                    assert!(!clear_exclude, "an absent --clear-exclude stays false");
                    assert!(include.is_empty(), "no --include was given");
                    assert_eq!(exclude, vec!["legacy/**".to_string()]);
                }
                _ => panic!("expected Set"),
            },
            _ => panic!("expected Registry"),
        }
    }

    #[test]
    fn registry_set_with_only_a_clear_flag_is_not_nothing_to_change() {
        // C-015 / S-014 / C-020 row 3: a clear IS a change, so the widened
        // guard must let it through. Deleting `&& !clear_include` from the
        // guard puts this call back on the exit-64 path — the assertion is on
        // the exit code, because the guard returns before any list moves.
        let (_tmp, _config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        let (_report, code) = run_registry_set(&ctx, "acme", None, None, false, &[], true, &[], false)
            .expect("a clear-only edit must be accepted");
        assert_eq!(code, ExitCode::Success);

        let (_tmp, _config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        let (_report, code) = run_registry_set(&ctx, "acme", None, None, false, &[], false, &[], true)
            .expect("a clear-exclude-only edit must be accepted");
        assert_eq!(code, ExitCode::Success);
    }

    #[test]
    fn registry_set_naming_no_field_names_the_clear_flags_too() {
        // C-015 / S-015: the guard widened, so the message it backs must widen
        // with it — a message enumerating five flags for a six-flag guard tells
        // the user the clear route does not exist. The `config unset` sentence
        // stays: owner decision 9 keeps both routes valid.
        let (_tmp, _config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        let msg = run_registry_set(&ctx, "acme", None, None, false, &[], false, &[], false)
            .map(|_| ())
            .expect_err("a set naming no field must still be refused")
            .to_string();
        for flag in [
            "--oci/--index",
            "--include",
            "--exclude",
            "--clear-include",
            "--clear-exclude",
            "--default",
        ] {
            assert!(
                msg.contains(flag),
                "the guard's message must enumerate {flag}, or it contradicts the guard: {msg}"
            );
        }
        assert!(
            msg.contains("grim config unset registry.acme.include"),
            "the second clearing route stays valid and stays named: {msg}"
        );
    }

    #[test]
    fn registry_set_clear_include_empties_only_the_include_list() {
        // C-016 / C-020 row 2 / S-012. Both halves are asserted, never just
        // "something cleared": the targeted list must empty AND the other side
        // must survive, so a swap of the two adjacent `bool` parameters fails
        // here rather than passing silently.
        let (_tmp, config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(&ctx, "acme", None, None, false, &[], true, &[], false)
            .map(|_| ())
            .expect("--clear-include must succeed");

        let rc = reread_acme(&ctx, &config_path);
        assert!(
            rc.include.is_empty(),
            "--clear-include must empty include; got: {:?}",
            rc.include
        );
        assert_eq!(
            rc.exclude,
            vec!["legacy/**".to_string()],
            "the exclude list must survive a --clear-include (a swapped bool pair fails here)"
        );
        assert_eq!(rc.oci.as_deref(), Some("ghcr.io/acme"), "the locator is untouched");
        assert!(!rc.default, "the default flag is untouched");
    }

    #[test]
    fn registry_set_clear_exclude_empties_only_the_exclude_list() {
        // C-016 / C-020 row 2 / S-012, mirrored. The mirror is the assertion
        // that names which flag did the work: without it, `clear_include` and
        // `clear_exclude` are interchangeable at every call site in the suite.
        let (_tmp, config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(&ctx, "acme", None, None, false, &[], false, &[], true)
            .map(|_| ())
            .expect("--clear-exclude must succeed");

        let rc = reread_acme(&ctx, &config_path);
        assert!(
            rc.exclude.is_empty(),
            "--clear-exclude must empty exclude; got: {:?}",
            rc.exclude
        );
        assert_eq!(
            rc.include,
            vec!["a/**".to_string(), "b/**".to_string()],
            "the include list must survive a --clear-exclude (a swapped bool pair fails here)"
        );
        assert_eq!(rc.oci.as_deref(), Some("ghcr.io/acme"), "the locator is untouched");
    }

    #[test]
    fn registry_set_preserves_the_filter_lists_when_editing_any_other_field() {
        // C-020 row 1 / S-018 — the surviving-mutant witness, and the reason
        // the clear flags are designed together with it. Mutating
        // `if !include.is_empty() {` to `{` runs the arm on EVERY edit with an
        // empty slice, silently destroying a committed filter list at exit 0.
        // The mirror form: seed both lists, edit each OTHER field in turn, and
        // assert the untouched lists did not move.
        let include = || vec!["a/**".to_string(), "b/**".to_string()];
        let exclude = || vec!["legacy/**".to_string()];

        // `--default` only — the handover's exact reproduction.
        let (_tmp, config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(&ctx, "acme", None, None, true, &[], false, &[], false)
            .map(|_| ())
            .expect("a --default-only edit must succeed");
        let rc = reread_acme(&ctx, &config_path);
        assert_eq!(rc.include, include(), "a --default-only edit must not touch include");
        assert_eq!(rc.exclude, exclude(), "a --default-only edit must not touch exclude");

        // `--oci` only.
        let (_tmp, config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(&ctx, "acme", Some("ghcr.io/moved"), None, false, &[], false, &[], false)
            .map(|_| ())
            .expect("a locator-only edit must succeed");
        let rc = reread_acme(&ctx, &config_path);
        assert_eq!(rc.include, include(), "a locator edit must not touch include");
        assert_eq!(rc.exclude, exclude(), "a locator edit must not touch exclude");

        // `--exclude` only — the cross-list direction. (The `--include`-only
        // direction is already pinned by
        // `registry_set_leaves_every_field_it_was_not_given`.)
        let (_tmp, config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(
            &ctx,
            "acme",
            None,
            None,
            false,
            &[],
            false,
            &["new/**".to_string()],
            false,
        )
        .map(|_| ())
        .expect("an exclude-only edit must succeed");
        let rc = reread_acme(&ctx, &config_path);
        assert_eq!(rc.include, include(), "writing exclude must not touch include");
        assert_eq!(
            rc.exclude,
            vec!["new/**".to_string()],
            "the named list is replaced wholesale"
        );
    }

    #[test]
    fn registry_set_clear_is_silent_at_every_list_length() {
        // C-017: a clear says what it does in its own flag name, so it warns
        // about nothing — unlike `config set`'s single-pattern replacement,
        // which warns precisely because it destroys a list under a report that
        // reads as an addition (`warn_on_discarded_patterns`). Both lengths are
        // pinned: 1, and the >1 length that makes the other path warn.
        for patterns in [vec!["a/**".to_string()], vec!["a/**".to_string(), "b/**".to_string()]] {
            let (_tmp, _config_path, ctx) = project_scope();
            run_registry_add(&ctx, "acme", Some("ghcr.io/acme"), None, false, &patterns, &[])
                .map(|_| ())
                .expect("seed add must succeed");
            let logs = capture_logs(|| {
                run_registry_set(&ctx, "acme", None, None, false, &[], true, &[], false)
                    .map(|_| ())
                    .expect("--clear-include must succeed");
            });
            assert!(
                !logs.contains("WARN"),
                "a clear of {} pattern(s) must be silent; got:\n{logs}",
                patterns.len()
            );
        }
    }

    #[test]
    fn registry_set_clear_round_trips_as_an_absent_key() {
        // C-018 / S-017: an emptied list is written as NO key at all —
        // byte-identical to an entry that never carried one. The mechanism is
        // `write_config`'s own emptiness guard (`add.rs:978-984`), so a clear
        // needs no write-layer change; this pins the property from the caller's
        // side, where a builder can actually break it.
        let (_tmp, cleared_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(&ctx, "acme", None, None, false, &[], true, &[], false)
            .map(|_| ())
            .expect("--clear-include must succeed");
        let cleared = std::fs::read_to_string(&cleared_path).expect("config written");

        assert!(
            !cleared.contains("include ="),
            "a cleared list must leave no `include =` line; got:\n{cleared}"
        );
        assert!(
            reread_acme(&ctx, &cleared_path).include.is_empty(),
            "and the file must re-parse to an empty include list"
        );

        // The same entry, never filtered on the include side.
        let (_tmp, never_path, never_ctx) = project_scope();
        run_registry_add(
            &never_ctx,
            "acme",
            Some("ghcr.io/acme"),
            None,
            false,
            &[],
            &["legacy/**".to_string()],
        )
        .map(|_| ())
        .expect("seed add must succeed");
        assert_eq!(
            cleared,
            std::fs::read_to_string(&never_path).expect("config written"),
            "a cleared entry must be byte-identical to one that never had the key"
        );
    }

    #[test]
    fn registry_set_clear_include_is_idempotent() {
        // C-019 / S-016: the second clear is not an error and changes nothing.
        // Idempotence is what makes the flag safe to emit unconditionally from
        // a UI that does not know the current list.
        let (_tmp, config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        run_registry_set(&ctx, "acme", None, None, false, &[], true, &[], false)
            .map(|_| ())
            .expect("the first clear must succeed");
        let after_first = std::fs::read_to_string(&config_path).expect("config readable");

        let (_report, code) = run_registry_set(&ctx, "acme", None, None, false, &[], true, &[], false)
            .expect("a clear of an already-empty list must still exit 0");
        assert_eq!(code, ExitCode::Success);
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("config readable"),
            after_first,
            "a repeated clear must leave the file byte-identical"
        );
        let rc = reread_acme(&ctx, &config_path);
        assert!(rc.include.is_empty());
        assert_eq!(
            rc.exclude,
            vec!["legacy/**".to_string()],
            "and it must still leave the other side alone"
        );
    }

    #[test]
    fn registry_set_clear_reports_a_cleared_field_carrying_no_value_key() {
        // C-021 assertion 3. The discriminator is an explicit `"cleared"`
        // action, never a bare `null`: `ConfigWriteReport.value`'s own null
        // already means "not applicable to this verb", and a third nested
        // meaning would overload the token. So the assertion is on key
        // ABSENCE — `value == null` would pass a `null`-encoded design and
        // is exactly what this must reject.
        let (_tmp, _config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        let (report, _code) = run_registry_set(&ctx, "acme", None, None, false, &[], false, &[], true)
            .expect("--clear-exclude must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        let fields = serde_json::to_value(&w.fields).expect("fields serialize");
        assert_eq!(
            fields,
            serde_json::json!([{ "field": "exclude", "action": "cleared" }]),
            "a clear-only edit reports exactly one element, for the side it cleared"
        );
        let element = fields[0].as_object().expect("each element is an object");
        assert!(
            !element.contains_key("value"),
            "a `cleared` element must carry NO `value` key — not `value: null`; got: {element:?}"
        );
    }

    #[test]
    fn registry_set_reports_one_element_per_touched_field_in_all_order() {
        // C-021 assertion 4, and design E-6's trap. `RegistryField::ALL`'s
        // order is `Oci, Index, Default, Include, Exclude`, while the enum's
        // own DECLARATION order is `Oci, Index, Include, Exclude, Default` —
        // they differ, and the VS Code extension indexes `ALL` positionally.
        // The fixture touches `default` AND both filter lists precisely so the
        // two orders are distinguishable.
        let (_tmp, _config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        let (report, _code) = run_registry_set(
            &ctx,
            "acme",
            Some("ghcr.io/moved"),
            None,
            true,
            &["a/**".to_string(), "b/**".to_string()],
            false,
            &[],
            true,
        )
        .expect("a multi-field edit must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        let fields = serde_json::to_value(&w.fields).expect("fields serialize");

        // `index` was never named, so it must not appear at all.
        let expected_order: Vec<&str> = RegistryField::ALL
            .into_iter()
            .map(RegistryField::field_name)
            .filter(|name| *name != "index")
            .collect();
        let emitted: Vec<&str> = fields
            .as_array()
            .expect("fields is an array")
            .iter()
            .map(|e| e["field"].as_str().expect("each element names its field"))
            .collect();
        assert_eq!(
            emitted, expected_order,
            "elements follow `RegistryField::ALL`'s frozen order, not the enum's declaration order"
        );
        assert_ne!(
            emitted,
            vec!["oci", "include", "exclude", "default"],
            "declaration order is the trap E-6 names — iterate ALL, never the enum"
        );

        assert_eq!(
            fields,
            serde_json::json!([
                { "field": "oci",     "action": "set",     "value": "ghcr.io/moved" },
                { "field": "default", "action": "set",     "value": true },
                { "field": "include", "action": "set",     "value": ["a/**", "b/**"] },
                { "field": "exclude", "action": "cleared" },
            ]),
            "each element carries the field's own JSON type; untouched fields emit nothing"
        );
    }

    #[test]
    fn registry_set_kind_swap_reports_both_locator_sides() {
        // Design E-12 §1, the half no other test reaches. `--index` on an OCI
        // entry writes `rc.oci = None` AND `rc.index = Some(..)` in one
        // closure, so the command performs two mutations and must report two —
        // reporting only the named side would hide a write that happened, the
        // exact class of quiet report `subsystem-cli.md`'s "report actual
        // results, not an echo of the input" forbids. `oci` precedes `index`
        // because `RegistryField::ALL` orders them so.
        let (_tmp, _config_path, ctx) = project_scope();
        acme_with_both_filters(&ctx);
        let (report, _code) = run_registry_set(
            &ctx,
            "acme",
            None,
            Some("https://index.example"),
            false,
            &[],
            false,
            &[],
            false,
        )
        .expect("a kind swap must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        assert_eq!(
            serde_json::to_value(&w.fields).expect("fields serialize"),
            serde_json::json!([
                { "field": "oci",   "action": "cleared" },
                { "field": "index", "action": "set", "value": "https://index.example" },
            ]),
            "a kind swap reports the side it cleared as well as the side it set"
        );

        // The entry is index-only now, so re-pointing the index clears
        // nothing: without the `had_oci` guard this would report a phantom
        // `oci cleared` on an entry that has not declared an `oci` at all.
        let (report, _code) = run_registry_set(
            &ctx,
            "acme",
            None,
            Some("https://index.other"),
            false,
            &[],
            false,
            &[],
            false,
        )
        .expect("re-pointing the index must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        assert_eq!(
            serde_json::to_value(&w.fields).expect("fields serialize"),
            serde_json::json!([{ "field": "index", "action": "set", "value": "https://index.other" }]),
            "an absent `oci` is not `cleared` — nothing was there to clear"
        );

        // The mirror: the entry now carries an `index`, so swapping back
        // clears that side and reports it. `oci` still precedes `index` — the
        // order is `ALL`'s, never "named field first".
        let (report, _code) = run_registry_set(&ctx, "acme", Some("ghcr.io/back"), None, false, &[], false, &[], false)
            .expect("swapping back must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        assert_eq!(
            serde_json::to_value(&w.fields).expect("fields serialize"),
            serde_json::json!([
                { "field": "oci",   "action": "set", "value": "ghcr.io/back" },
                { "field": "index", "action": "cleared" },
            ]),
            "the mirror swap reports both sides too, still in `RegistryField::ALL` order"
        );

        // And the guard's own witness: an entry that never declared an `index`
        // has nothing to clear, so a plain `--oci` edit reports ONE element.
        // Dropping the `had_index` guard would add a phantom `index cleared`
        // row to every locator edit grim has ever written.
        let (_tmp, _config_path, fresh) = project_scope();
        acme_with_both_filters(&fresh);
        let (report, _code) = run_registry_set(
            &fresh,
            "acme",
            Some("ghcr.io/moved"),
            None,
            false,
            &[],
            false,
            &[],
            false,
        )
        .expect("a locator edit must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        assert_eq!(
            serde_json::to_value(&w.fields).expect("fields serialize"),
            serde_json::json!([{ "field": "oci", "action": "set", "value": "ghcr.io/moved" }]),
            "an absent side is not `cleared` — nothing was there to clear"
        );

        // Design E-12 §2, the clause with no witness until here: element
        // presence means "this field was written", never "this field changed".
        // The entry now carries `ghcr.io/moved`; naming that same locator
        // again still emits its element, because the code assigns
        // unconditionally and `fields` describes the assignment performed and
        // the resulting state, not a before/after diff. Making the report
        // diff-aware would need a pre-read the command does not do.
        let (report, _code) = run_registry_set(
            &fresh,
            "acme",
            Some("ghcr.io/moved"),
            None,
            false,
            &[],
            false,
            &[],
            false,
        )
        .expect("re-naming the same locator must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        assert_eq!(
            serde_json::to_value(&w.fields).expect("fields serialize"),
            serde_json::json!([{ "field": "oci", "action": "set", "value": "ghcr.io/moved" }]),
            "a locator re-named with the value it already held still emits its element"
        );

        // The same clause on the flag side. `--default` on an entry that is
        // already the default writes `true` over `true` and reports it.
        run_registry_set(&fresh, "acme", None, None, true, &[], false, &[], false)
            .map(|_| ())
            .expect("promoting the entry must succeed");
        let (report, _code) = run_registry_set(&fresh, "acme", None, None, true, &[], false, &[], false)
            .expect("re-promoting the default entry must succeed");
        let ConfigReport::Write(w) = report else {
            panic!("expected a write report");
        };
        assert_eq!(
            serde_json::to_value(&w.fields).expect("fields serialize"),
            serde_json::json!([{ "field": "default", "action": "set", "value": true }]),
            "an already-default entry re-flagged still emits its element"
        );
    }
}
