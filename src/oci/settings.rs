// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The `settings` artifact: harness configuration as a JSON layer.
//!
//! Design: `adr_settings_artifact_kind.md`.

use std::collections::BTreeMap;

use serde_json::{Map, Number, Value, json};

use crate::install::client_target::ClientTarget;
use crate::install::vendor::{MergeClass, SettingsField, SettingsValueType};
use crate::oci::annotations::{RepositoryUrlError, validate_repository_url};
use crate::oci::bundle::BundleMember;
use crate::oci::requires_decl;
use crate::oci::text::{escape_controls, registry_text};

/// The media type of a settings layer.
pub const SETTINGS_LAYER_MEDIA_TYPE: &str = "application/vnd.grimoire.settings.v1+json";

/// The layer size cap, checked on the untrusted descriptor before any download.
pub const SETTINGS_LAYER_SIZE_LIMIT: u64 = 64 * 1024;

/// The rows whose values name a marketplace, so a URL there can carry a credential.
const MARKETPLACE_ROWS: &[&str] = &["/extraKnownMarketplaces/<name>", "/marketplaces/<name>"];

/// What a `codex.rules` entry may decide.
const RULE_DECISIONS: &[&str] = &["allow", "prompt", "forbidden"];

/// The marketplace source Claude's settings take here, and its closed key set.
const CLAUDE_SOURCE_KINDS: &[&str] = &["github"];
const CLAUDE_SOURCE_KEYS: &[&str] = &["source", "repo", "ref"];

/// The Codex key that rules out [`CODEX_SANDBOX_KEYS`], in one document or across packages.
pub(crate) const CODEX_PROFILE_KEY: &str = "default_permissions";
pub(crate) const CODEX_SANDBOX_KEYS: [&str; 2] = ["sandbox_mode", "sandbox_workspace_write"];

/// The harnesses that expand `{env:` and `{file:` wherever they read a string:
/// OpenCode, and Kilo, which keeps OpenCode's config replacer.
const EXPANDING_VENDORS: &[&str] = &[ClientTarget::OpenCode.as_str(), ClientTarget::Kilo.as_str()];

/// The harness that resolves `$NAME`, `${NAME}` and `${NAME:-default}` in
/// every string of its settings file.
const DOLLAR_EXPANDING_VENDOR: &str = ClientTarget::Gemini.as_str();

/// Whether `text` holds a `$` the Gemini resolver reads as a reference: one
/// followed by a word character or a brace.
fn holds_dollar_reference(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'$' && (pair[1] == b'{' || pair[1] == b'_' || pair[1].is_ascii_alphanumeric()))
}

/// Every descriptive top-level key, with the text `grim schema` prints for it.
const DESCRIPTIVE_KEYS: &[(&str, &str)] = &[
    ("description", "What this package configures."),
    ("summary", "Short summary for listings."),
    ("keywords", "Comma-separated keywords."),
    ("license", "SPDX license expression."),
    ("repository", "Source repository, an https URL."),
    ("deprecated", "Deprecation notice."),
    ("replaced-by", "The reference that replaces this package."),
    ("authors", "Comma-separated authors."),
    ("vendor", "Publishing vendor."),
    ("homepage", "Homepage URL."),
    ("documentation", "Documentation URL."),
];

/// Which gate a document is judged by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationMode {
    /// The author's side, at build and publish.
    Author,
    /// The consumer's side, over a fetched layer.
    Consume,
}

/// One settings artifact: descriptive fields plus one table per vendor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsDocument {
    /// One-line description, required as for an MCP artifact.
    pub description: String,
    /// Short summary for listings.
    pub summary: Option<String>,
    /// Comma-separated keywords.
    pub keywords: Option<String>,
    /// SPDX license expression.
    pub license: Option<String>,
    /// Source repository URL.
    pub repository: Option<String>,
    /// Deprecation notice.
    pub deprecated: Option<String>,
    /// The reference that replaces this artifact.
    pub replaced_by: Option<String>,
    /// Comma-separated authors.
    pub authors: Option<String>,
    /// Publishing vendor.
    pub vendor: Option<String>,
    /// Homepage URL.
    pub homepage: Option<String>,
    /// Documentation URL.
    pub documentation: Option<String>,
    /// The settings themselves, keyed by vendor name.
    pub vendors: BTreeMap<String, Map<String, Value>>,
}

impl SettingsDocument {
    /// Parse an authored `settings.toml`, splitting off its `requires` edges.
    ///
    /// # Errors
    ///
    /// Malformed text, or a value [`ValidationMode::Author`] refuses.
    pub fn from_toml_str_with_requires(raw: &str) -> Result<(Self, Vec<BundleMember>), SettingsError> {
        let mut table: toml::Table = toml::from_str(raw).map_err(|error| toml_error(raw, &error))?;

        // Only `requires` is handed over: the grammar also reads `bundles`,
        // which here is a vendor name like any other.
        let mut declared = toml::Table::new();
        if let Some(requires) = table.remove("requires") {
            declared.insert("requires".to_string(), requires);
        }
        // `SettingsError` is `Clone` and `Eq`, which the source error is not;
        // this display names keys only.
        let requires = requires_decl::from_toml_tables(&declared).map_err(|error| SettingsError::Malformed {
            detail: error.to_string(),
        })?;

        let mut fields = Map::new();
        for (key, value) in table {
            let pointer = format!("/{}", encode_segment(&key));
            fields.insert(key, toml_to_json(value, &pointer)?);
        }
        let document = Self::from_top_level(fields)?;
        // Author reports no warnings: an unknown vendor is refused outright.
        document.validate(ValidationMode::Author)?;
        check_pattern_order(raw)?;
        Ok((document, requires))
    }

    /// Serialize the document as its layer: pretty JSON with sorted keys and a
    /// final newline. An unset descriptive field stays out of the layer.
    ///
    /// # Errors
    ///
    /// A document past [`SETTINGS_LAYER_SIZE_LIMIT`].
    pub fn to_layer_bytes(&self) -> Result<Vec<u8>, SettingsError> {
        let mut fields = Map::new();
        fields.insert("description".to_string(), Value::String(self.description.clone()));
        for (key, value) in [
            ("summary", &self.summary),
            ("keywords", &self.keywords),
            ("license", &self.license),
            ("repository", &self.repository),
            ("deprecated", &self.deprecated),
            ("replaced-by", &self.replaced_by),
            ("authors", &self.authors),
            ("vendor", &self.vendor),
            ("homepage", &self.homepage),
            ("documentation", &self.documentation),
        ] {
            if let Some(text) = value {
                fields.insert(key.to_string(), Value::String(text.clone()));
            }
        }
        for (vendor, table) in &self.vendors {
            fields.insert(vendor.clone(), Value::Object(table.clone()));
        }

        // The serializer's text is dropped so no value can reach a message.
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(fields)).map_err(|_| SettingsError::Malformed {
            detail: "the document does not serialize".to_string(),
        })?;
        bytes.push(b'\n');
        check_size(bytes.len() as u64)?;
        Ok(bytes)
    }

    /// Parse and validate the fetched layer of `package` under
    /// [`ValidationMode::Consume`], logging each [`SettingsWarning`] at `warn`
    /// after the package's name.
    ///
    /// # Errors
    ///
    /// A layer past the cap, malformed text, or a refused value.
    pub fn from_layer_bytes(package: &str, bytes: &[u8]) -> Result<Self, SettingsError> {
        check_size(bytes.len() as u64)?;
        // The position alone: the parser's text can quote the document.
        let value: Value = serde_json::from_slice(bytes).map_err(|error| SettingsError::Malformed {
            detail: format!("line {} column {}", error.line(), error.column()),
        })?;
        let Value::Object(fields) = value else {
            return Err(SettingsError::Malformed {
                detail: "a settings layer is a JSON object".to_string(),
            });
        };
        let document = Self::from_top_level(fields)?;
        for warning in document.validate(ValidationMode::Consume)? {
            tracing::warn!("settings package '{}' {warning}", escape_controls(package));
        }
        Ok(document)
    }

    /// Judge the document against each vendor's field table.
    ///
    /// # Errors
    ///
    /// The first value `mode` refuses.
    pub fn validate(&self, mode: ValidationMode) -> Result<Vec<SettingsWarning>, SettingsError> {
        if self.description.trim().is_empty() {
            return Err(SettingsError::Malformed {
                detail: "description must not be empty".to_string(),
            });
        }
        if let Some(repository) = &self.repository {
            check_repository(repository)?;
        }

        let mut warnings = Vec::new();
        for (vendor, table) in &self.vendors {
            if let Some(fields) = known_fields(vendor) {
                let rows: Vec<&'static SettingsField> = fields.iter().collect();
                VendorCheck { vendor, mode }.walk_table(table, &rows, 0, "")?;
                if vendor == ClientTarget::Codex.as_str() {
                    codex_exclusion(table)?;
                }
                continue;
            }
            let shown = registry_text(vendor);
            if mode == ValidationMode::Author {
                return Err(SettingsError::UnknownVendor { vendor: shown });
            }
            let check = VendorCheck { vendor: &shown, mode };
            for (key, value) in table {
                check.scan(value, &format!("/{}", encode_segment(key)))?;
            }
            warnings.push(if is_client(vendor) {
                SettingsWarning::ClientWithoutSettings { vendor: shown }
            } else {
                SettingsWarning::UnknownVendor { vendor: shown }
            });
        }
        Ok(warnings)
    }

    /// Split a document's top level into descriptive fields and vendor tables.
    fn from_top_level(mut fields: Map<String, Value>) -> Result<Self, SettingsError> {
        if fields.contains_key("requires") {
            return Err(SettingsError::Malformed {
                detail: "requires is reserved and never reaches a layer".to_string(),
            });
        }
        let mut document = Self {
            // A missing description reads as blank, which `validate` refuses.
            description: take_text(&mut fields, "description")?.unwrap_or_default(),
            summary: take_text(&mut fields, "summary")?,
            keywords: take_text(&mut fields, "keywords")?,
            license: take_text(&mut fields, "license")?,
            repository: take_text(&mut fields, "repository")?,
            deprecated: take_text(&mut fields, "deprecated")?,
            replaced_by: take_text(&mut fields, "replaced-by")?,
            authors: take_text(&mut fields, "authors")?,
            vendor: take_text(&mut fields, "vendor")?,
            homepage: take_text(&mut fields, "homepage")?,
            documentation: take_text(&mut fields, "documentation")?,
            vendors: BTreeMap::new(),
        };
        for (vendor, value) in fields {
            match value {
                Value::Object(table) => {
                    document.vendors.insert(vendor, table);
                }
                Value::Null => {
                    return Err(SettingsError::NullValue {
                        pointer: format!("/{}", encode_segment(&vendor)),
                    });
                }
                _ => {
                    return Err(SettingsError::WrongType {
                        vendor: registry_text(&vendor),
                        pointer: "/".to_string(),
                        expected: "a table",
                    });
                }
            }
        }
        Ok(document)
    }
}

/// A parse failure by line. `Display` would print the source line, which can
/// hold a token.
fn toml_error(raw: &str, error: &toml::de::Error) -> SettingsError {
    let line = error
        .span()
        .and_then(|span| raw.get(..span.start))
        .map_or(0, |head| head.matches('\n').count())
        + 1;
    SettingsError::Malformed {
        detail: format!("line {line}: {}", error.message()),
    }
}

/// Convert authored TOML, refusing what JSON has no form for. The hand-written
/// pass exists because `flatten` would defeat `deny_unknown_fields`.
fn toml_to_json(value: toml::Value, pointer: &str) -> Result<Value, SettingsError> {
    Ok(match value {
        toml::Value::String(text) => Value::String(text),
        toml::Value::Integer(number) => Value::from(number),
        toml::Value::Boolean(flag) => Value::Bool(flag),
        toml::Value::Float(_) => {
            return Err(SettingsError::FloatValue {
                pointer: pointer.to_string(),
            });
        }
        toml::Value::Datetime(_) => {
            return Err(SettingsError::DatetimeValue {
                pointer: pointer.to_string(),
            });
        }
        toml::Value::Array(items) => Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| toml_to_json(item, &format!("{pointer}/{index}")))
                .collect::<Result<_, _>>()?,
        ),
        toml::Value::Table(table) => {
            let mut fields = Map::new();
            for (key, item) in table {
                let child = format!("{pointer}/{}", encode_segment(&key));
                fields.insert(key, toml_to_json(item, &child)?);
            }
            Value::Object(fields)
        }
    })
}

fn take_text(fields: &mut Map<String, Value>, key: &str) -> Result<Option<String>, SettingsError> {
    match fields.remove(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(Value::Null) => Err(SettingsError::NullValue {
            pointer: format!("/{key}"),
        }),
        Some(_) => Err(SettingsError::Malformed {
            detail: format!("{key} must be a string"),
        }),
    }
}

fn check_size(actual: u64) -> Result<(), SettingsError> {
    if actual > SETTINGS_LAYER_SIZE_LIMIT {
        return Err(SettingsError::TooLarge {
            actual,
            limit: SETTINGS_LAYER_SIZE_LIMIT,
        });
    }
    Ok(())
}

fn check_repository(value: &str) -> Result<(), SettingsError> {
    validate_repository_url(value).map_err(|error| match error {
        RepositoryUrlError::EmbeddedCredentials => SettingsError::CredentialInUrl {
            pointer: "/repository".to_string(),
        },
        // `NotHttps` prints the value, which can hold a token.
        _ => SettingsError::Malformed {
            detail: "repository must be an https URL".to_string(),
        },
    })
}

/// The field table of a vendor this grim writes settings for. A client with
/// no rows has nowhere to put a table, so it reads as unknown.
pub(crate) fn known_fields(vendor: &str) -> Option<&'static [SettingsField]> {
    let client = ClientTarget::ALL.iter().find(|client| client.as_str() == vendor)?;
    let fields = client.vendor().settings_fields();
    (!fields.is_empty()).then_some(fields)
}

/// One key as an RFC 6901 segment, screened first: a key is registry text on
/// its way to a terminal.
pub(crate) fn encode_segment(key: &str) -> String {
    registry_text(key).replace('~', "~0").replace('/', "~1")
}

/// The segment of `row` at `depth`. The bare `rules` row counts like the rest.
pub(crate) fn segment_at(row: &SettingsField, depth: usize) -> Option<&'static str> {
    row.pointer.trim_start_matches('/').split('/').nth(depth)
}

/// Any angle-bracketed segment is a map key, whatever it is called.
fn is_placeholder(segment: &str) -> bool {
    segment.starts_with('<') && segment.ends_with('>')
}

pub(crate) fn row_takes_key(row: &SettingsField, depth: usize, key: &str) -> bool {
    segment_at(row, depth).is_some_and(|segment| {
        if is_placeholder(segment) {
            row.map_keys.is_none_or(|keys| keys.contains(&key))
        } else {
            segment == key
        }
    })
}

/// Visit `value` and everything below it, parents first.
fn visit<F>(value: &Value, pointer: &str, check: &mut F) -> Result<(), SettingsError>
where
    F: FnMut(&Value, &str) -> Result<(), SettingsError>,
{
    check(value, pointer)?;
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                visit(item, &format!("{pointer}/{index}"), check)?;
            }
        }
        Value::Object(table) => {
            for (key, item) in table {
                visit(item, &format!("{pointer}/{}", encode_segment(key)), check)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

/// Every `://` counts, so a URL nested in a query is read too. The URL
/// parser's reading counts as well, so a spelling it normalises (no slashes,
/// backslashes) is read the way a harness would.
fn holds_userinfo(text: &str) -> bool {
    let parsed = reqwest::Url::parse(text).is_ok_and(|url| !url.username().is_empty() || url.password().is_some());
    parsed
        || text
            .match_indices("://")
            .any(|(at, mark)| authority_has_userinfo(&text[at + mark.len()..]))
}

fn authority_has_userinfo(rest: &str) -> bool {
    // git and curl end the authority at these three only: cutting at `\` too
    // passes userinfo they read as a credential (CWE-522).
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // An empty authority is no shield: URL parsers skip the extra slashes.
    authority.contains('@') || (authority.is_empty() && rest.contains('@'))
}

/// A sibling key of the user's own is never compared: only this document's.
fn codex_exclusion(table: &Map<String, Value>) -> Result<(), SettingsError> {
    if !table.contains_key(CODEX_PROFILE_KEY) {
        return Ok(());
    }
    for other in CODEX_SANDBOX_KEYS {
        if table.contains_key(other) {
            return Err(SettingsError::ExclusiveKeys {
                first: format!("/codex/{CODEX_PROFILE_KEY}"),
                second: format!("/codex/{other}"),
            });
        }
    }
    Ok(())
}

/// Refuse an OpenCode pattern table not written in sorted key order.
///
/// The last matching pattern wins and the layer sorts its keys, so sorting
/// silently would change which rule wins. Byte order puts `*` before words
/// and `git *` before `git push *`, so general-then-specific stays
/// expressible. A parsed layer has lost its order, so this gate exists only
/// here: a registry layer reads sorted whatever its bytes said.
fn check_pattern_order(raw: &str) -> Result<(), SettingsError> {
    let document: toml_edit::DocumentMut = raw
        .parse()
        .map_err(|_: toml_edit::TomlError| SettingsError::Malformed {
            detail: "the document does not parse".to_string(),
        })?;
    let Some(permission) = document
        .get("opencode")
        .and_then(|item| item.get("permission"))
        .and_then(toml_edit::Item::as_table_like)
    else {
        return Ok(());
    };
    for (tool, item) in permission.iter() {
        let Some(patterns) = item.as_table_like() else {
            continue;
        };
        let keys: Vec<&str> = patterns.iter().map(|(key, _)| key).collect();
        if !keys.is_sorted() {
            return Err(SettingsError::Malformed {
                detail: format!(
                    "/opencode/permission/{} must list its patterns in sorted order, the layer sorts them",
                    encode_segment(tool)
                ),
            });
        }
    }
    Ok(())
}

/// One vendor table under one gate. Pointers are vendor-relative, as a row
/// spells them.
struct VendorCheck<'a> {
    vendor: &'a str,
    mode: ValidationMode,
}

impl VendorCheck<'_> {
    fn absolute(&self, pointer: &str) -> String {
        format!("/{}{pointer}", encode_segment(self.vendor))
    }

    fn unknown_field(&self, pointer: String) -> SettingsError {
        SettingsError::UnknownField {
            vendor: self.vendor.to_string(),
            pointer,
            mode: self.mode,
        }
    }

    fn wrong_type(&self, pointer: &str, expected: &'static str) -> SettingsError {
        SettingsError::WrongType {
            vendor: self.vendor.to_string(),
            pointer: pointer.to_string(),
            expected,
        }
    }

    fn walk_table(
        &self,
        table: &Map<String, Value>,
        rows: &[&'static SettingsField],
        depth: usize,
        prefix: &str,
    ) -> Result<(), SettingsError> {
        for (key, value) in table {
            let pointer = format!("{prefix}/{}", encode_segment(key));
            self.token(key, &pointer)?;
            let matching: Vec<_> = rows
                .iter()
                .copied()
                .filter(|row| row_takes_key(row, depth, key))
                .collect();
            // Refused before its value is read, so a newer publisher's field
            // keeps the reason that says so whatever it holds.
            if matching.is_empty() {
                return Err(self.unknown_field(pointer));
            }
            self.walk_value(value, &matching, depth + 1, &pointer)?;
        }
        Ok(())
    }

    /// Two rows can share a prefix, as `/permission` and `/permission/<tool>`
    /// do, so the value's shape picks one.
    fn walk_value(
        &self,
        value: &Value,
        rows: &[&'static SettingsField],
        next: usize,
        pointer: &str,
    ) -> Result<(), SettingsError> {
        let leaf = rows.iter().copied().find(|row| segment_at(row, next).is_none());
        let deeper: Vec<_> = rows
            .iter()
            .copied()
            .filter(|row| segment_at(row, next).is_some())
            .collect();
        match (value, leaf) {
            (Value::Object(table), _) if !deeper.is_empty() => self.walk_table(table, &deeper, next, pointer),
            (_, Some(row)) => self.leaf(value, row, pointer),
            _ => {
                self.scan(value, pointer)?;
                Err(self.wrong_type(pointer, "a table"))
            }
        }
    }

    fn leaf(&self, value: &Value, row: &SettingsField, pointer: &str) -> Result<(), SettingsError> {
        self.scan(value, pointer)?;
        // An own-file row's type describes one entry, not the list it heads.
        if row.merge == MergeClass::OwnFile
            && let SettingsValueType::Atomic { keys } = row.ty
        {
            return self.rules(value, keys, pointer);
        }
        self.typed(value, row.ty, pointer)?;
        if MARKETPLACE_ROWS.contains(&row.pointer) {
            return self.marketplace(value, pointer);
        }
        Ok(())
    }

    /// The checks that hold at any depth, atomic values included.
    fn scan(&self, value: &Value, pointer: &str) -> Result<(), SettingsError> {
        visit(value, pointer, &mut |value: &Value, at: &str| match value {
            Value::Null => Err(SettingsError::NullValue {
                pointer: self.absolute(at),
            }),
            Value::Number(number) => self.number(number, at),
            Value::String(text) => self.token(text, at).and_then(|()| self.reference(text, at)),
            Value::Object(table) => table
                .keys()
                .try_for_each(|key| self.token(key, &format!("{at}/{}", encode_segment(key)))),
            Value::Bool(_) | Value::Array(_) => Ok(()),
        })
    }

    fn number(&self, number: &Number, pointer: &str) -> Result<(), SettingsError> {
        if number.is_f64() {
            return Err(SettingsError::FloatValue {
                pointer: self.absolute(pointer),
            });
        }
        if number.as_i64().is_none() {
            return Err(SettingsError::IntegerRange {
                pointer: self.absolute(pointer),
            });
        }
        Ok(())
    }

    fn token(&self, text: &str, pointer: &str) -> Result<(), SettingsError> {
        if EXPANDING_VENDORS.contains(&self.vendor) && (text.contains("{env:") || text.contains("{file:")) {
            return Err(SettingsError::ExpansionToken {
                pointer: self.absolute(pointer),
            });
        }
        Ok(())
    }

    /// Gemini resolves a reference in a string value, never in a key.
    fn reference(&self, text: &str, pointer: &str) -> Result<(), SettingsError> {
        if self.vendor == DOLLAR_EXPANDING_VENDOR && holds_dollar_reference(text) {
            return Err(SettingsError::ExpansionToken {
                pointer: self.absolute(pointer),
            });
        }
        Ok(())
    }

    fn typed(&self, value: &Value, ty: SettingsValueType, pointer: &str) -> Result<(), SettingsError> {
        match ty {
            SettingsValueType::Bool if value.is_boolean() => Ok(()),
            SettingsValueType::Bool => Err(self.wrong_type(pointer, "a boolean")),
            SettingsValueType::String if value.is_string() => Ok(()),
            SettingsValueType::String => Err(self.wrong_type(pointer, "a string")),
            SettingsValueType::Enum(allowed) => self.in_set(value, allowed, pointer),
            SettingsValueType::StringList => match value.as_array() {
                Some(items) if items.iter().all(Value::is_string) => Ok(()),
                _ => Err(self.wrong_type(pointer, "a list of strings")),
            },
            SettingsValueType::OneOf(arms) => self.one_of(value, arms, pointer),
            SettingsValueType::Atomic { keys } => self.closed_table(value, keys, pointer).map(|_| ()),
            SettingsValueType::EnumMap(allowed) => {
                let Value::Object(table) = value else {
                    return Err(self.wrong_type(pointer, "a table"));
                };
                table.iter().try_for_each(|(key, item)| {
                    self.in_set(item, allowed, &format!("{pointer}/{}", encode_segment(key)))
                })
            }
        }
    }

    fn one_of(&self, value: &Value, arms: &[SettingsValueType], pointer: &str) -> Result<(), SettingsError> {
        let mut closest = None;
        for arm in arms {
            match self.typed(value, *arm, pointer) {
                Ok(()) => return Ok(()),
                Err(refusal @ SettingsError::NotInEnum { .. }) => closest = Some(refusal),
                // An arm of another shape says nothing about this value.
                Err(_) => {}
            }
        }
        Err(closest.unwrap_or_else(|| self.wrong_type(pointer, "one of the accepted types")))
    }

    fn in_set(&self, value: &Value, allowed: &'static [&'static str], pointer: &str) -> Result<(), SettingsError> {
        match value.as_str() {
            Some(text) if allowed.contains(&text) => Ok(()),
            Some(_) => Err(SettingsError::NotInEnum {
                vendor: self.vendor.to_string(),
                pointer: pointer.to_string(),
                allowed,
            }),
            None => Err(self.wrong_type(pointer, "a string")),
        }
    }

    fn closed_table<'v>(
        &self,
        value: &'v Value,
        keys: &[&str],
        pointer: &str,
    ) -> Result<&'v Map<String, Value>, SettingsError> {
        let Value::Object(table) = value else {
            return Err(self.wrong_type(pointer, "a table"));
        };
        if let Some(key) = table.keys().find(|key| !keys.contains(&key.as_str())) {
            return Err(self.unknown_field(format!("{pointer}/{}", encode_segment(key))));
        }
        Ok(table)
    }

    fn marketplace(&self, value: &Value, pointer: &str) -> Result<(), SettingsError> {
        if self.vendor == ClientTarget::Claude.as_str() {
            self.claude_marketplace(value, pointer)?;
        }
        visit(value, pointer, &mut |value: &Value, at: &str| match value {
            Value::String(text) if holds_userinfo(text) => Err(SettingsError::CredentialInUrl {
                pointer: self.absolute(at),
            }),
            _ => Ok(()),
        })
    }

    /// A missing required key reads as null, which no arm below accepts.
    fn claude_marketplace(&self, value: &Value, pointer: &str) -> Result<(), SettingsError> {
        let at = format!("{pointer}/source");
        let source = self.closed_table(value.get("source").unwrap_or(&Value::Null), CLAUDE_SOURCE_KEYS, &at)?;
        self.in_set(
            source.get("source").unwrap_or(&Value::Null),
            CLAUDE_SOURCE_KINDS,
            &format!("{at}/source"),
        )?;
        self.typed(
            source.get("repo").unwrap_or(&Value::Null),
            SettingsValueType::String,
            &format!("{at}/repo"),
        )?;
        if let Some(reference) = source.get("ref") {
            self.typed(reference, SettingsValueType::String, &format!("{at}/ref"))?;
        }
        Ok(())
    }

    /// `pattern` is deliberately narrower than Codex's grammar, and `keys`
    /// keeps `match` and `not_match` off.
    fn rules(&self, value: &Value, keys: &[&str], pointer: &str) -> Result<(), SettingsError> {
        let Value::Array(entries) = value else {
            return Err(self.wrong_type(pointer, "a list of rules"));
        };
        for (index, entry) in entries.iter().enumerate() {
            let at = format!("{pointer}/{index}");
            let rule = self.closed_table(entry, keys, &at)?;

            let pattern = rule.get("pattern").unwrap_or(&Value::Null);
            let pattern_at = format!("{at}/pattern");
            self.typed(pattern, SettingsValueType::StringList, &pattern_at)?;
            if pattern.as_array().is_some_and(Vec::is_empty) {
                return Err(self.wrong_type(&pattern_at, "a non-empty list of strings"));
            }

            let decision = rule.get("decision").unwrap_or(&Value::Null);
            self.in_set(decision, RULE_DECISIONS, &format!("{at}/decision"))?;
            if let Some(justification) = rule.get("justification") {
                self.typed(justification, SettingsValueType::String, &format!("{at}/justification"))?;
            }
            visit(entry, &at, &mut |value: &Value, _: &str| match value {
                Value::String(text) if text.chars().any(char::is_control) => {
                    Err(SettingsError::RulesControlCharacter { index })
                }
                _ => Ok(()),
            })?;
        }
        Ok(())
    }
}

/// The JSON Schema of an authored document, `requires` aside.
///
/// Built from the rows the validator reads, so a row reaches both or neither.
/// The checks a schema cannot state stay the validator's alone: marketplace
/// shapes, expansion tokens, control characters and exclusive keys.
pub fn document_json_schema() -> Value {
    let mut properties: Map<String, Value> = DESCRIPTIVE_KEYS
        .iter()
        .map(|(key, description)| (key.to_string(), json!({"type": "string", "description": description})))
        .collect();
    for client in ClientTarget::ALL {
        if let Some(rows) = known_fields(client.as_str()) {
            let rows: Vec<_> = rows.iter().collect();
            properties.insert(client.as_str().to_string(), table_schema(&rows, 0));
        }
    }
    json!({
        "type": "object",
        "required": ["description"],
        "additionalProperties": false,
        "properties": properties,
    })
}

fn table_schema(rows: &[&SettingsField], depth: usize) -> Value {
    let mut groups: BTreeMap<&str, Vec<&SettingsField>> = BTreeMap::new();
    for row in rows {
        if let Some(segment) = segment_at(row, depth) {
            groups.entry(segment).or_default().push(row);
        }
    }
    let mut properties = Map::new();
    let mut other_keys = Value::Bool(false);
    for (segment, group) in groups {
        let value = value_schema(&group, depth + 1);
        if !is_placeholder(segment) {
            properties.insert(segment.to_string(), value);
        } else if let Some(keys) = group.iter().find_map(|row| row.map_keys) {
            properties.extend(keys.iter().map(|key| (key.to_string(), value.clone())));
        } else {
            other_keys = value;
        }
    }
    json!({"type": "object", "properties": properties, "additionalProperties": other_keys})
}

/// Two rows can share a prefix, as `/permission` and `/permission/<tool>` do,
/// so the value may take either shape.
fn value_schema(rows: &[&SettingsField], next: usize) -> Value {
    let leaf = rows.iter().find(|row| segment_at(row, next).is_none());
    let deeper: Vec<_> = rows
        .iter()
        .copied()
        .filter(|row| segment_at(row, next).is_some())
        .collect();
    match (leaf, deeper.is_empty()) {
        (Some(row), true) => leaf_schema(row),
        (Some(row), false) => json!({"anyOf": [leaf_schema(row), table_schema(&deeper, next)]}),
        (None, _) => table_schema(&deeper, next),
    }
}

fn leaf_schema(row: &SettingsField) -> Value {
    match row.ty {
        // An own-file row's type describes one entry, not the list it heads.
        SettingsValueType::Atomic { keys } if row.merge == MergeClass::OwnFile => rules_schema(keys),
        ty => type_schema(ty),
    }
}

fn rules_schema(keys: &[&str]) -> Value {
    let properties: Map<String, Value> = keys
        .iter()
        .map(|key| {
            let value = match *key {
                "pattern" => json!({"type": "array", "items": {"type": "string"}, "minItems": 1}),
                "decision" => json!({"type": "string", "enum": RULE_DECISIONS}),
                _ => json!({"type": "string"}),
            };
            (key.to_string(), value)
        })
        .collect();
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["pattern", "decision"],
            "additionalProperties": false,
            "properties": properties,
        },
    })
}

fn type_schema(ty: SettingsValueType) -> Value {
    match ty {
        SettingsValueType::Bool => json!({"type": "boolean"}),
        SettingsValueType::String => json!({"type": "string"}),
        SettingsValueType::Enum(allowed) => json!({"type": "string", "enum": allowed}),
        SettingsValueType::StringList => json!({"type": "array", "items": {"type": "string"}}),
        SettingsValueType::OneOf(arms) => {
            json!({"anyOf": arms.iter().copied().map(type_schema).collect::<Vec<_>>()})
        }
        SettingsValueType::Atomic { keys } => {
            let properties: Map<String, Value> = keys.iter().map(|key| (key.to_string(), Value::Bool(true))).collect();
            json!({"type": "object", "properties": properties, "additionalProperties": false})
        }
        SettingsValueType::EnumMap(allowed) => {
            json!({"type": "object", "additionalProperties": {"type": "string", "enum": allowed}})
        }
    }
}

/// What validation reports without refusing. Each reads after the package it
/// concerns: `settings package '<name>' <warning>`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsWarning {
    /// A vendor this grim does not know, ignored rather than refused.
    #[error(
        "carries a table for '{vendor}', a client this grim does not know; \
         it is ignored, and a newer grim may apply it"
    )]
    UnknownVendor {
        /// The vendor table's name.
        vendor: String,
    },
    /// A client this grim knows but writes no settings for, ignored. This grim
    /// refuses to build such a table, so only a newer grim's layer carries one.
    #[error(
        "carries a table for '{vendor}', which takes no settings from this grim; \
         it is ignored, and a newer grim may apply it"
    )]
    ClientWithoutSettings {
        /// The vendor table's name.
        vendor: String,
    },
}

/// The words an unknown-field refusal carries; the error renderer's
/// unknown-key hint keys on them.
pub(crate) const UNKNOWN_SETTING: &str = "has no setting at";

/// The quoting advice for an unknown `pointer` that a row of `vendor` spells as
/// the start of one dotted key: TOML split a bare dotted key into tables.
/// Empty when no row does. Names a row, never the document's value.
fn dotted_key_hint(vendor: &str, pointer: &str) -> String {
    let segments: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
    let Some((last, parents)) = segments.split_last() else {
        return String::new();
    };
    known_fields(vendor)
        .into_iter()
        .flatten()
        .filter(|row| {
            parents
                .iter()
                .enumerate()
                .all(|(depth, key)| row_takes_key(row, depth, key))
        })
        .filter_map(|row| segment_at(row, parents.len()))
        .find(|segment| segment.strip_prefix(last).is_some_and(|rest| rest.starts_with('.')))
        .map(|segment| {
            format!("; {vendor} keys are dotted names, so quote the whole key in TOML, as in \"{segment}\" = …")
        })
        .unwrap_or_default()
}

/// Whether `vendor` names a client this grim knows, settings rows or not.
fn is_client(vendor: &str) -> bool {
    ClientTarget::ALL.iter().any(|client| client.as_str() == vendor)
}

/// The words an unknown-vendor refusal carries; the error renderer's
/// unknown-key hint keys on them.
pub(crate) const UNKNOWN_VENDOR_TABLE: &str = "unknown vendor table";

/// A known client with no settings rows is named as one, not as unknown.
fn unknown_vendor_message(vendor: &str) -> String {
    if is_client(vendor) {
        format!("{vendor} takes no settings from grim, so a package cannot carry a {vendor} table")
    } else {
        format!("{UNKNOWN_VENDOR_TABLE} {vendor}")
    }
}

/// Why a settings document was refused. Values never reach a message: a
/// settings table can hold a token, so each variant names only its pointer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// A field outside the vendor's table. `mode` is the gate that saw it: at
    /// [`ValidationMode::Consume`] the refusal carries the
    /// `unsupported-artifact-feature` reason.
    #[error("{vendor} {UNKNOWN_SETTING} {pointer}{}", dotted_key_hint(.vendor, .pointer))]
    UnknownField {
        vendor: String,
        pointer: String,
        mode: ValidationMode,
    },
    /// A vendor table this grim does not know, or a client it writes no settings for.
    #[error("{}", unknown_vendor_message(.vendor))]
    UnknownVendor { vendor: String },
    /// A value of the wrong JSON type.
    #[error("{vendor} setting {pointer} must be {expected}")]
    WrongType {
        vendor: String,
        pointer: String,
        expected: &'static str,
    },
    /// A value outside the field's closed set.
    #[error("{vendor} setting {pointer} must be one of {allowed:?}")]
    NotInEnum {
        vendor: String,
        pointer: String,
        allowed: &'static [&'static str],
    },
    /// A JSON null, which no harness reads as "unset".
    #[error("setting {pointer} is null")]
    NullValue { pointer: String },
    /// A TOML datetime, which JSON cannot carry.
    #[error("setting {pointer} is a datetime")]
    DatetimeValue { pointer: String },
    /// A float, which has no stable hash.
    #[error("setting {pointer} is a float")]
    FloatValue { pointer: String },
    /// An integer outside the portable JSON range.
    #[error("setting {pointer} is out of range")]
    IntegerRange { pointer: String },
    /// A token the harness would expand when it reads the file.
    #[error(
        "setting {pointer} holds an expansion token, which the harness replaces with an environment \
         variable or a file when it reads the file; write the value literally"
    )]
    ExpansionToken { pointer: String },
    /// Two keys that cannot be set together.
    #[error("settings {first} and {second} are exclusive")]
    ExclusiveKeys { first: String, second: String },
    /// A URL carrying a credential.
    #[error("setting {pointer} holds a credential in a URL")]
    CredentialInUrl { pointer: String },
    /// A control character in a rule string.
    #[error("rule {index} holds a control character")]
    RulesControlCharacter { index: usize },
    /// A layer past the size cap.
    #[error("settings layer is {actual} bytes, the limit is {limit}")]
    TooLarge { actual: u64, limit: u64 },
    /// Text that does not parse.
    #[error("malformed settings document: {detail}")]
    Malformed { detail: String },
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;
    use crate::oci::ArtifactKind;

    /// Planted in refused values: a message that repeats it has leaked a value.
    const MARKER: &str = "fake-settings-value-0001";

    /// Every descriptive key, a `requires` edge and one table per vendor shape.
    const FULL_SOURCE: &str = r#"
description = "team settings"
summary = "short"
keywords = "a,b"
license = "Apache-2.0"
repository = "https://example.com/team/settings"
deprecated = "use the next one"
replaced-by = "ghcr.io/acme/settings/next:1"
authors = "Team"
vendor = "Acme"
homepage = "https://example.com"
documentation = "https://example.com/docs"

[requires.skills]
helper = "./helper:1"

[claude]
model = "opus"

[claude.permissions]
allow = ["Read"]

[codex]
model = "gpt"

[[codex.rules]]
pattern = ["git", "status"]
decision = "allow"

[opencode]
autoupdate = "notify"

[opencode.permission.bash]
"*" = "ask"
"git *" = "allow"
"#;

    /// Whether `schema`, followed along `segments`, can hold `expected`.
    fn reaches(schema: &Value, segments: &[String], expected: &Value) -> bool {
        if segments.is_empty() && schema == expected {
            return true;
        }
        if let Some(arms) = schema["anyOf"].as_array() {
            return arms.iter().any(|arm| reaches(arm, segments, expected));
        }
        let Some((head, rest)) = segments.split_first() else {
            return false;
        };
        let next = schema["properties"]
            .get(head)
            .unwrap_or(&schema["additionalProperties"]);
        reaches(next, rest, expected)
    }

    /// A document holding `vendors`, built without either parser.
    fn document(vendors: Value) -> SettingsDocument {
        let Value::Object(vendors) = vendors else {
            panic!("a test names its vendors in an object");
        };
        SettingsDocument {
            description: "test".to_string(),
            summary: None,
            keywords: None,
            license: None,
            repository: None,
            deprecated: None,
            replaced_by: None,
            authors: None,
            vendor: None,
            homepage: None,
            documentation: None,
            vendors: vendors
                .into_iter()
                .map(|(vendor, table)| match table {
                    Value::Object(table) => (vendor, table),
                    other => panic!("vendor {vendor} holds {other}"),
                })
                .collect(),
        }
    }

    /// A layer written by hand, so no consume test leans on `to_layer_bytes`.
    fn layer(vendors: Value) -> Vec<u8> {
        let mut body = vendors;
        body["description"] = json!("test");
        serde_json::to_vec(&body).unwrap()
    }

    /// The refusal of each gate: `validate` at Author, `from_layer_bytes` at Consume.
    fn refusals(vendors: Value) -> (SettingsError, SettingsError) {
        let author = document(vendors.clone()).validate(ValidationMode::Author).unwrap_err();
        let consume = SettingsDocument::from_layer_bytes("test", &layer(vendors)).unwrap_err();
        (author, consume)
    }

    fn accepted(vendors: Value) {
        document(vendors.clone()).validate(ValidationMode::Author).unwrap();
        SettingsDocument::from_layer_bytes("test", &layer(vendors)).unwrap();
    }

    fn parsed(raw: &str) -> Result<(SettingsDocument, Vec<BundleMember>), SettingsError> {
        SettingsDocument::from_toml_str_with_requires(raw)
    }

    fn layer_text(document: &SettingsDocument) -> String {
        String::from_utf8(document.to_layer_bytes().unwrap()).unwrap()
    }

    fn wrong_type(vendor: &str, pointer: &str, expected: &'static str) -> SettingsError {
        SettingsError::WrongType {
            vendor: vendor.to_string(),
            pointer: pointer.to_string(),
            expected,
        }
    }

    fn unknown_field(vendor: &str, pointer: &str, mode: ValidationMode) -> SettingsError {
        SettingsError::UnknownField {
            vendor: vendor.to_string(),
            pointer: pointer.to_string(),
            mode,
        }
    }

    /// `vendors` refused with `expected` at both gates.
    pub(crate) fn assert_refused_alike(vendors: Value, expected: &SettingsError) {
        let (author, consume) = refusals(vendors.clone());
        assert_eq!(&author, expected, "author, {vendors}");
        assert_eq!(&consume, expected, "consume, {vendors}");
    }

    /// `pointer` under both gates, each naming the gate that saw it.
    fn assert_unknown_field(vendors: Value, vendor: &str, pointer: &str) {
        let (author, consume) = refusals(vendors);
        assert_eq!(author, unknown_field(vendor, pointer, ValidationMode::Author));
        assert_eq!(consume, unknown_field(vendor, pointer, ValidationMode::Consume));
    }

    fn assert_not_in_enum(vendors: Value, vendor_name: &str, at: &str) {
        let (author, consume) = refusals(vendors.clone());
        for refusal in [author, consume] {
            assert!(
                matches!(&refusal, SettingsError::NotInEnum { vendor, pointer, .. } if vendor == vendor_name && pointer == at),
                "{vendors}: {refusal:?}"
            );
        }
    }

    /// Both values reach the wire: a published layer carries the media type,
    /// and the cap judges a registry's descriptor.
    #[test]
    fn every_vendor_row_reaches_the_schema() {
        let schema = document_json_schema();
        let mut rows = 0;
        for client in ClientTarget::ALL {
            for row in client.vendor().settings_fields() {
                let segments: Vec<String> = row
                    .pointer
                    .trim_start_matches('/')
                    .split('/')
                    .map(|segment| match (is_placeholder(segment), row.map_keys) {
                        (true, Some(keys)) => keys[0].to_string(),
                        (true, None) => "any-key".to_string(),
                        (false, _) => segment.to_string(),
                    })
                    .collect();
                let table = &schema["properties"][client.as_str()];
                assert!(
                    reaches(table, &segments, &leaf_schema(row)),
                    "{client:?} {}",
                    row.pointer
                );
                rows += 1;
            }
        }
        assert!(rows > 0);
    }

    #[test]
    fn the_schema_holds_a_closed_table_only_for_a_vendor_with_rows() {
        let schema = document_json_schema();
        for client in ClientTarget::ALL {
            let table = &schema["properties"][client.as_str()];
            if client.vendor().settings_fields().is_empty() {
                assert!(table.is_null(), "{client:?}");
            } else {
                assert_eq!(table["type"], "object", "{client:?}");
                assert_eq!(table["additionalProperties"], false, "{client:?}");
            }
        }
    }

    #[test]
    fn a_closed_map_lists_its_keys_and_an_open_one_takes_any_key() {
        let schema = document_json_schema();
        let bash = &schema["properties"]["opencode"]["properties"]["permission"]["anyOf"][1]["properties"]["bash"];
        assert_eq!(
            bash["anyOf"][1]["additionalProperties"]["enum"],
            json!(["allow", "ask", "deny"])
        );
        let marketplaces = &schema["properties"]["claude"]["properties"]["extraKnownMarketplaces"];
        assert_eq!(marketplaces["additionalProperties"]["type"], "object");
    }

    #[test]
    fn a_rule_in_the_schema_requires_what_the_validator_requires() {
        let schema = document_json_schema();
        let rule = &schema["properties"]["codex"]["properties"]["rules"]["items"];
        assert_eq!(rule["required"], json!(["pattern", "decision"]));
        assert_eq!(rule["additionalProperties"], false);
        assert_eq!(rule["properties"]["decision"]["enum"], json!(RULE_DECISIONS));
        assert_eq!(rule["properties"]["pattern"]["minItems"], 1);
    }

    #[test]
    fn every_descriptive_key_of_the_schema_reaches_the_layer() {
        let mut raw = String::new();
        for (key, _) in DESCRIPTIVE_KEYS {
            let value = if *key == "repository" {
                "https://example.com/r"
            } else {
                "text"
            };
            raw.push_str(&format!("{key} = \"{value}\"\n"));
        }
        let (parsed, _) = parsed(&raw).unwrap();
        let layer: Value = serde_json::from_str(&layer_text(&parsed)).unwrap();
        let mut keys: Vec<&str> = DESCRIPTIVE_KEYS.iter().map(|(key, _)| *key).collect();
        keys.sort_unstable();
        assert_eq!(
            layer
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            keys
        );
    }

    #[test]
    fn the_layer_constants_hold_their_wire_values() {
        assert_eq!(SETTINGS_LAYER_MEDIA_TYPE, "application/vnd.grimoire.settings.v1+json");
        assert_eq!(SETTINGS_LAYER_SIZE_LIMIT, 65_536);
    }

    /// Author and Consume are two distinct gates, not one spelled twice.
    #[test]
    fn the_validation_modes_stay_distinct() {
        assert_ne!(ValidationMode::Author, ValidationMode::Consume);
    }

    #[test]
    fn a_document_round_trips_through_its_layer() {
        let (document, _) = parsed(FULL_SOURCE).unwrap();
        let bytes = document.to_layer_bytes().unwrap();
        assert_eq!(SettingsDocument::from_layer_bytes("test", &bytes).unwrap(), document);
    }

    #[test]
    fn layer_bytes_do_not_depend_on_authored_key_order() {
        let first = "license = \"MIT\"\ndescription = \"x\"\n[claude]\nmodel = \"a\"\nlanguage = \"en\"\n[codex]\nmodel = \"b\"\n";
        let second = "description = \"x\"\nlicense = \"MIT\"\n[codex]\nmodel = \"b\"\n[claude]\nlanguage = \"en\"\nmodel = \"a\"\n";
        let (first, _) = parsed(first).unwrap();
        let (second, _) = parsed(second).unwrap();
        assert_eq!(first.to_layer_bytes().unwrap(), second.to_layer_bytes().unwrap());
    }

    #[test]
    fn the_layer_is_pretty_sorted_json_ending_in_a_newline() {
        let (document, _) =
            parsed("license = \"MIT\"\ndescription = \"x\"\n[codex]\nmodel = \"b\"\n[claude]\nmodel = \"a\"\n")
                .unwrap();
        assert_eq!(
            layer_text(&document),
            "{\n  \"claude\": {\n    \"model\": \"a\"\n  },\n  \"codex\": {\n    \"model\": \"b\"\n  },\n  \"description\": \"x\",\n  \"license\": \"MIT\"\n}\n"
        );
    }

    /// A written `null` would fail this grim's own consume check.
    #[test]
    fn an_unset_descriptive_field_stays_out_of_the_layer() {
        assert_eq!(layer_text(&document(json!({}))), "{\n  \"description\": \"test\"\n}\n");
    }

    #[test]
    fn every_descriptive_field_reaches_the_layer_under_its_wire_key() {
        let (document, _) = parsed(FULL_SOURCE).unwrap();
        let layer: Value = serde_json::from_str(&layer_text(&document)).unwrap();
        for key in [
            "description",
            "summary",
            "keywords",
            "license",
            "repository",
            "deprecated",
            "replaced-by",
            "authors",
            "vendor",
            "homepage",
            "documentation",
        ] {
            assert!(layer.get(key).is_some_and(Value::is_string), "{key}");
        }
        assert!(layer.get("replaced_by").is_none());
    }

    #[test]
    fn requires_splits_off_and_never_reaches_the_layer() {
        let (document, requires) = parsed(FULL_SOURCE).unwrap();
        assert_eq!(
            requires,
            [BundleMember {
                kind: ArtifactKind::Skill,
                name: "helper".to_string(),
                id: "./helper:1".to_string(),
            }]
        );
        assert!(!document.vendors.contains_key("requires"));
        let layer: Value = serde_json::from_str(&layer_text(&document)).unwrap();
        assert!(layer.get("requires").is_none());
    }

    #[test]
    fn a_requires_key_in_a_layer_is_refused() {
        let bytes = serde_json::to_vec(&json!({"description": "test", "requires": {"skills": {}}})).unwrap();
        let refusal = SettingsDocument::from_layer_bytes("test", &bytes).unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");
    }

    #[test]
    fn a_malformed_requires_table_is_refused() {
        for raw in [
            "description = \"x\"\nrequires = 1\n",
            "description = \"x\"\n[requires]\nwidgets = {}\n",
        ] {
            let refusal = parsed(raw).unwrap_err();
            assert!(matches!(refusal, SettingsError::Malformed { .. }), "{raw}: {refusal:?}");
        }
    }

    /// `[bundles]` is a dependency table in a bundle source only.
    #[test]
    fn a_bundles_table_names_a_vendor_and_not_a_dependency() {
        let refusal = parsed("description = \"x\"\n[bundles]\nset = \"./set:1\"\n").unwrap_err();
        assert_eq!(
            refusal,
            SettingsError::UnknownVendor {
                vendor: "bundles".to_string()
            }
        );
    }

    #[test]
    fn a_vendor_that_is_not_a_table_is_refused() {
        let expected = wrong_type("claude", "/", "a table");
        assert_eq!(parsed("description = \"x\"\nclaude = 1\n").unwrap_err(), expected);
        let bytes = serde_json::to_vec(&json!({"description": "x", "claude": 1})).unwrap();
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", &bytes).unwrap_err(),
            expected
        );

        let bytes = serde_json::to_vec(&json!({"description": "x", "newclient": 1})).unwrap();
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", &bytes).unwrap_err(),
            wrong_type("newclient", "/", "a table")
        );
    }

    #[test]
    fn a_missing_or_blank_description_is_refused() {
        for raw in ["[claude]\nmodel = \"a\"\n", "description = \"  \"\n"] {
            let refusal = parsed(raw).unwrap_err();
            assert!(matches!(refusal, SettingsError::Malformed { .. }), "{raw}: {refusal:?}");
        }
        let refusal = SettingsDocument::from_layer_bytes("test", b"{}").unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");
    }

    #[test]
    fn a_descriptive_field_must_be_a_string() {
        let refusal = parsed("description = \"x\"\nlicense = 1\n").unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");

        let bytes = serde_json::to_vec(&json!({"description": "x", "license": null})).unwrap();
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", &bytes).unwrap_err(),
            SettingsError::NullValue {
                pointer: "/license".to_string()
            }
        );
    }

    #[test]
    fn a_repository_must_be_https_without_userinfo() {
        let refusal = parsed("description = \"x\"\nrepository = \"http://example.com/r\"\n").unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");

        let bytes =
            serde_json::to_vec(&json!({"description": "x", "repository": "https://u:p@example.com/r"})).unwrap();
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", &bytes).unwrap_err(),
            SettingsError::CredentialInUrl {
                pointer: "/repository".to_string()
            }
        );
    }

    #[test]
    fn an_unknown_field_is_refused_and_names_the_gate_that_saw_it() {
        assert_unknown_field(json!({"claude": {"nope": true}}), "claude", "/nope");
    }

    #[test]
    fn an_unknown_nested_field_carries_its_full_pointer() {
        assert_unknown_field(
            json!({"claude": {"permissions": {"nope": []}}}),
            "claude",
            "/permissions/nope",
        );
    }

    /// A newer publisher's field must reach the reason that says so, whatever
    /// value it holds.
    #[test]
    fn an_unknown_field_is_named_before_its_value_is_read() {
        assert_unknown_field(json!({"claude": {"nope": 1.5}}), "claude", "/nope");
        assert_unknown_field(json!({"claude": {"nope": null}}), "claude", "/nope");
    }

    #[test]
    fn an_unknown_vendor_is_refused_by_the_author_and_warned_at_consume() {
        let vendors = json!({"newclient": {"anything": true}});
        assert_eq!(
            document(vendors.clone()).validate(ValidationMode::Author).unwrap_err(),
            SettingsError::UnknownVendor {
                vendor: "newclient".to_string()
            }
        );
        let consumed = SettingsDocument::from_layer_bytes("test", &layer(vendors)).unwrap();
        assert_eq!(
            consumed.validate(ValidationMode::Consume).unwrap(),
            [SettingsWarning::UnknownVendor {
                vendor: "newclient".to_string()
            }]
        );
    }

    /// Zed is a client with no settings rows, so a table for it has nowhere to
    /// go: the author is refused, and a consumer is told it is a known client.
    #[test]
    fn a_client_without_settings_rows_is_refused_by_the_author_and_named_at_consume() {
        assert!(ClientTarget::Zed.vendor().settings_fields().is_empty());
        let vendors = json!({"zed": {"model": "a"}});
        assert!(matches!(
            document(vendors.clone()).validate(ValidationMode::Author),
            Err(SettingsError::UnknownVendor { .. })
        ));
        let consumed = SettingsDocument::from_layer_bytes("test", &layer(vendors)).unwrap();
        assert_eq!(
            consumed.validate(ValidationMode::Consume).unwrap(),
            [SettingsWarning::ClientWithoutSettings {
                vendor: "zed".to_string()
            }]
        );
    }

    #[test]
    fn an_unknown_vendor_at_consume_still_refuses_null_and_floats() {
        let refusal =
            SettingsDocument::from_layer_bytes("test", &layer(json!({"newclient": {"a": {"b": null}}}))).unwrap_err();
        assert_eq!(
            refusal,
            SettingsError::NullValue {
                pointer: "/newclient/a/b".to_string()
            }
        );
        let refusal =
            SettingsDocument::from_layer_bytes("test", &layer(json!({"newclient": {"a": [1.5]}}))).unwrap_err();
        assert_eq!(
            refusal,
            SettingsError::FloatValue {
                pointer: "/newclient/a/0".to_string()
            }
        );
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused_in_both_modes() {
        let cases = [
            (
                json!({"claude": {"spinnerTipsEnabled": "yes"}}),
                "claude",
                "/spinnerTipsEnabled",
                "a boolean",
            ),
            (json!({"claude": {"model": true}}), "claude", "/model", "a string"),
            (
                json!({"claude": {"permissions": {"allow": "Read"}}}),
                "claude",
                "/permissions/allow",
                "a list of strings",
            ),
            (
                json!({"claude": {"permissions": {"allow": [1]}}}),
                "claude",
                "/permissions/allow",
                "a list of strings",
            ),
            (
                json!({"claude": {"permissions": "all"}}),
                "claude",
                "/permissions",
                "a table",
            ),
            (
                json!({"codex": {"permissions": {"ci": "x"}}}),
                "codex",
                "/permissions/ci",
                "a table",
            ),
            (
                json!({"claude": {"effortLevel": 1}}),
                "claude",
                "/effortLevel",
                "a string",
            ),
            (
                json!({"opencode": {"autoupdate": 1}}),
                "opencode",
                "/autoupdate",
                "one of the accepted types",
            ),
        ];
        for (vendors, vendor, pointer, expected) in cases {
            assert_refused_alike(vendors, &wrong_type(vendor, pointer, expected));
        }
    }

    #[test]
    fn a_value_outside_its_enum_is_refused_in_both_modes() {
        assert_not_in_enum(json!({"claude": {"effortLevel": "extreme"}}), "claude", "/effortLevel");
        assert_not_in_enum(
            json!({"codex": {"approval_policy": "always"}}),
            "codex",
            "/approval_policy",
        );
        assert_not_in_enum(
            json!({"opencode": {"autoupdate": "sometimes"}}),
            "opencode",
            "/autoupdate",
        );
    }

    #[test]
    fn null_is_refused_anywhere_in_both_modes() {
        let cases = [
            (json!({"claude": {"model": null}}), "/claude/model"),
            (json!({"claude": {"permissions": null}}), "/claude/permissions"),
            (
                json!({"claude": {"permissions": {"allow": [null]}}}),
                "/claude/permissions/allow/0",
            ),
            (
                json!({"codex": {"permissions": {"ci": {"network": {"inner": null}}}}}),
                "/codex/permissions/ci/network/inner",
            ),
        ];
        for (vendors, pointer) in cases {
            assert_refused_alike(
                vendors,
                &SettingsError::NullValue {
                    pointer: pointer.to_string(),
                },
            );
        }
    }

    #[test]
    fn a_null_vendor_or_descriptive_field_is_refused() {
        let bytes = serde_json::to_vec(&json!({"description": "x", "claude": null})).unwrap();
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", &bytes).unwrap_err(),
            SettingsError::NullValue {
                pointer: "/claude".to_string()
            }
        );
    }

    #[test]
    fn a_toml_datetime_is_refused_at_author() {
        let cases = [
            ("[claude]\nmodel = 2026-11-04T10:00:00Z\n", "/claude/model"),
            ("[claude]\nmodel = 2026-11-04\n", "/claude/model"),
            (
                "[claude.permissions]\nallow = [10:00:00]\n",
                "/claude/permissions/allow/0",
            ),
            (
                "[codex.permissions.ci]\nnetwork = { at = 2026-11-04 }\n",
                "/codex/permissions/ci/network/at",
            ),
        ];
        for (body, pointer) in cases {
            assert_eq!(
                parsed(&format!("description = \"x\"\n{body}")).unwrap_err(),
                SettingsError::DatetimeValue {
                    pointer: pointer.to_string()
                },
                "{body}"
            );
        }
    }

    /// JSON has no datetime, and the sentinel key a TOML parser uses for one is
    /// an ordinary unknown field in a layer.
    #[test]
    fn a_datetime_has_no_form_in_a_layer() {
        accepted(json!({"claude": {"model": "2026-11-04T10:00:00Z"}}));
        assert_unknown_field(
            json!({"claude": {"$__toml_private_datetime": "2026-11-04"}}),
            "claude",
            "/$__toml_private_datetime",
        );
    }

    #[test]
    fn a_float_is_refused_in_both_modes() {
        let cases = [
            (json!({"claude": {"model": 1.5}}), "/claude/model"),
            (
                json!({"claude": {"permissions": {"allow": [1.5]}}}),
                "/claude/permissions/allow/0",
            ),
            (
                json!({"codex": {"permissions": {"ci": {"network": {"limit": 1.5}}}}}),
                "/codex/permissions/ci/network/limit",
            ),
        ];
        for (vendors, pointer) in cases {
            assert_refused_alike(
                vendors,
                &SettingsError::FloatValue {
                    pointer: pointer.to_string(),
                },
            );
        }
        for value in ["1.5", "nan", "inf"] {
            assert_eq!(
                parsed(&format!("description = \"x\"\n[claude]\nmodel = {value}\n")).unwrap_err(),
                SettingsError::FloatValue {
                    pointer: "/claude/model".to_string()
                },
                "{value}"
            );
        }
    }

    /// Past `u64` the JSON parser hands over a float, which is refused as one.
    #[test]
    fn an_integer_past_i64_is_refused_in_both_modes() {
        assert_refused_alike(
            json!({"claude": {"model": 9_223_372_036_854_775_808_u64}}),
            &SettingsError::IntegerRange {
                pointer: "/claude/model".to_string(),
            },
        );
        let refusal = parsed("description = \"x\"\n[claude]\nmodel = 9223372036854775808\n").unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");
        for number in ["18446744073709551616", "-9223372036854775809"] {
            let raw = format!("{{\"description\": \"test\", \"claude\": {{\"model\": {number}}}}}");
            assert_eq!(
                SettingsDocument::from_layer_bytes("test", raw.as_bytes()).unwrap_err(),
                SettingsError::FloatValue {
                    pointer: "/claude/model".to_string()
                },
                "{number}"
            );
        }
    }

    #[test]
    fn an_integer_at_the_i64_bounds_passes() {
        accepted(json!({"codex": {"permissions": {"ci": {"network": {"high": i64::MAX, "low": i64::MIN}}}}}));
    }

    #[test]
    fn an_expansion_token_under_opencode_is_refused_in_both_modes() {
        let cases = [
            (json!({"opencode": {"model": "{env:KEY}"}}), "/opencode/model"),
            (json!({"opencode": {"model": "x{file:notes}"}}), "/opencode/model"),
            (json!({"opencode": {"{env:KEY}": true}}), "/opencode/{env:KEY}"),
            (
                json!({"opencode": {"permission": {"{file:a}": "allow"}}}),
                "/opencode/permission/{file:a}",
            ),
            (
                json!({"opencode": {"permission": {"bash": {"{env:X}": "allow"}}}}),
                "/opencode/permission/bash/{env:X}",
            ),
        ];
        for (vendors, pointer) in cases {
            assert_refused_alike(
                vendors,
                &SettingsError::ExpansionToken {
                    pointer: pointer.to_string(),
                },
            );
        }
    }

    /// The harness reads decoded text, so an escaped brace is still a token.
    #[test]
    fn an_expansion_token_is_judged_after_decoding() {
        let raw = br#"{"description": "test", "opencode": {"model": "\u007benv:KEY}"}}"#;
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", raw).unwrap_err(),
            SettingsError::ExpansionToken {
                pointer: "/opencode/model".to_string()
            }
        );
        let raw = br#"{"description": "test", "opencode": {"permission": {"bash": {"\u007bfile:a}": "allow"}}}}"#;
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", raw).unwrap_err(),
            SettingsError::ExpansionToken {
                pointer: "/opencode/permission/bash/{file:a}".to_string()
            }
        );
        assert_eq!(
            parsed("description = \"x\"\n[opencode]\nmodel = \"\\u007Benv:KEY}\"\n").unwrap_err(),
            SettingsError::ExpansionToken {
                pointer: "/opencode/model".to_string()
            }
        );
    }

    #[test]
    fn an_expansion_token_outside_opencode_is_plain_text() {
        accepted(json!({"claude": {"model": "{env:KEY}"}}));
    }

    #[test]
    fn the_codex_exclusion_is_refused_in_both_modes() {
        let cases = [
            (
                json!({"codex": {"default_permissions": "ci", "sandbox_mode": "read-only"}}),
                "/codex/sandbox_mode",
            ),
            (
                json!({"codex": {"default_permissions": "ci", "sandbox_workspace_write": {"network_access": true}}}),
                "/codex/sandbox_workspace_write",
            ),
        ];
        for (vendors, second) in cases {
            assert_refused_alike(
                vendors,
                &SettingsError::ExclusiveKeys {
                    first: "/codex/default_permissions".to_string(),
                    second: second.to_string(),
                },
            );
        }
    }

    #[test]
    fn each_side_of_the_codex_exclusion_passes_alone() {
        accepted(json!({"codex": {"default_permissions": "ci"}}));
        accepted(json!({"codex": {"sandbox_mode": "read-only", "sandbox_workspace_write": {"network_access": true}}}));
    }

    #[test]
    fn a_marketplace_url_with_userinfo_is_refused_in_both_modes() {
        let cases = [
            (
                json!({"codex": {"marketplaces": {"m": {"source": "https://user:pass@example.com/m.git"}}}}),
                "/codex/marketplaces/m/source",
            ),
            (
                json!({"copilot": {"extraKnownMarketplaces": {"m": {"source": "https://token@example.com/m"}}}}),
                "/copilot/extraKnownMarketplaces/m/source",
            ),
            (
                json!({"claude": {"extraKnownMarketplaces": {"m": {"source": {"source": "github", "repo": "https://u:p@github.com/a/b"}}}}}),
                "/claude/extraKnownMarketplaces/m/source/repo",
            ),
            (
                json!({"codex": {"marketplaces": {"m": {"source": "ssh://git@example.com/m.git"}}}}),
                "/codex/marketplaces/m/source",
            ),
            (
                json!({"codex": {"marketplaces": {"m": {"source": "https://example.com/m", "sparse_paths": ["https://u:p@example.com/x"]}}}}),
                "/codex/marketplaces/m/sparse_paths/0",
            ),
            (
                json!({"codex": {"marketplaces": {"m": {"source": "https://example.com/?next=https://u:p@example.org/x"}}}}),
                "/codex/marketplaces/m/source",
            ),
            (
                json!({"codex": {"marketplaces": {"m": {"source": "https:///u:p@example.com/m"}}}}),
                "/codex/marketplaces/m/source",
            ),
        ];
        for (vendors, pointer) in cases {
            assert_refused_alike(
                vendors,
                &SettingsError::CredentialInUrl {
                    pointer: pointer.to_string(),
                },
            );
        }
    }

    #[test]
    fn a_marketplace_url_without_userinfo_passes() {
        for source in [
            "https://example.com/m.git",
            "https://example.com/org/repo@v1",
            "https://example.com/m?ref=a@b",
            "git@example.com:org/repo.git",
        ] {
            accepted(json!({"codex": {"marketplaces": {"m": {"source": source}}}}));
        }
    }

    /// A row that names a source outside the list would skip the userinfo check.
    #[test]
    fn every_atomic_row_with_a_source_key_is_a_marketplace_row() {
        for client in ClientTarget::ALL {
            for row in client.vendor().settings_fields() {
                if let SettingsValueType::Atomic { keys } = row.ty
                    && keys.contains(&"source")
                {
                    assert!(MARKETPLACE_ROWS.contains(&row.pointer), "{}", row.pointer);
                }
            }
        }
    }

    #[test]
    fn a_control_character_in_a_rule_is_refused_in_both_modes() {
        let cases = [
            (
                json!({"codex": {"rules": [{"pattern": ["git\u{1b}"], "decision": "allow"}]}}),
                0,
            ),
            (
                json!({"codex": {"rules": [{"pattern": ["git"], "decision": "allow", "justification": "a\nb"}]}}),
                0,
            ),
            (
                json!({"codex": {"rules": [
                    {"pattern": ["git"], "decision": "allow"},
                    {"pattern": ["x\u{7f}"], "decision": "forbidden"},
                ]}}),
                1,
            ),
        ];
        for (vendors, index) in cases {
            assert_refused_alike(vendors, &SettingsError::RulesControlCharacter { index });
        }
    }

    #[test]
    fn rules_keep_match_and_not_match_off() {
        for key in ["match", "not_match"] {
            assert_unknown_field(
                json!({"codex": {"rules": [{"pattern": ["git"], "decision": "allow", (key): ["a"]}]}}),
                "codex",
                &format!("/rules/0/{key}"),
            );
        }
    }

    #[test]
    fn a_rule_pattern_is_a_flat_non_empty_list() {
        let at = "/rules/0/pattern";
        let cases = [
            (json!([]), "a non-empty list of strings"),
            (json!("git"), "a list of strings"),
            (json!([["a", "b"]]), "a list of strings"),
            (json!([1]), "a list of strings"),
        ];
        for (pattern, expected) in cases {
            assert_refused_alike(
                json!({"codex": {"rules": [{"pattern": pattern, "decision": "allow"}]}}),
                &wrong_type("codex", at, expected),
            );
        }
        assert_refused_alike(
            json!({"codex": {"rules": [{"decision": "allow"}]}}),
            &wrong_type("codex", at, "a list of strings"),
        );
    }

    #[test]
    fn a_rule_decision_is_a_closed_set() {
        for decision in ["allow", "prompt", "forbidden"] {
            accepted(json!({"codex": {"rules": [{"pattern": ["git"], "decision": decision}]}}));
        }
        assert_not_in_enum(
            json!({"codex": {"rules": [{"pattern": ["git"], "decision": "maybe"}]}}),
            "codex",
            "/rules/0/decision",
        );
        assert_refused_alike(
            json!({"codex": {"rules": [{"pattern": ["git"]}]}}),
            &wrong_type("codex", "/rules/0/decision", "a string"),
        );
    }

    #[test]
    fn a_rule_justification_is_a_string() {
        assert_refused_alike(
            json!({"codex": {"rules": [{"pattern": ["git"], "decision": "allow", "justification": 1}]}}),
            &wrong_type("codex", "/rules/0/justification", "a string"),
        );
    }

    #[test]
    fn a_rules_value_is_a_list_of_tables() {
        assert_refused_alike(
            json!({"codex": {"rules": "x"}}),
            &wrong_type("codex", "/rules", "a list of rules"),
        );
        assert_refused_alike(
            json!({"codex": {"rules": ["x"]}}),
            &wrong_type("codex", "/rules/0", "a table"),
        );
    }

    #[test]
    fn a_complete_rule_passes() {
        accepted(json!({"codex": {"rules": [
            {"pattern": ["git", "status"], "decision": "allow", "justification": "read only"},
        ]}}));
    }

    #[test]
    fn a_claude_marketplace_holds_a_github_source() {
        accepted(
            json!({"claude": {"extraKnownMarketplaces": {"m": {"source": {"source": "github", "repo": "acme/plugins"}}}}}),
        );
        accepted(
            json!({"claude": {"extraKnownMarketplaces": {"m": {"source": {"source": "github", "repo": "acme/plugins", "ref": "main"}}}}}),
        );
    }

    #[test]
    fn a_claude_marketplace_of_another_shape_is_refused() {
        let at = "/extraKnownMarketplaces/m/source";
        let marketplace = |entry: Value| json!({"claude": {"extraKnownMarketplaces": {"m": entry}}});

        assert_refused_alike(
            marketplace(json!({"source": "acme/plugins"})),
            &wrong_type("claude", at, "a table"),
        );
        assert_refused_alike(marketplace(json!({})), &wrong_type("claude", at, "a table"));
        assert_not_in_enum(
            marketplace(json!({"source": {"source": "url", "repo": "a/b"}})),
            "claude",
            "/extraKnownMarketplaces/m/source/source",
        );
        assert_refused_alike(
            marketplace(json!({"source": {"source": "github"}})),
            &wrong_type("claude", "/extraKnownMarketplaces/m/source/repo", "a string"),
        );
        assert_refused_alike(
            marketplace(json!({"source": {"source": "github", "repo": "a/b", "ref": 1}})),
            &wrong_type("claude", "/extraKnownMarketplaces/m/source/ref", "a string"),
        );
        assert_unknown_field(
            marketplace(json!({"source": {"source": "github", "repo": "a/b", "path": "x"}})),
            "claude",
            "/extraKnownMarketplaces/m/source/path",
        );
    }

    /// `/permission` and `/permission/<tool>` share a prefix, so the value's
    /// shape picks the row.
    #[test]
    fn opencode_permission_is_a_string_or_a_tool_table() {
        accepted(json!({"opencode": {"permission": "ask"}}));
        accepted(json!({"opencode": {"permission": {"bash": "allow", "read": {"*": "ask"}}}}));
        assert_refused_alike(
            json!({"opencode": {"permission": true}}),
            &wrong_type("opencode", "/permission", "a string"),
        );
        assert_not_in_enum(
            json!({"opencode": {"permission": "sometimes"}}),
            "opencode",
            "/permission",
        );
    }

    #[test]
    fn a_tool_outside_the_list_is_an_unknown_field() {
        assert_unknown_field(
            json!({"opencode": {"permission": {"nope": "allow"}}}),
            "opencode",
            "/permission/nope",
        );
    }

    /// One invalid value makes OpenCode reject its whole config.
    #[test]
    fn a_pattern_value_outside_the_set_is_refused() {
        assert_not_in_enum(
            json!({"opencode": {"permission": {"bash": {"*": "maybe"}}}}),
            "opencode",
            "/permission/bash/*",
        );
        assert_not_in_enum(
            json!({"opencode": {"permission": {"bash": "maybe"}}}),
            "opencode",
            "/permission/bash",
        );
        assert_refused_alike(
            json!({"opencode": {"permission": {"bash": {"*": 1}}}}),
            &wrong_type("opencode", "/permission/bash", "one of the accepted types"),
        );
    }

    #[test]
    fn a_placeholder_row_takes_any_key() {
        accepted(json!({"codex": {"permissions": {"any-name": {"description": "x"}}}}));
        accepted(json!({"copilot": {"extraKnownMarketplaces": {"other": {"source": "https://example.com/m"}}}}));
    }

    #[test]
    fn an_atomic_row_is_closed_at_its_first_level() {
        assert_unknown_field(
            json!({"codex": {"permissions": {"ci": {"nope": 1}}}}),
            "codex",
            "/permissions/ci/nope",
        );
    }

    /// The layer sorts keys and the last matching pattern wins, so an order
    /// the author chose cannot survive publication unchanged.
    #[test]
    fn an_unsorted_pattern_table_is_refused_at_author() {
        let standard = "description = \"x\"\n[opencode.permission.bash]\n\"git *\" = \"allow\"\n\"*\" = \"ask\"\n";
        let inline = "description = \"x\"\n[opencode.permission]\nbash = { \"git *\" = \"allow\", \"*\" = \"ask\" }\n";
        for raw in [standard, inline] {
            let SettingsError::Malformed { detail } = parsed(raw).unwrap_err() else {
                panic!("{raw}");
            };
            assert!(detail.contains("/opencode/permission/bash"), "{detail}");
            assert!(!detail.contains("git"), "{detail}");
        }
    }

    #[test]
    fn a_sorted_pattern_table_keeps_its_order_in_the_layer() {
        let raw = "description = \"x\"\n[opencode.permission.bash]\n\"*\" = \"ask\"\n\"git *\" = \"allow\"\n\"git push *\" = \"deny\"\n";
        let (document, _) = parsed(raw).unwrap();
        let text = layer_text(&document);
        let order: Vec<usize> = ["\"*\"", "\"git *\"", "\"git push *\""]
            .iter()
            .map(|key| text.find(key).unwrap())
            .collect();
        assert!(order.is_sorted(), "{text}");
    }

    /// A settings table can hold a token, so no refusal names a value.
    #[test]
    fn a_refusal_never_repeats_a_value() {
        let all = [
            parsed(&format!("description = \"x\"\n[claude]\nmodel = \"{MARKER}\n")).unwrap_err(),
            parsed(&format!("description = \"x\"\n[claude]\nmodel = {MARKER}\n")).unwrap_err(),
            parsed(&format!(
                "description = \"x\"\nrepository = \"http://{MARKER}.example\"\n"
            ))
            .unwrap_err(),
            parsed(&format!(
                "description = \"x\"\nrepository = \"https://u:{MARKER}@example.com\"\n"
            ))
            .unwrap_err(),
            SettingsDocument::from_layer_bytes("test", format!("{{\"description\": \"{MARKER}").as_bytes())
                .unwrap_err(),
            refusals(json!({"claude": {"effortLevel": MARKER}})).1,
            refusals(json!({"claude": {"spinnerTipsEnabled": MARKER}})).1,
            refusals(json!({"opencode": {"model": format!("{{env:{MARKER}}}")}})).1,
            refusals(
                json!({"codex": {"marketplaces": {"m": {"source": format!("https://u:{MARKER}@example.com/m")}}}}),
            )
            .1,
            refusals(json!({"codex": {"rules": [{"pattern": [format!("{MARKER}\n")], "decision": "allow"}]}})).1,
        ];
        for refusal in all {
            let shown = format!("{refusal} {refusal:?}");
            assert!(!shown.contains(MARKER), "{shown}");
        }
    }

    /// Registry text in a key reaches a terminal through the refusal, so it
    /// arrives screened and spelled as an RFC 6901 pointer.
    #[test]
    fn registry_keys_reach_a_refusal_screened() {
        let consumed =
            SettingsDocument::from_layer_bytes("test", &layer(json!({"new\u{1b}client": {"a": true}}))).unwrap();
        assert_eq!(
            consumed.validate(ValidationMode::Consume).unwrap(),
            [SettingsWarning::UnknownVendor {
                vendor: "new\\u{1b}client".to_string()
            }]
        );
        assert_unknown_field(json!({"claude": {"a/b~c\u{1b}": true}}), "claude", "/a~1b~0c\\u{1b}");
    }

    /// `visit` recurses, so the parser's limit is what bounds its stack.
    #[test]
    fn a_nesting_past_the_parser_limit_is_malformed() {
        let nested = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let raw = format!("{{\"description\": \"test\", \"claude\": {{\"model\": {nested}}}}}");
        let refusal = SettingsDocument::from_layer_bytes("test", raw.as_bytes()).unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");
    }

    #[test]
    fn the_cap_admits_its_own_size_on_both_paths() {
        let limit = usize::try_from(SETTINGS_LAYER_SIZE_LIMIT).unwrap();
        let refused = SettingsError::TooLarge {
            actual: SETTINGS_LAYER_SIZE_LIMIT + 1,
            limit: SETTINGS_LAYER_SIZE_LIMIT,
        };

        let wire = |padding: usize| layer(json!({"claude": {"model": "x".repeat(padding)}}));
        let frame = wire(0).len();
        assert_eq!(wire(limit - frame).len(), limit);
        SettingsDocument::from_layer_bytes("test", &wire(limit - frame)).unwrap();
        assert_eq!(
            SettingsDocument::from_layer_bytes("test", &wire(limit - frame + 1)).unwrap_err(),
            refused
        );

        let pretty = |padding: usize| document(json!({"claude": {"model": "x".repeat(padding)}}));
        let frame = pretty(0).to_layer_bytes().unwrap().len();
        assert_eq!(pretty(limit - frame).to_layer_bytes().unwrap().len(), limit);
        assert_eq!(pretty(limit - frame + 1).to_layer_bytes().unwrap_err(), refused);
    }

    #[test]
    fn text_that_does_not_parse_is_malformed() {
        let refusal = parsed("=").unwrap_err();
        assert!(matches!(refusal, SettingsError::Malformed { .. }), "{refusal:?}");
        for raw in [b"{".as_slice(), b"[]", b"\xff"] {
            let refusal = SettingsDocument::from_layer_bytes("test", raw).unwrap_err();
            assert!(
                matches!(refusal, SettingsError::Malformed { .. }),
                "{raw:?}: {refusal:?}"
            );
        }
    }

    /// One value `ty` accepts.
    fn sample(ty: SettingsValueType) -> Value {
        match ty {
            SettingsValueType::Bool => json!(true),
            SettingsValueType::String => json!("x"),
            SettingsValueType::Enum(allowed) => json!(allowed[0]),
            SettingsValueType::StringList => json!(["x"]),
            SettingsValueType::OneOf(arms) => sample(arms[0]),
            SettingsValueType::Atomic { keys } => json!({(keys[0]): "x"}),
            SettingsValueType::EnumMap(allowed) => json!({"*": allowed[0]}),
        }
    }

    /// One value `row` accepts, its two hand-checked shapes included.
    fn sample_for(client: ClientTarget, row: &SettingsField) -> Value {
        if row.merge == MergeClass::OwnFile {
            return json!([{"pattern": ["git"], "decision": "allow"}]);
        }
        if client == ClientTarget::Claude && MARKETPLACE_ROWS.contains(&row.pointer) {
            return json!({"source": {"source": "github", "repo": "acme/plugins"}});
        }
        sample(row.ty)
    }

    /// The table and the walker must agree, or a shipped key would be refused.
    #[test]
    fn every_row_of_every_vendor_accepts_a_value_of_its_type() {
        let mut checked = 0;
        for client in ClientTarget::ALL {
            for row in client.vendor().settings_fields() {
                let mut value = sample_for(client, row);
                for segment in row.pointer.trim_start_matches('/').rsplit('/') {
                    let key = if segment.starts_with('<') {
                        row.map_keys.map_or("x", |keys| keys[0])
                    } else {
                        segment
                    };
                    value = json!({(key): value});
                }
                accepted(json!({(client.as_str()): value}));
                checked += 1;
            }
        }
        assert!(checked > 50, "{checked}");
    }

    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
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

    /// What `f` logs at `warn` and above.
    fn warnings_of(f: impl FnOnce()) -> String {
        crate::log_switch::tracing_capture::arm();
        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink = std::sync::Arc::clone(&logs);
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(move || Captured(std::sync::Arc::clone(&sink)))
                .with_ansi(false)
                .without_time()
                .with_max_level(tracing::Level::WARN)
                .finish(),
        );
        f();
        drop(guard);
        String::from_utf8(logs.lock().expect("log buffer is never poisoned").clone()).expect("logs are UTF-8")
    }

    /// A consumed layer's table for a client this grim does not know is
    /// ignored, and the user is told which, in which package, since a newer
    /// grim would apply it.
    #[test]
    fn a_consumed_unknown_vendor_table_is_named_in_a_warning() {
        let logs = warnings_of(|| {
            SettingsDocument::from_layer_bytes("team\u{1b}", &layer(json!({"newclient": {"a": 1}}))).unwrap();
        });
        assert!(logs.contains("WARN"), "{logs}");
        assert!(
            logs.contains(
                "settings package 'team\\u{1b}' carries a table for 'newclient', a client this grim does not know; \
                 it is ignored, and a newer grim may apply it"
            ),
            "{logs}"
        );
        let known = warnings_of(|| {
            SettingsDocument::from_layer_bytes("team", &layer(json!({"zed": {"a": 1}}))).unwrap();
        });
        assert!(
            known.contains(
                "settings package 'team' carries a table for 'zed', which takes no settings from this grim; \
                 it is ignored, and a newer grim may apply it"
            ),
            "{known}"
        );
        assert!(!known.contains("does not know"), "{known}");
        let quiet = warnings_of(|| {
            SettingsDocument::from_layer_bytes("test", &layer(json!({"claude": {"model": "opus"}}))).unwrap();
        });
        assert_eq!(quiet, "", "a known table warns nothing");
    }

    /// The renderer's unknown-key hint keys on this wording, at both gates.
    #[test]
    fn an_unknown_setting_gets_the_unknown_key_hint() {
        for mode in [ValidationMode::Author, ValidationMode::Consume] {
            let message = unknown_field("claude", "/futureKey", mode).to_string();
            assert!(crate::error::unknown_key_hint(&message).is_some(), "{message}");
        }
    }

    /// A refused vendor table has the causes an unknown key has, a typo or a
    /// client only a newer grim knows, so it carries the same hint. A table
    /// for a client this grim knows names its own remedy and gets none.
    #[test]
    fn an_unknown_vendor_table_gets_the_unknown_key_hint() {
        let refusal = |vendors: Value| {
            document(vendors)
                .validate(ValidationMode::Author)
                .unwrap_err()
                .to_string()
        };
        let unknown = refusal(json!({"newclient": {"a": 1}}));
        assert!(crate::error::unknown_key_hint(&unknown).is_some(), "{unknown}");
        let known = refusal(json!({"zed": {"model": "a"}}));
        assert!(crate::error::unknown_key_hint(&known).is_none(), "{known}");
    }

    /// TOML splits a bare dotted key into tables, so a flat dotted key written
    /// bare is refused at its first segment; the refusal says to quote it.
    #[test]
    fn a_bare_dotted_key_is_told_to_quote_it() {
        let bare = parsed("description = \"d\"\n[kiro]\nchat.defaultModel = \"m\"\n").unwrap_err();
        assert_eq!(bare, unknown_field("kiro", "/chat", ValidationMode::Author));
        assert_eq!(
            bare.to_string(),
            "kiro has no setting at /chat; kiro keys are dotted names, so quote the whole key in TOML, \
             as in \"chat.defaultModel\" = …"
        );
        parsed("description = \"d\"\n[kiro]\n\"chat.defaultModel\" = \"m\"\n").unwrap();
        // Positive control: a key no row spells as a dotted prefix gets no quoting hint.
        let plain = unknown_field("kiro", "/nothing", ValidationMode::Author).to_string();
        assert_eq!(plain, "kiro has no setting at /nothing");
    }

    /// A client grim knows but writes no settings for is named as such.
    #[test]
    fn a_table_for_a_client_without_settings_says_so() {
        let refusal = document(json!({"zed": {"model": "a"}}))
            .validate(ValidationMode::Author)
            .unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "zed takes no settings from grim, so a package cannot carry a zed table"
        );
        let unknown = document(json!({"newclient": {"a": 1}}))
            .validate(ValidationMode::Author)
            .unwrap_err();
        assert_eq!(unknown.to_string(), "unknown vendor table newclient");
    }

    /// The refusal says what the token does and what to write instead.
    #[test]
    fn an_expansion_token_refusal_says_why_and_what_to_do() {
        let refusal = SettingsError::ExpansionToken {
            pointer: "/kilo/model".to_string(),
        };
        assert_eq!(
            refusal.to_string(),
            "setting /kilo/model holds an expansion token, which the harness replaces with an environment \
             variable or a file when it reads the file; write the value literally"
        );
    }

    /// Gemini reads `$` before a word character or a brace as a reference, at
    /// the end of a string as anywhere else.
    #[test]
    fn a_dollar_reference_is_a_dollar_before_a_word_character_or_a_brace() {
        for text in ["$a", "$Z", "$1", "$_", "${", "x$_y", "end $9"] {
            assert!(holds_dollar_reference(text), "{text}");
        }
        for text in ["", "$", "a $ b", "$-", "$.", "$$", "$}", "US$"] {
            assert!(!holds_dollar_reference(text), "{text}");
        }
    }

    /// git and curl end the authority at `/`, `?` or `#` only, so userinfo
    /// holding a backslash is still userinfo to them (CWE-522).
    #[test]
    fn a_marketplace_url_whose_userinfo_holds_a_backslash_is_refused() {
        for source in ["https://u\\:p@example.com/m", "https://u\\x@example.com/m"] {
            assert_refused_alike(
                json!({"codex": {"marketplaces": {"m": {"source": source}}}}),
                &SettingsError::CredentialInUrl {
                    pointer: "/codex/marketplaces/m/source".to_string(),
                },
            );
        }
    }

    /// The URL parser's own reading counts, so a WHATWG spelling with no `://`
    /// cannot carry userinfo past the check.
    #[test]
    fn a_marketplace_url_in_a_normalised_spelling_is_refused() {
        for source in [
            "https:u:p@example.com/m",
            "https:/u:p@example.com/m",
            "https:\\\\u:p@example.com/m",
            "HTTPS:token@example.com/m",
        ] {
            assert_refused_alike(
                json!({"codex": {"marketplaces": {"m": {"source": source}}}}),
                &SettingsError::CredentialInUrl {
                    pointer: "/codex/marketplaces/m/source".to_string(),
                },
            );
        }
    }
}
