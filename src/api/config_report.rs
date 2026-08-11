// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! `grim config` output types.
//!
//! Plain format varies by variant:
//! - `Get`: bare value on a single line (no key, no table — script contract).
//! - `Write`: one-row table (`Action | Key | Value | Scope | Dry Run`) — the
//!   shared confirmation for `set`, `unset`, and `registry add`/`rm`/`use`.
//!   `dry_run` is `true` only for `grim config set --dry-run`; every other
//!   write verb (including `unset`, which has no `--dry-run` flag) reports
//!   `false`.
//! - `List`: one table per invocation (`Key | Value`); unset rows (shown
//!   only with `--all`) render an empty `Value` cell.
//! - `RegistryList`: one table (`Alias | Type | Source | Default |
//!   Filters`); `Filters` is browse-filter counts (`N include, M exclude`,
//!   `—` when unfiltered), never the patterns.
//! - `RegistryShow`: one-row table, same columns.
//! - `RegistryFields`: one table (`Key | Type | Title | Description`) —
//!   static per-registry-field metadata, no scope/file read involved.
//!
//! JSON format:
//! - `Get`: `{"key":"…","value":"…"|null,"set":bool,"scope":"…"}`.
//! - `Write`: single object matching struct fields, incl. always-present
//!   `dry_run` and `fields` (`[]` on every write verb except `registry
//!   set`, which carries one row per field it touched).
//! - `List`: `{"items": [...]}` of [`ConfigEntry`] objects — every item
//!   always carries the full metadata shape `{"key","value","set","type",
//!   "title","description","default","values","constraints"}`
//!   (always-present-null policy), whether or not `--all` was passed; the
//!   flag only widens the row set (adding supported-but-unset keys), never
//!   the row shape. `constraints` is non-null only for keys whose list
//!   items carry a shape rule beyond closed-set membership (today:
//!   `options.tui.tree_separators`) — see [`ValueConstraints`].
//! - `RegistryList`: `{"items": [...]}` of `{"alias","oci","index",
//!   "include","exclude","default"}` objects — the pattern lists are
//!   always present, `[]` when unfiltered.
//! - `RegistryShow`: single object matching struct fields.
//! - `RegistryFields`: `{"items": [...]}` of `{"key","type","title",
//!   "description"}` objects — `key` is the short field name (`"oci"`),
//!   deliberately diverging from `ConfigEntry`'s dotted keys; no
//!   `value`/`set`/`default` (meaningless for a field pattern).

use std::fmt;
use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::cli::printer::Printable;

/// The top-level dispatch type returned by `grim config` and rendered by
/// the `app.rs` dispatch arm.  Each variant corresponds to one config
/// subcommand group.
pub enum ConfigReport {
    /// Result of `grim config get`.
    Get(ConfigGetReport),
    /// Result of any write — `set`, `unset`, `registry add`/`rm`/`use`.
    Write(ConfigWriteReport),
    /// Result of `grim config list`.
    List(ConfigListReport),
    /// Result of `grim config registry list`.
    RegistryList(RegistryListReport),
    /// Result of `grim config registry show`.
    RegistryShow(RegistryShowReport),
    /// Result of `grim config registry fields`.
    RegistryFields(RegistryFieldsReport),
}

impl Printable for ConfigReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Get(r) => r.print_plain(w),
            Self::Write(r) => r.print_plain(w),
            Self::List(r) => r.print_plain(w),
            Self::RegistryList(r) => r.print_plain(w),
            Self::RegistryShow(r) => r.print_plain(w),
            Self::RegistryFields(r) => r.print_plain(w),
        }
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Get(r) => r.print_json(w),
            Self::Write(r) => r.print_json(w),
            Self::List(r) => r.print_json(w),
            Self::RegistryList(r) => r.print_json(w),
            Self::RegistryShow(r) => r.print_json(w),
            Self::RegistryFields(r) => r.print_json(w),
        }
    }
}

/// Result of `grim config get <key>`.
///
/// Plain format: bare value on a single line (no key, no table).  `None`
/// means the key is present in the schema but has no value; the command
/// exits with `Failure(1)` and emits no output.
///
/// JSON format: `{"key":"…","value":"…","set":true,"scope":"…"}` when set,
/// or `{"key":"…","value":null,"set":false,"scope":"…"}` when unset.
/// The `set` field enables script-friendly boolean checks without testing
/// `value` for null.
#[derive(Debug)]
pub struct ConfigGetReport {
    /// The dotted key that was queried.
    pub key: String,
    /// The string value, or `None` when the key is unset.
    pub value: Option<String>,
    /// Which scope the value was read from.
    pub scope: Origin,
}

impl Serialize for ConfigGetReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("key", &self.key)?;
        map.serialize_entry("value", &self.value)?;
        map.serialize_entry("set", &self.value.is_some())?;
        map.serialize_entry("scope", &self.scope)?;
        map.end()
    }
}

impl Printable for ConfigGetReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        if let Some(value) = &self.value {
            writeln!(w, "{value}")
        } else {
            Ok(())
        }
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// The kind of write a [`ConfigWriteReport`] confirms.
///
/// Typed column rather than a raw string per `subsystem-cli-api.md`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteAction {
    /// `grim config set <key> <value>`.
    Set,
    /// `grim config unset <key>` (or `registry rm` is reported separately).
    Unset,
    /// `grim config registry add <alias>`.
    RegistryAdded,
    /// `grim config registry rm <alias>`.
    RegistryRemoved,
    /// `grim config registry use <alias>` (made default).
    RegistryDefault,
    /// `grim config registry set <alias>` (edited in place).
    RegistrySet,
}

impl fmt::Display for WriteAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Set => "set",
            Self::Unset => "unset",
            Self::RegistryAdded => "registry-added",
            Self::RegistryRemoved => "registry-removed",
            Self::RegistryDefault => "registry-default",
            Self::RegistrySet => "registry-set",
        })
    }
}

/// Confirmation for any config write: `set`, `unset`, and the registry
/// lifecycle verbs (`add`, `rm`, `use`, `set`).
///
/// Plain format: one-row table — `Action | Key | Value | Scope | Dry Run`.
///
/// JSON format: `{"action": "…", "key": "…", "value": "…"|null, "scope": "…",
/// "dry_run": bool, "fields": […]}`.
#[derive(Debug, Serialize)]
pub struct ConfigWriteReport {
    /// What kind of write this confirms.
    pub action: WriteAction,
    /// The dotted key or `registry.<alias>` affected.
    pub key: String,
    /// The new value (e.g. the URL for `registry add`), or `None` for
    /// `unset` / `rm` / `use`.
    pub value: Option<String>,
    /// Which scope was written.
    pub scope: Origin,
    /// `true` for `grim config set --dry-run` (validated, nothing written);
    /// always `false` for every other write verb, which has no dry-run
    /// surface.
    pub dry_run: bool,
    /// One row per registry field `registry set` touched, in
    /// `RegistryField::ALL` order. Always present — `[]` for every write
    /// verb other than `registry set` (`subsystem-cli-api.md` bans
    /// `skip_serializing_if` in `src/api/`, so this never vanishes).
    ///
    /// **Scoped to the entry named by [`Self::key`], never to the whole
    /// file.** `--default` also demotes whichever sibling entry held the flag
    /// (`clear_all_defaults`, the same invariant `registry use` keeps), and
    /// that sibling's change is not reported here — no element names another
    /// alias. A consumer reading `fields` as "everything this write did to
    /// `grimoire.toml`" would miss it.
    pub fields: Vec<RegistryFieldChange>,
}

impl Printable for ConfigWriteReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        use crate::cli::printer::print_table;
        // `value` is single-valued, so a `registry set` that moved several
        // fields has no one value to name and printed a blank cell. The cell
        // summarises `fields` instead — keyed on `fields` being non-empty
        // rather than on the action, because `fields` is `[]` on the other
        // five verbs (design E-7), so one expression covers all six and their
        // cells stay byte-identical without a verb-sniffing branch.
        //
        // Both user-derived cells are `escape_debug`d — `key`, which
        // interpolates the entry's alias, and the value, whether it comes from
        // `self.value` or from a summarised locator. `char::is_control` (the
        // screen `validate_registries` puts an alias through) is FALSE for the
        // bidi and zero-width format characters, and the locator is never
        // control-screened at all, so an unescaped cell puts a real U+202E or
        // ESC byte on the one line a user reads to confirm what grim just
        // wrote. `validate_registries` relies on display-time escaping here
        // rather than narrowing its input; every sibling registry renderer
        // (`registry list`, `registry show`, `config list`, `grim context`)
        // already makes the same call.
        let value_str = if self.fields.is_empty() {
            self.value.as_deref().unwrap_or_default().escape_debug().to_string()
        } else {
            self.fields
                .iter()
                .map(RegistryFieldChange::summary)
                .collect::<Vec<_>>()
                .join(", ")
        };
        print_table(
            w,
            &["Action", "Key", "Value", "Scope", "Dry Run"],
            &[vec![
                self.action.to_string(),
                self.key.escape_debug().to_string(),
                value_str,
                self.scope.to_string(),
                self.dry_run.to_string(),
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// One field `registry set` touched, reported in
/// [`ConfigWriteReport::fields`].
///
/// JSON: `{"field": "…", "action": "set", "value": …}` for a field that was
/// set, or `{"field": "…", "action": "cleared"}` for one that was cleared —
/// deliberately **no** `value` key on a `"cleared"` row (not `value: null`).
/// `ConfigWriteReport.value`'s own `null` already means "not applicable to
/// this write verb"; a `"cleared"` row encodes an *event*, not a *state*, so
/// it gets its own explicit discriminator rather than overloading `null` a
/// second way (design C-021).
///
/// **The event is "the clear was applied", not "something was removed", and
/// the two sides of `fields` differ on that.** A list row
/// (`include`/`exclude`) emits `cleared` whenever `--clear-include` /
/// `--clear-exclude` was given, including on an already-empty list where the
/// file comes out byte-unchanged; it is deliberately not conditioned on prior
/// content, because guarding it would make `registry set acme
/// --clear-include` on an unfiltered entry report `fields: []` and bring back
/// the blank `Value` cell design E-12 §4 says is no longer reachable. A
/// locator row (`oci`/`index`) *is* so conditioned — the unnamed side of a
/// kind swap emits `cleared` only when it actually held a value, since
/// reporting a locator cleared where none existed would be a phantom write
/// (design E-12 §1).
#[derive(Debug, Serialize)]
pub struct RegistryFieldChange {
    /// The short field name — one of [`RegistryField::field_name`]'s
    /// values (`"oci"`, `"index"`, `"default"`, `"include"`, `"exclude"`).
    ///
    /// [`RegistryField::field_name`]: crate::command::config_keys::RegistryField::field_name
    pub field: &'static str,
    /// Whether the field was set or cleared, and the new value when set.
    /// Flattened so `action` (and any `value`) sit alongside `field` in the
    /// row rather than nested under it.
    #[serde(flatten)]
    pub action: RegistryFieldChangeAction,
}

impl RegistryFieldChange {
    /// This row as one segment of [`ConfigWriteReport`]'s plain `Value` cell,
    /// joined with `", "` in `fields` order.
    ///
    /// The grammar is normative (design E-12 §4): `{field}={value}` for a
    /// locator, `default={bool}`, `{field}={count}` for a list — the count,
    /// never the patterns, matching the `Filters` cell `registry list` and
    /// `show` already print — and `{field} cleared` for a clear, so a cleared
    /// field reads as the event it is rather than as an empty value.
    ///
    /// The locator is the one segment carrying config-file text, so it is
    /// `escape_debug`d here — `field` is `&'static str` and the count and flag
    /// are machine-formatted. Keeping the list side a count is what keeps
    /// authored pattern text out of this renderer entirely.
    fn summary(&self) -> String {
        let field = self.field;
        match &self.action {
            RegistryFieldChangeAction::Cleared => format!("{field} cleared"),
            RegistryFieldChangeAction::Set { value } => match value {
                RegistryFieldValue::String(locator) => format!("{field}={}", locator.escape_debug()),
                RegistryFieldValue::List(patterns) => format!("{field}={}", patterns.len()),
                RegistryFieldValue::Bool(flag) => format!("{field}={flag}"),
            },
        }
    }
}

/// Discriminates a [`RegistryFieldChange`] row.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum RegistryFieldChangeAction {
    /// The field was set to a new value.
    Set {
        /// The new value, in the field's own JSON type.
        value: RegistryFieldValue,
    },
    /// The field was cleared (`--clear-include` / `--clear-exclude`).
    /// Serializes with no sibling `value` key at all.
    Cleared,
}

/// The JSON type a [`RegistryFieldChangeAction::Set`] value takes, matching
/// the touched field's own shape: a string for `oci`/`index`, an array of
/// strings for `include`/`exclude`, a boolean for `default`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RegistryFieldValue {
    /// `oci` / `index`.
    String(String),
    /// `include` / `exclude`.
    List(Vec<String>),
    /// `default`.
    Bool(bool),
}

/// Result of `grim config list`.
///
/// Plain format: one table — `Key | Value`. The list reads from exactly one
/// scope per invocation, so an Origin column would be constant-valued and
/// is omitted.
///
/// JSON format: `{"items": [...]}` of `{"key":"…","value":"…"}` objects —
/// uniform `items` envelope per `subsystem-cli-api.md`.
#[derive(Debug, Serialize)]
pub struct ConfigListReport {
    /// All effective key=value pairs for the scope.
    pub items: Vec<ConfigEntry>,
}

impl Printable for ConfigListReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        use crate::cli::printer::print_table;
        // Both cells are `escape_debug`d because both carry config-file text.
        // The value is authored (a browse-filter glob, a locator), and the key
        // interpolates the entry's alias, so `registry.<alias>.include` puts
        // two attacker-controlled strings on one line. `char::is_control` — the
        // screen browse patterns and aliases pass — is **false** for the bidi
        // and zero-width format characters (U+202E, U+200B), so those reach
        // stdout intact, and a `grimoire.toml` is found by silent walk-up from
        // cwd: this is `git clone && grim config list`. Same call the sibling
        // diagnostics make (`catalog_service.rs`, `registry_resolve.rs`).
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.key.escape_debug().to_string(),
                    e.value.as_deref().unwrap_or("").escape_debug().to_string(),
                ]
            })
            .collect();
        print_table(w, &["Key", "Value"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// The declared type of a config key's value.
///
/// Presentation metadata ONLY — describes how a key's value is shown and
/// documented (`grim config list` JSON `type` field). It never dispatches
/// validation: parsing and rejecting a `set` value stays the job of each
/// key's own setter in `command::config` (`parse_bool`, `parse_u32`,
/// `parse_default_view`, `parse_tree_separators`), which is free to apply
/// rules this enum knows nothing about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    /// A single string value.
    String {
        /// The runtime default, `None` when there is no fixed default.
        default: Option<&'static str>,
    },
    /// `"true"` or `"false"`.
    Bool {
        /// The runtime default.
        default: bool,
    },
    /// A non-negative integer.
    U32 {
        /// The runtime default.
        default: u32,
    },
    /// A closed set of string values, listed in [`Self::values`].
    Enum {
        /// The allowed values.
        values: &'static [&'static str],
        /// The runtime default.
        default: &'static str,
    },
    /// A comma-joined list of strings. Ordered, open — values need not come
    /// from a closed set. Contrast [`Self::StringSet`].
    StringList {
        /// The runtime default, `None` when there is no fixed default.
        default: Option<&'static [&'static str]>,
    },
    /// A comma-joined set of unique strings, each drawn from the closed
    /// [`Self::values`] list. Unordered, closed — contrast [`Self::StringList`]
    /// (ordered, open).
    StringSet {
        /// The allowed values.
        values: &'static [&'static str],
        /// The runtime default, `None` when there is no fixed default.
        default: Option<&'static [&'static str]>,
    },
}

impl ValueType {
    /// The allowed values for an [`Self::Enum`] or [`Self::StringSet`] key,
    /// `None` for every other variant.
    pub fn values(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Enum { values, .. } | Self::StringSet { values, .. } => Some(values),
            Self::String { .. } | Self::Bool { .. } | Self::U32 { .. } | Self::StringList { .. } => None,
        }
    }

    /// The runtime default in CLI string form, `None` when there is no
    /// fixed default.
    pub fn default_str(self) -> Option<String> {
        match self {
            Self::String { default } => default.map(String::from),
            Self::Bool { default } => Some(default.to_string()),
            Self::U32 { default } => Some(default.to_string()),
            Self::Enum { default, .. } => Some(default.to_string()),
            Self::StringList { default } | Self::StringSet { default, .. } => default.map(|values| values.join(",")),
        }
    }

    /// The stable JSON/plain identifier for this type.
    fn as_str(self) -> &'static str {
        match self {
            Self::String { .. } => "string",
            Self::Bool { .. } => "boolean",
            Self::U32 { .. } => "integer",
            Self::Enum { .. } => "enum",
            Self::StringList { .. } => "string-list",
            Self::StringSet { .. } => "string-set",
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ValueType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Machine-readable pre-check constraints on the individual items of a
/// list-valued config key (e.g. `options.tui.tree_separators`).
///
/// Advisory only: `item_pattern` is **necessary, NOT sufficient** — some
/// item-shape rules (e.g. Unicode display width) cannot be expressed as a
/// regex. `grim`'s own validation (the predicate behind the key's setter)
/// is authoritative; a value that matches `item_pattern` can still be
/// rejected at `grim config set` time. Present only on keys whose items
/// carry a shape rule beyond membership in a closed set — contrast
/// `options.clients`, whose closed set is already machine-readable via
/// [`ConfigEntry::values`] and carries no `constraints`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ValueConstraints {
    /// Advisory regex a single list item should match. Necessary, not
    /// sufficient — see the type-level doc.
    pub item_pattern: &'static str,
    /// The required display width (`unicode_width`) of a single item, for
    /// the rule `item_pattern` cannot express. `grim`'s validation is
    /// authoritative.
    pub item_width: u32,
}

/// One key=value line from `grim config list`.
///
/// All nine fields are always present — even the optional ones — per the
/// always-present-null policy (`subsystem-cli-api.md`): `--all` widens
/// which keys appear as rows, never which fields a row's JSON object
/// carries.
#[derive(Debug, Serialize)]
pub struct ConfigEntry {
    /// The dotted key, e.g. `options.tui.default_view`.
    pub key: String,
    /// The string representation of the value, `None` when unset (only
    /// emitted as a row under `--all`).
    pub value: Option<String>,
    /// `true` when [`Self::value`] is `Some`.
    pub set: bool,
    /// The key's declared value type.
    #[serde(rename = "type")]
    pub value_type: ValueType,
    /// Short human title, e.g. `"Default view"`.
    pub title: &'static str,
    /// One to three sentences describing the key's effect — style and
    /// content governed by `subsystem-config-keys.md`.
    pub description: &'static str,
    /// The runtime default in CLI string form, `None` when there is no
    /// fixed default.
    pub default: Option<String>,
    /// The allowed values for an enum key, `None` otherwise.
    pub values: Option<&'static [&'static str]>,
    /// Machine-readable pre-check constraints on individual list items,
    /// `None` for a scalar key or a list key with no item-shape rule
    /// beyond membership in [`Self::values`]. See [`ValueConstraints`] for
    /// the advisory-not-authoritative honesty contract.
    pub constraints: Option<ValueConstraints>,
}

impl ConfigEntry {
    /// Build an entry, deriving [`Self::set`] from `value` and
    /// [`Self::values`] / [`Self::default`] from `value_type`.
    pub fn new(
        key: String,
        value: Option<String>,
        value_type: ValueType,
        title: &'static str,
        description: &'static str,
        constraints: Option<ValueConstraints>,
    ) -> Self {
        let set = value.is_some();
        let values = value_type.values();
        let default = value_type.default_str();
        Self {
            key,
            value,
            set,
            value_type,
            title,
            description,
            default,
            values,
            constraints,
        }
    }
}

/// The scope a config value originated from.
///
/// Used as a typed column in `grim config list --show-origin` — never a
/// raw string per `subsystem-cli-api.md` typed-enum rule.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// From the project `grimoire.toml`.
    Project,
    /// From `$GRIM_HOME/grimoire.toml`.
    Global,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Project => "project",
            Self::Global => "global",
        })
    }
}

/// Result of `grim config registry list`.
///
/// Plain format: one table — `Alias | Type | Source | Default | Filters`.
/// `Filters` carries browse-filter counts, never the patterns.
///
/// JSON format: `{"items": [...]}` of
/// `{"alias":"…"|null,"oci":"…"|null,"index":"…"|null,"include":[…],
/// "exclude":[…],"default":bool}` objects — uniform `items` envelope per
/// `subsystem-cli-api.md`.
#[derive(Debug, Serialize)]
pub struct RegistryListReport {
    /// All registries declared in the scope's `[[registries]]`.
    pub items: Vec<RegistryRow>,
}

impl Printable for RegistryListReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        use crate::cli::printer::print_table;
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|r| {
                let (ty, source) = type_and_source(r.oci.as_deref(), r.index.as_deref());
                vec![
                    // Escaped for the same reason the locator is: the alias
                    // clears `char::is_control`, which does not cover U+202E.
                    r.alias.as_deref().unwrap_or("").escape_debug().to_string(),
                    ty.to_string(),
                    source,
                    r.default.to_string(),
                    filter_cell(&r.include, &r.exclude),
                ]
            })
            .collect();
        print_table(w, &["Alias", "Type", "Source", "Default", "Filters"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// The `Filters` cell for a registry row: browse-filter **counts**, in the
/// same spelling `grim context`'s plain row uses, or `—` when the entry is
/// unfiltered. Counts, never the patterns — a glob list has no width bound
/// and `--format json` is where the patterns are read.
fn filter_cell(include: &[String], exclude: &[String]) -> String {
    if include.is_empty() && exclude.is_empty() {
        "—".to_string()
    } else {
        format!("{} include, {} exclude", include.len(), exclude.len())
    }
}

/// The `Type | Source` cell pair for a registry/index entry: which kind of
/// browse source it is and its locator, **`escape_debug`d**. Empty pair only
/// for an invalid entry that validation would reject.
///
/// The locator is the one `[[registries]]` field that is never
/// control-screened — `validate_registries` screens the *alias* for
/// `char::is_control` and never the `oci`/`index` value — so a TOML
/// `` escape puts a real ESC byte in it and arbitrary ANSI on stdout
/// from a `grimoire.toml` found by silent walk-up. Escaping here rather than
/// at the two call sites is deliberate: `registry list` and `registry show`
/// both route through this, so neither can be fixed and the other forgotten.
fn type_and_source(oci: Option<&str>, index: Option<&str>) -> (&'static str, String) {
    match (oci, index) {
        (Some(oci), _) => ("registry", oci.escape_debug().to_string()),
        (None, Some(index)) => ("index", index.escape_debug().to_string()),
        (None, None) => ("", String::new()),
    }
}

/// One row in `grim config registry list`.
///
/// Both `oci` and `index` keys are always present; exactly one is
/// non-null for a valid entry (always-present-null policy,
/// `subsystem-cli-api.md`).
#[derive(Debug, Serialize)]
pub struct RegistryRow {
    /// The registry alias, or `None` for alias-less (locator-only) entries.
    pub alias: Option<String>,
    /// The plain OCI registry ref (`null` for index entries).
    pub oci: Option<String>,
    /// The package-index locator (`null` for registry entries).
    pub index: Option<String>,
    /// The authored browse-`include` glob patterns; `[]` when unfiltered.
    pub include: Vec<String>,
    /// The authored browse-`exclude` glob patterns; `[]` when unfiltered.
    pub exclude: Vec<String>,
    /// Whether this is the default registry.
    pub default: bool,
}

/// Result of `grim config registry show <alias>`.
///
/// Plain format: one-row table — `Alias | Type | Source | Default |
/// Filters`. The `Filters` cell carries **counts** in `grim context`'s
/// spelling (`N include, M exclude`, `—` when unfiltered); the patterns
/// themselves stay in `--format json`, since a glob list has no width
/// bound (plan C-014/C-020).
///
/// JSON format: `{"alias": "…", "oci": "…"|null, "index": "…"|null,
/// "include": […], "exclude": […], "default": bool}` — both locator keys
/// always present, exactly one non-null; both pattern lists always
/// present, `[]` when unfiltered (always-present policy,
/// `subsystem-cli-api.md`).
#[derive(Debug, Serialize)]
pub struct RegistryShowReport {
    /// The registry alias.
    pub alias: String,
    /// The plain OCI registry ref (`null` for index entries).
    pub oci: Option<String>,
    /// The package-index locator (`null` for registry entries).
    pub index: Option<String>,
    /// The authored browse-`include` glob patterns; `[]` when unfiltered.
    pub include: Vec<String>,
    /// The authored browse-`exclude` glob patterns; `[]` when unfiltered.
    pub exclude: Vec<String>,
    /// Whether this is the default registry.
    pub default: bool,
}

impl Printable for RegistryShowReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        use crate::cli::printer::print_table;
        let (ty, source) = type_and_source(self.oci.as_deref(), self.index.as_deref());
        print_table(
            w,
            &["Alias", "Type", "Source", "Default", "Filters"],
            &[vec![
                self.alias.escape_debug().to_string(),
                ty.to_string(),
                source,
                self.default.to_string(),
                filter_cell(&self.include, &self.exclude),
            ]],
        )
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// Result of `grim config registry fields`.
///
/// Static metadata — the 5 addressable per-registry field names
/// (`oci`, `index`, `default`, `include`, `exclude`) and their
/// type/title/description. No
/// scope, no file read: this describes the field *pattern*
/// (`registry.<alias>.<field>`), not any resolved alias's values, so it
/// works identically inside or outside a project.
///
/// Plain format: one table — `Key | Type | Title | Description`.
///
/// JSON format: `{"items": [...]}` of [`RegistryFieldEntry`] objects —
/// uniform `items` envelope per `subsystem-cli-api.md`.
#[derive(Debug, Serialize)]
pub struct RegistryFieldsReport {
    /// The 5 registry fields, in `RegistryField::ALL` order (`oci, index,
    /// default, include, exclude` — append-only, see that constant).
    pub items: Vec<RegistryFieldEntry>,
}

impl Printable for RegistryFieldsReport {
    fn print_plain(&self, w: &mut impl Write) -> io::Result<()> {
        use crate::cli::printer::print_table;
        let rows: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    e.key.to_string(),
                    e.value_type.to_string(),
                    e.title.to_string(),
                    e.description.to_string(),
                ]
            })
            .collect();
        print_table(w, &["Key", "Type", "Title", "Description"], &rows)
    }

    fn print_json(&self, w: &mut impl Write) -> io::Result<()> {
        crate::cli::printer::write_json_pretty(w, self)
    }
}

/// One row in `grim config registry fields`.
///
/// Deliberately **not** a [`ConfigEntry`]: `value`/`set`/`default` are
/// meaningless for a field pattern (there is no alias to resolve a value
/// against), so this type carries only the field's identity and static
/// metadata. `key` is the SHORT field name (e.g. `"oci"`) — unlike
/// `ConfigEntry::key`, which carries the full dotted key
/// (`registry.<alias>.oci`).
///
/// All four fields are always present (no optional columns to widen).
#[derive(Debug, Serialize)]
pub struct RegistryFieldEntry {
    /// The short field name — one of [`RegistryField::field_name`]'s
    /// values (`"oci"`, `"index"`, `"default"`, `"include"`, `"exclude"`).
    ///
    /// [`RegistryField::field_name`]: crate::command::config_keys::RegistryField::field_name
    pub key: &'static str,
    /// The field's declared value type.
    #[serde(rename = "type")]
    pub value_type: ValueType,
    /// Short human title, e.g. `"OCI registry ref"`.
    pub title: &'static str,
    /// One-sentence description of the field.
    pub description: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::config_keys::RegistryField;
    use crate::install::client_target::ClientTarget;

    #[test]
    fn origin_display_matches_serde_rename() {
        assert_eq!(Origin::Project.to_string(), "project");
        assert_eq!(Origin::Global.to_string(), "global");
    }

    #[test]
    fn config_get_report_serializes_with_value() {
        let r = ConfigGetReport {
            key: "options.clients".to_string(),
            value: Some("claude".to_string()),
            scope: Origin::Project,
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(v["key"], "options.clients");
        assert_eq!(v["value"], "claude");
        assert_eq!(v["set"], true);
        assert_eq!(v["scope"], "project");
    }

    #[test]
    fn config_get_report_serializes_none_as_null_with_set_false() {
        let r = ConfigGetReport {
            key: "options.clients".to_string(),
            value: None,
            scope: Origin::Global,
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(v["value"].is_null());
        assert_eq!(v["set"], false);
        assert_eq!(v["scope"], "global");
    }

    #[test]
    fn config_get_report_plain_prints_bare_value_when_set() {
        // ADR: plain `get` emits bare value — no key, no table — so that
        // `$(grim config get options.clients)` works in scripts.
        let r = ConfigGetReport {
            key: "options.clients".to_string(),
            value: Some("claude,opencode".to_string()),
            scope: Origin::Project,
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("claude,opencode"),
            "plain get must emit the bare value; got: {out:?}"
        );
        // Must NOT echo the key — callers rely on value-only stdout.
        assert!(
            !out.contains("options.clients"),
            "plain get must not echo the key; got: {out:?}"
        );
    }

    #[test]
    fn config_get_report_plain_emits_nothing_when_unset() {
        // ADR: `get` of an unset key exits Failure(1) with no stdout.
        // The Printable impl must not write anything to `w` when value is None.
        let r = ConfigGetReport {
            key: "options.clients".to_string(),
            value: None,
            scope: Origin::Project,
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        assert!(
            buf.is_empty(),
            "plain get of unset key must write nothing; got: {buf:?}"
        );
    }

    #[test]
    fn config_write_report_json_carries_action_key_value_scope() {
        // ADR: ConfigWriteReport JSON shape {"action":"…","key":"…","value":"…","scope":"…","dry_run":bool}.
        let r = ConfigWriteReport {
            action: WriteAction::Set,
            key: "options.clients".to_string(),
            value: Some("claude".to_string()),
            scope: Origin::Project,
            dry_run: false,
            fields: Vec::new(),
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v["action"].is_string(), "action must be a string; got: {v}");
        assert_eq!(v["key"], "options.clients", "key field must match");
        assert_eq!(v["scope"], "project", "scope must be 'project'");
        let val = v["value"].as_str().unwrap_or("");
        assert!(val.contains("claude"), "value field must contain 'claude'");
        assert_eq!(v["dry_run"], false, "dry_run must be false for a real write");
    }

    #[test]
    fn config_write_report_json_pins_frozen_shape() {
        // Frozen shape: exactly these 6 keys, always present (additive-only
        // JSON contract — a future field must widen this set, never replace
        // it). `fields` (design C-021) is the field that widened it here.
        let r = ConfigWriteReport {
            action: WriteAction::Set,
            key: "options.clients".to_string(),
            value: Some("claude".to_string()),
            scope: Origin::Project,
            dry_run: true,
            fields: Vec::new(),
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        let obj = v.as_object().expect("write report must serialize as an object");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> = ["action", "key", "value", "scope", "dry_run", "fields"]
            .into_iter()
            .collect();
        assert_eq!(
            keys, expected,
            "ConfigWriteReport JSON must pin exactly the frozen shape"
        );
        assert_eq!(v["dry_run"], true);
        assert_eq!(
            v["fields"],
            serde_json::json!([]),
            "fields must be [] for a released verb"
        );
    }

    #[test]
    fn config_write_report_plain_emits_table_with_action_columns() {
        // subsystem-cli-api.md: single-table rule — exactly one print_table call.
        // The table must contain action, key, value, scope, and dry-run data.
        let r = ConfigWriteReport {
            action: WriteAction::Set,
            key: "options.clients".to_string(),
            value: Some("claude".to_string()),
            scope: Origin::Project,
            dry_run: false,
            fields: Vec::new(),
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.is_empty(), "plain write-confirmation must not be empty");
        // All five column values must appear in the output.
        assert!(
            text.contains("options.clients"),
            "key must appear in table; got: {text:?}"
        );
        assert!(text.contains("claude"), "value must appear in table; got: {text:?}");
        assert!(text.contains("project"), "scope must appear in table; got: {text:?}");
        assert!(text.contains("Dry Run"), "Dry Run header must appear; got: {text:?}");
        assert!(text.contains("false"), "dry_run value must appear; got: {text:?}");
    }

    #[test]
    fn config_write_report_plain_dry_run_true_renders_true() {
        let r = ConfigWriteReport {
            action: WriteAction::Set,
            key: "options.clients".to_string(),
            value: Some("claude".to_string()),
            scope: Origin::Project,
            dry_run: true,
            fields: Vec::new(),
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("true"),
            "dry_run true must render as 'true'; got: {text:?}"
        );
    }

    /// The `Value` cell of a one-row write table, read positionally off this
    /// render's own header line so an EMPTY cell stays distinguishable from an
    /// absent one (splitting on the column gap would erase it). Asserts the
    /// Single-Table Rule on the way past: one header row, one data row.
    ///
    /// The offsets are **char** offsets, because `print_table` pads with
    /// `cell.chars().count()` — column starts in the data row are char
    /// positions, not byte positions. `find` hands back a byte offset, which
    /// is the same number only while the header is ASCII, so that is asserted
    /// rather than assumed. Slicing the row by those numbers as a BYTE range
    /// (`row.get(start..end)`) would misread whenever a cell left of `Value`
    /// carries a multi-byte char — and `Key` carries a user-supplied alias,
    /// which `validate_registries` permits to be non-ASCII.
    fn value_cell(text: &str) -> String {
        let mut lines = text.lines();
        let header = lines.next().expect("a header row");
        let row = lines.next().expect("a data row");
        assert!(
            lines.next().is_none(),
            "the Single-Table Rule allows exactly one data row; got: {text:?}"
        );
        assert!(
            header.is_ascii(),
            "`find`'s byte offset is only a char offset while the header is ASCII; got: {header:?}"
        );
        let start = header.find("Value").expect("a Value column");
        let end = header.find("Scope").expect("a Scope column");
        row.chars()
            .skip(start)
            .take(end - start)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn config_write_report_plain_summarises_registry_set_fields_in_the_value_cell() {
        // C-021 assertion 6 (design E-7) — the contract's only behavioural
        // obligation. `value` is single-valued, so a filter-only `registry set`
        // prints a BLANK cell today; the cell instead summarises `fields`.
        // Only `registry set`'s cell may move, and only because that verb is
        // unreleased.
        let r = ConfigWriteReport {
            action: WriteAction::RegistrySet,
            key: "registry.acme".to_string(),
            value: Some("ghcr.io/moved".to_string()),
            scope: Origin::Project,
            dry_run: false,
            fields: vec![
                RegistryFieldChange {
                    field: "oci",
                    action: RegistryFieldChangeAction::Set {
                        value: RegistryFieldValue::String("ghcr.io/moved".to_string()),
                    },
                },
                RegistryFieldChange {
                    field: "default",
                    action: RegistryFieldChangeAction::Set {
                        value: RegistryFieldValue::Bool(true),
                    },
                },
                RegistryFieldChange {
                    field: "include",
                    action: RegistryFieldChangeAction::Set {
                        value: RegistryFieldValue::List(vec!["a/**".to_string(), "b/**".to_string()]),
                    },
                },
                RegistryFieldChange {
                    field: "exclude",
                    action: RegistryFieldChangeAction::Cleared,
                },
            ],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let cell = value_cell(&text);

        // Design E-12 §4's grammar is NORMATIVE, and containment checks alone
        // cannot see it: replayed against five single-token mutants, only
        // "the locator arm drops the field name" went red — an arm printing
        // the include PATTERNS instead of their count, a `{field}=cleared`
        // spelling, a `'; '` separator and an off-by-one count all survived.
        // The pattern-count clause is the one the plain renderer's security
        // property rests on, so the whole cell is pinned byte-for-byte.
        assert_eq!(
            cell, "oci=ghcr.io/moved, default=true, include=2, exclude cleared",
            "the cell grammar is normative: `{{field}}={{value}}` for a locator, \
             `default={{bool}}`, `{{field}}={{count}}` for a list — the count, never \
             the patterns — and `{{field}} cleared` for a clear, joined with `, ` \
             in `fields` order; got: {text:?}"
        );

        assert!(!cell.is_empty(), "a multi-field edit must not print a blank cell");
        for touched in ["oci", "default", "include", "exclude"] {
            assert!(
                cell.contains(touched),
                "the summary must name every field the call touched; {touched} missing from {cell:?}"
            );
        }
        assert!(
            !cell.contains("index"),
            "an untouched field must not appear in the summary; got: {cell:?}"
        );
        assert!(
            cell.contains("cleared"),
            "a cleared field must read as an event, not as a value; got: {cell:?}"
        );
        assert!(
            cell.contains("ghcr.io/moved"),
            "a set locator keeps its value in the summary; got: {cell:?}"
        );
    }

    #[test]
    fn config_write_report_plain_leaves_every_released_verb_s_value_cell_alone() {
        // C-021 assertion 6, the other half. `fields` is `[]` on all five
        // released verbs, so deriving the summary from `!fields.is_empty()`
        // (E-7) leaves their cells byte-identical — a verb-sniffing branch
        // that got the condition wrong would show up right here.
        for (action, value, expected) in [
            (WriteAction::Set, Some("claude".to_string()), "claude"),
            (WriteAction::Unset, None, ""),
            (
                WriteAction::RegistryAdded,
                Some("ghcr.io/acme".to_string()),
                "ghcr.io/acme",
            ),
            (WriteAction::RegistryRemoved, None, ""),
            (WriteAction::RegistryDefault, None, ""),
        ] {
            let r = ConfigWriteReport {
                action,
                key: "registry.acme".to_string(),
                value,
                scope: Origin::Project,
                dry_run: false,
                fields: Vec::new(),
            };
            let mut buf: Vec<u8> = Vec::new();
            r.print_plain(&mut buf).unwrap();
            let text = String::from_utf8(buf).unwrap();
            assert_eq!(
                value_cell(&text),
                expected,
                "{action}'s Value cell must stay exactly as it shipped; got: {text:?}"
            );
        }
    }

    #[test]
    fn config_list_report_plain_shows_key_value_entries() {
        // ADR: list plain format — key=value lines, one table per invocation.
        let r = ConfigListReport {
            items: vec![ConfigEntry::new(
                "options.clients".to_string(),
                Some("claude".to_string()),
                ValueType::StringSet {
                    values: ClientTarget::VALUE_NAMES,
                    default: None,
                },
                "Clients",
                "Determines which clients receive installs and updates when `--client` is absent. \
                 Auto-detects clients when left empty, falling back to all clients when none are detected.",
                None,
            )],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("options.clients"),
            "key must appear in list output; got: {text:?}"
        );
        assert!(
            text.contains("claude"),
            "value must appear in list output; got: {text:?}"
        );
    }

    /// The bidi override `char::is_control` does **not** catch, so it passes
    /// the browse-pattern and alias screens and reaches a terminal intact.
    const BIDI_OVERRIDE: char = '\u{202e}';
    /// A real ESC byte — what a TOML `` escape puts in an unscreened
    /// `oci` locator, and the start of every ANSI sequence.
    const ESC: char = '\u{1b}';

    #[test]
    fn config_list_plain_escapes_both_cells_ws3() {
        // W-S3: `list` grew `registry.<alias>.include`/`.exclude` rows, and
        // BOTH cells are config-file text — the value is authored, and the key
        // interpolates the alias. `char::is_control` is false for U+202E, so
        // the pattern screen passes it through; a `grimoire.toml` is found by
        // silent walk-up, making this `git clone && grim config list`.
        let r = ConfigListReport {
            items: vec![ConfigEntry::new(
                format!("registry.zz{BIDI_OVERRIDE}acme.include"),
                Some(format!("zz{BIDI_OVERRIDE}nothing/**")),
                ValueType::String { default: None },
                "Include",
                "Browse-filter include patterns.",
                None,
            )],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            !text.contains(BIDI_OVERRIDE),
            "no raw bidi override may reach stdout; got: {text:?}"
        );
        assert_eq!(
            text.matches("\\u{202e}").count(),
            2,
            "the key AND the value must both be escaped — the key carries the alias; got: {text:?}"
        );
    }

    #[test]
    fn registry_list_plain_escapes_the_locator_and_alias_ws3() {
        // W-S3's other half. The locator is the one `[[registries]]` field
        // validation never control-screens — it screens the ALIAS for
        // `char::is_control` and never the `oci`/`index` value — so a TOML
        // `` escape puts a real ESC here and arbitrary ANSI on stdout.
        // The alias needs the same call for the U+202E reason above.
        let r = RegistryListReport {
            items: vec![RegistryRow {
                alias: Some(format!("zz{BIDI_OVERRIDE}acme")),
                oci: Some(format!("ghcr.io/{ESC}[2J{ESC}[Hwiped")),
                index: None,
                include: Vec::new(),
                exclude: Vec::new(),
                default: true,
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.contains(ESC), "no raw ESC byte may reach stdout; got: {text:?}");
        assert!(
            !text.contains(BIDI_OVERRIDE),
            "no raw bidi override may reach stdout; got: {text:?}"
        );
    }

    #[test]
    fn registry_show_plain_escapes_the_locator_through_the_shared_seam_ws3() {
        // `show` and `list` share `type_and_source`, which is why the escape
        // lives there — but "shared today" is not "shared tomorrow", and this
        // is the surface a user reaches for one hostile alias by name.
        let r = RegistryShowReport {
            alias: format!("zz{BIDI_OVERRIDE}acme"),
            oci: None,
            index: Some(format!("https://{ESC}[2Jindex.example")),
            include: Vec::new(),
            exclude: Vec::new(),
            default: false,
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.contains(ESC), "no raw ESC byte may reach stdout; got: {text:?}");
        assert!(
            !text.contains(BIDI_OVERRIDE),
            "no raw bidi override may reach stdout; got: {text:?}"
        );
    }

    #[test]
    fn config_write_report_plain_escapes_the_key_and_both_value_sources_ws3() {
        // The write report was the last `grim config` renderer emitting
        // config-file text raw, and it is the line a user reads to confirm what
        // grim just wrote. Three cells carry that text and all three are
        // asserted: `key` (interpolates the alias), `value` (the locator, on a
        // verb whose `fields` is empty), and the locator inside a `fields`
        // summary. `char::is_control` — the only screen an alias passes — is
        // FALSE for U+202E, and the locator is never control-screened at all.
        for report in [
            ConfigWriteReport {
                action: WriteAction::RegistryAdded,
                key: format!("registry.zz{BIDI_OVERRIDE}acme"),
                value: Some(format!("ghcr.io/{ESC}[2J{ESC}[Hwiped")),
                scope: Origin::Project,
                dry_run: false,
                fields: Vec::new(),
            },
            ConfigWriteReport {
                action: WriteAction::RegistrySet,
                key: format!("registry.zz{BIDI_OVERRIDE}acme"),
                value: None,
                scope: Origin::Project,
                dry_run: false,
                fields: vec![RegistryFieldChange {
                    field: "oci",
                    action: RegistryFieldChangeAction::Set {
                        value: RegistryFieldValue::String(format!("ghcr.io/{ESC}[2J{ESC}[Hwiped")),
                    },
                }],
            },
        ] {
            let mut buf: Vec<u8> = Vec::new();
            report.print_plain(&mut buf).unwrap();
            let text = String::from_utf8(buf).unwrap();
            assert!(!text.contains(ESC), "no raw ESC byte may reach stdout; got: {text:?}");
            assert!(
                !text.contains(BIDI_OVERRIDE),
                "no raw bidi override may reach stdout; got: {text:?}"
            );
        }
    }

    #[test]
    fn config_write_report_plain_leaves_a_non_ascii_alias_readable() {
        // The boundary for the escape above, and the witness for `value_cell`'s
        // char-offset contract. `escape_debug` passes printable non-ASCII
        // through, and `validate_registries` permits it in an alias — so the
        // `Key` cell can be multi-byte while `print_table` still pads by CHARS.
        // Reading the row by a byte range would slide the window here.
        let r = ConfigWriteReport {
            action: WriteAction::RegistrySet,
            key: "registry.café".to_string(),
            value: None,
            scope: Origin::Project,
            dry_run: false,
            fields: vec![RegistryFieldChange {
                field: "include",
                action: RegistryFieldChangeAction::Set {
                    value: RegistryFieldValue::List(vec!["a/**".to_string(), "b/**".to_string()]),
                },
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("registry.café"),
            "a printable non-ASCII alias must render verbatim; got: {text:?}"
        );
        assert_eq!(
            value_cell(&text),
            "include=2",
            "the Value cell must stay readable beside a multi-byte Key cell; got: {text:?}"
        );
    }

    #[test]
    fn config_list_plain_leaves_an_ordinary_row_untouched_ws3() {
        // The boundary: `escape_debug` must not mangle what a real config
        // holds. A wildcard-free glob, a brace alternation and a locator all
        // render verbatim — otherwise the fix above would be a regression on
        // every non-hostile config.
        let r = ConfigListReport {
            items: vec![ConfigEntry::new(
                "registry.acme.include".to_string(),
                Some("acme/{platform,tools}/**".to_string()),
                ValueType::String { default: None },
                "Include",
                "Browse-filter include patterns.",
                None,
            )],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("registry.acme.include") && text.contains("acme/{platform,tools}/**"),
            "an ordinary key and value must render byte-identically; got: {text:?}"
        );
    }

    #[test]
    fn config_list_report_json_is_items_envelope() {
        let r = ConfigListReport {
            items: vec![ConfigEntry::new(
                "options.clients".to_string(),
                Some("claude".to_string()),
                ValueType::StringSet {
                    values: ClientTarget::VALUE_NAMES,
                    default: None,
                },
                "Clients",
                "Determines which clients receive installs and updates when `--client` is absent. \
                 Auto-detects clients when left empty, falling back to all clients when none are detected.",
                None,
            )],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object(), "JSON list must be an items envelope; got: {v}");
        assert!(v["items"].is_array());
        assert_eq!(v["items"][0]["key"], "options.clients");
        assert_eq!(v["items"][0]["value"], "claude");
    }

    #[test]
    fn config_entry_json_pins_full_metadata_shape() {
        // Frozen I2 shape: exactly these 9 keys, always present.
        let enum_entry = ConfigEntry::new(
            "options.tui.default_view".to_string(),
            Some("tree".to_string()),
            ValueType::Enum {
                values: &["flat", "tree"],
                default: "tree",
            },
            "Default view",
            "Sets the view the browser opens in. Defaults to `tree`, grouping items by path segments; \
             `flat` lists them ungrouped.",
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&enum_entry).unwrap()).unwrap();
        let obj = v.as_object().expect("entry must serialize as an object");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> = [
            "key",
            "value",
            "set",
            "type",
            "title",
            "description",
            "default",
            "values",
            "constraints",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected, "ConfigEntry JSON must pin exactly the frozen shape");
        assert_eq!(v["type"], "enum");
        assert_eq!(v["values"], serde_json::json!(["flat", "tree"]));
        assert_eq!(v["default"], "tree");
        assert!(
            v["constraints"].is_null(),
            "a non-constrained entry must serialize constraints as explicit null"
        );

        let bool_entry = ConfigEntry::new(
            "options.show_deprecated".to_string(),
            Some("true".to_string()),
            ValueType::Bool { default: false },
            "Show deprecated",
            "Controls whether deprecated artifacts appear in `grim search` and the TUI catalog. \
             Hidden by default unless already installed.",
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&bool_entry).unwrap()).unwrap();
        assert_eq!(v["type"], "boolean");
        assert!(
            v["values"].is_null(),
            "non-enum entry must serialize values as explicit null"
        );
        assert!(v["constraints"].is_null());

        let tree_separators_entry = ConfigEntry::new(
            "options.tui.tree_separators".to_string(),
            Some("/".to_string()),
            ValueType::StringList { default: Some(&["/"]) },
            "Tree separators",
            "Characters on which the repository path is split into nested groups in tree view.",
            Some(ValueConstraints {
                item_pattern: crate::config::project_config::TREE_SEPARATOR_ITEM_PATTERN,
                item_width: 1,
            }),
        );
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&tree_separators_entry).unwrap()).unwrap();
        assert_eq!(
            v["constraints"],
            serde_json::json!({"item_pattern": r"^[^\s\p{C}]$", "item_width": 1}),
            "tree_separators constraints must carry the advisory item_pattern and item_width"
        );
    }

    #[test]
    fn config_entry_unset_serializes_null_value_set_false() {
        let r = ConfigEntry::new(
            "options.default_registry".to_string(),
            None,
            ValueType::String { default: None },
            "Default registry",
            "Default registry for short identifiers.",
            None,
        );
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(v["value"].is_null());
        assert_eq!(v["set"], false);
    }

    #[test]
    fn config_list_plain_renders_unset_row_with_empty_value_cell() {
        let r = ConfigListReport {
            items: vec![ConfigEntry::new(
                "options.default_registry".to_string(),
                None,
                ValueType::String { default: None },
                "Default registry",
                "Registry used when an artifact reference names no registry. Ignored when a \
                 `[[registries]]` entry is declared — the array's default entry expands short \
                 identifiers instead.",
                None,
            )],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        // Exactly one table: the header line plus one data row.
        assert_eq!(text.lines().count(), 2, "must render exactly one table; got: {text:?}");
        assert!(text.contains("options.default_registry"));
        // No "(unset)" sentinel — the cell is simply empty.
        assert!(
            !text.contains("(unset)"),
            "must not use an unset sentinel; got: {text:?}"
        );
    }

    #[test]
    fn value_type_display_matches_serde() {
        assert_eq!(ValueType::String { default: None }.to_string(), "string");
        assert_eq!(ValueType::Bool { default: false }.to_string(), "boolean");
        assert_eq!(ValueType::U32 { default: 0 }.to_string(), "integer");
        assert_eq!(
            ValueType::Enum {
                values: &["flat", "tree"],
                default: "flat"
            }
            .to_string(),
            "enum"
        );
        assert_eq!(ValueType::StringList { default: None }.to_string(), "string-list");
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&ValueType::StringList { default: None }).unwrap()).unwrap();
        assert_eq!(v, "string-list");
    }

    #[test]
    fn string_set_display_serde_values_and_default() {
        let t = ValueType::StringSet {
            values: &["claude", "opencode", "copilot"],
            default: None,
        };
        assert_eq!(t.to_string(), "string-set");
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(v, "string-set");
        assert_eq!(t.values(), Some(&["claude", "opencode", "copilot"][..]));
        assert_eq!(t.default_str(), None);

        let with_default = ValueType::StringSet {
            values: &["claude", "opencode", "copilot"],
            default: Some(&["claude", "opencode"]),
        };
        assert_eq!(with_default.default_str(), Some("claude,opencode".to_string()));
    }

    #[test]
    fn registry_list_report_plain_shows_alias_oci_default() {
        // ADR: registry list — one table (Alias | Type | Source | Default).
        let r = RegistryListReport {
            items: vec![RegistryRow {
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                include: Vec::new(),
                exclude: Vec::new(),
                default: true,
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("acme"), "alias must appear; got: {text:?}");
        assert!(text.contains("ghcr.io/acme"), "URL must appear; got: {text:?}");
    }

    #[test]
    fn registry_list_report_json_is_items_envelope() {
        let r = RegistryListReport {
            items: vec![RegistryRow {
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                include: vec!["acme/platform/**".to_string()],
                exclude: Vec::new(),
                default: false,
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_object(), "registry list JSON must be an items envelope; got: {v}");
        assert!(v["items"].is_array());
        assert_eq!(v["items"][0]["alias"], "acme");
        assert_eq!(v["items"][0]["oci"], "ghcr.io/acme");
        // Always-present-null: the unused locator key is explicit null.
        let index = v["items"][0].get("index").expect("index key must always be present");
        assert!(index.is_null(), "index must be explicit null for an oci row");
    }

    #[test]
    fn registry_show_report_json_keeps_null_locator_key() {
        let r = RegistryShowReport {
            alias: "pub".to_string(),
            oci: None,
            index: Some("https://index.example".to_string()),
            include: Vec::new(),
            exclude: Vec::new(),
            default: false,
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["index"], "https://index.example");
        let oci = v.get("oci").expect("oci key must always be present");
        assert!(oci.is_null(), "oci must be explicit null for an index row");
    }

    #[test]
    fn registry_list_report_json_keeps_empty_pattern_lists_as_arrays() {
        // `src/api/` bans `skip_serializing_if` (subsystem-cli-api.md), and
        // `RegistryRow` is the one shape whose serialization no other test
        // touches: `registry_list_report_json_is_items_envelope` never reads
        // the two keys, and the command-side test asserts on struct fields.
        // `get` + `== []` is the assertion pair that tells an empty array
        // apart from an absent key.
        let r = RegistryListReport {
            items: vec![RegistryRow {
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                include: Vec::new(),
                exclude: Vec::new(),
                default: false,
            }],
        };
        let v = serde_json::to_value(&r).unwrap();
        for side in ["include", "exclude"] {
            let list = v["items"][0].get(side).expect("key must always be present");
            assert_eq!(list, &serde_json::json!([]), "{side} must serialize as []");
        }
    }

    #[test]
    fn registry_show_report_json_carries_both_pattern_lists() {
        // Plan C-014 / S-010: `include`/`exclude` are always present and
        // carry the authored patterns on their own side. Swapping the two
        // fields at the producer fails this assertion.
        let r = RegistryShowReport {
            alias: "acme".to_string(),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            include: vec!["acme/platform/**".to_string(), "acme/tools/**".to_string()],
            exclude: vec!["acme/platform/legacy/**".to_string()],
            default: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["include"], serde_json::json!(["acme/platform/**", "acme/tools/**"]));
        assert_eq!(v["exclude"], serde_json::json!(["acme/platform/legacy/**"]));
    }

    #[test]
    fn registry_show_report_json_keeps_empty_pattern_lists_as_arrays() {
        // Plan C-014 / S-010: `[]`, never an absent key — `src/api/` bans
        // `skip_serializing_if` (subsystem-cli-api.md).
        let r = RegistryShowReport {
            alias: "acme".to_string(),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            include: Vec::new(),
            exclude: Vec::new(),
            default: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        for side in ["include", "exclude"] {
            let list = v.get(side).expect("key must always be present");
            assert_eq!(list, &serde_json::json!([]), "{side} must serialize as []");
        }
    }

    #[test]
    fn registry_list_plain_shows_filter_counts_never_the_patterns() {
        // Owner decision: a filtered registry must be visible in the very
        // command that lists registries — as COUNTS, in `grim context`'s
        // spelling. The patterns themselves must never reach the table (a
        // glob list has no width bound), and an unfiltered row reads `—`.
        let filtered = RegistryListReport {
            items: vec![RegistryRow {
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                include: vec!["acme/platform/**".to_string()],
                exclude: vec!["acme/legacy/**".to_string()],
                default: true,
            }],
        };
        let unfiltered = RegistryListReport {
            items: vec![RegistryRow {
                alias: Some("acme".to_string()),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                include: Vec::new(),
                exclude: Vec::new(),
                default: true,
            }],
        };
        let render = |r: &RegistryListReport| {
            let mut buf: Vec<u8> = Vec::new();
            r.print_plain(&mut buf).unwrap();
            String::from_utf8(buf).unwrap()
        };
        let filtered_out = render(&filtered);
        assert!(
            filtered_out.contains("Filters") && filtered_out.contains("1 include, 1 exclude"),
            "a filtered row must report its counts; got:\n{filtered_out}"
        );
        assert!(
            !filtered_out.contains("acme/platform/**") && !filtered_out.contains("acme/legacy/**"),
            "the patterns themselves must never reach the plain table; got:\n{filtered_out}"
        );
        assert!(
            render(&unfiltered).contains('—'),
            "an unfiltered row must render the empty-cell marker; got:\n{}",
            render(&unfiltered)
        );
        // Still exactly one table in both states (subsystem-cli-api.md).
        assert_eq!(filtered_out.lines().count(), 2);
        assert_eq!(render(&unfiltered).lines().count(), 2);
    }

    #[test]
    fn registry_show_plain_carries_the_same_filter_cell() {
        // One spelling across both renderers — `show` and `list` must not
        // disagree about how a filter looks.
        let render = |include: Vec<String>, exclude: Vec<String>| {
            let r = RegistryShowReport {
                alias: "acme".to_string(),
                oci: Some("ghcr.io/acme".to_string()),
                index: None,
                include,
                exclude,
                default: false,
            };
            let mut buf: Vec<u8> = Vec::new();
            r.print_plain(&mut buf).unwrap();
            String::from_utf8(buf).unwrap()
        };
        let filtered = render(vec!["a/**".to_string(), "b/**".to_string()], vec!["c/**".to_string()]);
        assert!(filtered.contains("2 include, 1 exclude"), "got:\n{filtered}");
        assert!(!filtered.contains("a/**"), "patterns must stay out; got:\n{filtered}");
        assert!(render(Vec::new(), Vec::new()).contains('—'));
    }

    #[test]
    fn registry_fields_report_json_is_items_envelope_with_short_keys() {
        // ADR: `config registry fields` rows are keyed by the SHORT field
        // name (`oci`), not a `registry.<alias>.oci` dotted pattern —
        // there is no alias to interpolate for static metadata.
        let r = RegistryFieldsReport {
            items: RegistryField::ALL
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
                .collect(),
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_json(&mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert!(
            v.is_object(),
            "registry fields JSON must be an items envelope; got: {v}"
        );
        let items = v["items"].as_array().expect("items must be an array");
        // The whole key vector, not a count: `items` is built from
        // `RegistryField::ALL` right above, so `items.len() == ALL.len()`
        // is a tautology that cannot fail. This pins membership, order and
        // count in one line, and a field added to `ALL` fails it honestly.
        let field_keys: Vec<&str> = items.iter().filter_map(|i| i["key"].as_str()).collect();
        assert_eq!(
            field_keys,
            ["oci", "index", "default", "include", "exclude"],
            "must list every registry field in declared order; got: {items:?}"
        );
        assert_eq!(items[0]["key"], "oci", "first field must be 'oci'; got: {items:?}");
        assert_eq!(
            items[0]["type"], "string",
            "oci field type must be 'string'; got: {items:?}"
        );
        // Located by key, not by index — field order is a separate contract
        // (pinned for `oci` above) and adding a field must not break this.
        let default_row = items
            .iter()
            .find(|i| i["key"] == "default")
            .unwrap_or_else(|| panic!("'default' field must be listed; got: {items:?}"));
        assert_eq!(
            default_row["type"], "boolean",
            "default field type must be 'boolean'; got: {items:?}"
        );
        // Deliberately NOT a ConfigEntry shape: no value/set/default fields.
        let obj = items[0].as_object().expect("item must be an object");
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> = ["key", "type", "title", "description"].into_iter().collect();
        assert_eq!(keys, expected, "RegistryFieldEntry JSON must pin exactly this shape");
    }

    #[test]
    fn registry_fields_report_plain_is_one_table() {
        let r = RegistryFieldsReport {
            items: vec![RegistryFieldEntry {
                key: "oci",
                value_type: ValueType::String { default: None },
                title: "OCI registry ref",
                description: "A plain OCI registry ref.",
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text.lines().count(), 2, "must render exactly one table; got: {text:?}");
        assert!(text.contains("oci"), "key must appear; got: {text:?}");
        assert!(text.contains("OCI registry ref"), "title must appear; got: {text:?}");
    }

    #[test]
    fn registry_show_report_plain_is_one_row_table() {
        // ADR: registry show — one-row table (Alias | Type | Source | Default).
        let r = RegistryShowReport {
            alias: "acme".to_string(),
            oci: Some("ghcr.io/acme".to_string()),
            index: None,
            include: Vec::new(),
            exclude: Vec::new(),
            default: false,
        };
        let mut buf: Vec<u8> = Vec::new();
        r.print_plain(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("acme"), "alias must appear; got: {text:?}");
        assert!(text.contains("ghcr.io/acme"), "URL must appear; got: {text:?}");
    }
}
