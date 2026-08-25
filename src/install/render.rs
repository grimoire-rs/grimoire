// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! The per-vendor frontmatter projection engine.
//!
//! Lifts tool-namespaced `metadata` keys (`claude.<field>: "…"`) out of a
//! canonical, agentskills-pure artifact into the native typed frontmatter
//! each client actually reads. The canonical artifact stays spec-compliant
//! on the wire: `metadata` values are strings, tool capabilities are
//! namespaced keys inside it. The same projection runs for skills and for
//! rule `metadata`, against the registries each [`Vendor`] declares:
//!
//! - a known `<vendor>.<field>` key converts to the field's native YAML
//!   type and is emitted as a native top-level key;
//! - an unknown `<vendor>.<field>` key is a typo guard: **warn + drop**;
//! - a known key with an invalid literal is a hard [`RenderError`]
//!   (fails publish, never silently ships a broken value);
//! - a foreign-namespace key (`opencode.*` while rendering Claude) is
//!   dropped silently;
//! - plain metadata keys (no known tool prefix) pass through unchanged.
//!
//! This module is pure mechanics; vendor field knowledge lives in the
//! [`Vendor`] registries (`vendor_claude` / `vendor_opencode` /
//! `vendor_copilot`). The projection is deterministic: identical input
//! yields byte-identical output, so rendered files can be
//! integrity-hashed like any generated file.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::LazyLock;

use serde_yaml::Value;

use crate::skill::agent_frontmatter::ParsedAgent;
use crate::skill::rule_frontmatter::ParsedRule;
use crate::skill::{AgentFrontmatter, RuleFrontmatter, SkillFrontmatter};

use super::client_target::ClientTarget;
use super::vendor::{FieldType, KnownField, Vendor};

/// The known tool namespaces a `metadata` key may carry. Keys prefixed
/// with anything else (`vendor.x`) are plain metadata, not tool keys.
///
/// Derived from [`ClientTarget::ALL`] so a new vendor reserves its namespace
/// automatically — the one non-compile-forced per-vendor edit the old literal
/// required (`adr_vendor_wave_expansion.md` §4). A `dyn` trait call is not
/// const-evaluable, so this is a [`LazyLock`] rather than a `const`.
///
/// **A client reserves a namespace only if it is a real vendor.**
/// [`ClientTarget::Agents`] is excluded, and the reason is specific to that
/// name rather than general: the generic client is vendor-neutral by
/// definition, owns no metadata namespace, and lifts nothing (all three of its
/// field registries are empty) — and `agents` is an ordinary English word
/// likely to appear in skill metadata that has nothing to do with a vendor, so
/// reserving `agents.*` would strip genuine user data broadly. A name like
/// `cline.*` or `kilo.*` carries no such risk.
///
/// Reservation itself is **additive, not a break**: it is the shipped
/// behaviour every client already follows (`codex` and `antigravity` are the
/// released precedent), it is documented as policy in
/// `docs/src/vendor-metadata.md`, and the newly-reserved key is dropped with a
/// **warning that names it**, never silently. Drift is measured by hashing the
/// installed file against its recorded `content_hash`, never by re-rendering,
/// and grim only re-renders on force, a pin change, or a newly added client —
/// re-recording the hash in the same pass. So an untouched install never
/// falsely reports `Modified`.
static KNOWN_NAMESPACES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    ClientTarget::ALL
        .iter()
        .filter(|c| !matches!(c, ClientTarget::Agents))
        .map(|c| c.vendor().name())
        .collect()
});

/// A projection failure: a known namespaced key carries a literal that
/// cannot convert to the field's native type. Hard error — publish fails
/// rather than shipping a silently broken value.
#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    /// A known `<vendor>.<field>` key with an unconvertible value.
    ///
    /// Escaped on the attribute rather than at the four construction sites:
    /// `value` is unconstrained registry-fetched text and this reaches a
    /// terminal through `SkillErrorKind::MetadataInvalid`'s `#[source]` chain.
    /// `key` is a closed-set `<vendor>.<field>` today — escaped anyway, so a
    /// future field lookup that stops matching exactly cannot silently reopen
    /// the hole (the `ClientsInvalid::Duplicate` precedent).
    #[error("invalid value '{}' for metadata key '{}': expected {expected}", .value.escape_debug(), .key.escape_debug())]
    InvalidValue {
        /// The full namespaced metadata key (`claude.effort`).
        key: String,
        /// The offending string literal.
        value: String,
        /// Human-readable description of accepted literals.
        expected: String,
    },

    /// Serializing a rendered document to its native on-disk format failed.
    /// In practice unreachable for the flat string tables grim emits (Codex
    /// agent TOML) — surfaced rather than `.expect()`-panicked to keep
    /// library code free of panics across the render boundary.
    #[error("failed to serialize rendered {format} document")]
    Serialization {
        /// The target format (e.g. `TOML`).
        format: &'static str,
        /// The underlying serializer error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// The result of projecting a skill's frontmatter for one vendor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSkill {
    /// The re-serialized frontmatter YAML (no `---` fences).
    pub frontmatter_yaml: String,
    /// Typo-guard warnings (unknown `<vendor>.*` keys, override notes).
    pub warnings: Vec<String>,
}

/// A fully rendered document (skill or rule index) for one vendor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDoc {
    /// The complete rendered document.
    pub document: String,
    /// Typo-guard warnings from the projection.
    pub warnings: Vec<String>,
}

/// The generic projection of a metadata map for one vendor.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleProjection {
    /// The vendor's lifted `(native key, value)` pairs, registry order.
    pub lifted: Vec<(&'static str, Value)>,
    /// The frontmatter with every tool-namespaced metadata key removed
    /// (plain metadata, `paths`, and forward-compat extras preserved).
    pub cleaned: RuleFrontmatter,
    /// Typo-guard warnings (unknown own-namespace keys).
    pub warnings: Vec<String>,
    /// Whether any tool-namespaced key was present at all — `false` means
    /// a canonical-style vendor can install the source bytes verbatim.
    pub had_tool_keys: bool,
}

/// Whether `fm` carries any tool-namespaced metadata key — i.e. whether a
/// render would differ from the canonical bytes. `false` means the caller
/// can (and should) copy the file verbatim: byte-identical installs for
/// plain skills.
pub fn has_tool_namespaced_metadata(fm: &SkillFrontmatter) -> bool {
    fm.metadata.keys().any(|k| split_namespaced(k).is_some())
}

/// Whether `vendor_name` reserves a `<name>.` metadata prefix. False only for
/// the vendor-neutral `agents` client — see [`KNOWN_NAMESPACES`] for why.
///
/// Test-only: production code asks the question through [`split_namespaced`],
/// which needs the field half too. This exists so an invariant test can derive
/// its client set from the reservation policy instead of restating it.
#[cfg(test)]
pub fn reserves_namespace(vendor_name: &str) -> bool {
    KNOWN_NAMESPACES.contains(&vendor_name)
}

/// Split a metadata key into `(known_namespace, field)`; `None` when the
/// key has no known tool prefix (plain metadata).
fn split_namespaced(key: &str) -> Option<(&str, &str)> {
    let (ns, field) = key.split_once('.')?;
    KNOWN_NAMESPACES.contains(&ns).then_some((ns, field))
}

/// Convert a string metadata literal to the native YAML value for `ty`.
fn convert(key: &str, value: &str, ty: FieldType) -> Result<Value, RenderError> {
    match ty {
        FieldType::Bool => match value {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            other => Err(RenderError::InvalidValue {
                key: key.to_string(),
                value: other.to_string(),
                expected: "'true' or 'false'".to_string(),
            }),
        },
        FieldType::String => Ok(Value::String(value.to_string())),
        FieldType::Enum(allowed) => {
            if allowed.contains(&value) {
                Ok(Value::String(value.to_string()))
            } else {
                Err(RenderError::InvalidValue {
                    key: key.to_string(),
                    value: value.to_string(),
                    expected: format!("one of {}", allowed.join(", ")),
                })
            }
        }
        FieldType::Integer => {
            value
                .parse::<i64>()
                .map(|n| Value::Number(n.into()))
                .map_err(|_| RenderError::InvalidValue {
                    key: key.to_string(),
                    value: value.to_string(),
                    expected: "an integer".to_string(),
                })
        }
        FieldType::Float => match value.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(Value::Number(serde_yaml::Number::from(f))),
            _ => Err(RenderError::InvalidValue {
                key: key.to_string(),
                value: value.to_string(),
                expected: "a finite number".to_string(),
            }),
        },
        FieldType::CommaList => Ok(comma_list_value(value)),
    }
}

/// A comma-separated string as a native YAML sequence: segments trimmed,
/// empty segments dropped, input order kept. Deterministic, never fails.
pub fn comma_list_value(value: &str) -> Value {
    Value::Sequence(
        value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_string()))
            .collect(),
    )
}

/// Partition a metadata map for one vendor: plain keys into `plain`,
/// the vendor's own known keys converted into `lifted` (registry order),
/// own unknown keys into `warnings`, foreign tool keys dropped.
#[allow(clippy::type_complexity)]
fn partition_metadata(
    metadata: &std::collections::BTreeMap<String, String>,
    registry: &'static [KnownField],
    vendor_name: &str,
) -> Result<
    (
        std::collections::BTreeMap<String, String>,
        Vec<(&'static str, Value)>,
        Vec<String>,
        bool,
    ),
    RenderError,
> {
    let mut plain = std::collections::BTreeMap::new();
    let mut lifted: Vec<(&'static str, Value)> = Vec::new();
    let mut warnings = Vec::new();
    let mut had_tool_keys = false;

    for (key, value) in metadata {
        match split_namespaced(key) {
            None => {
                plain.insert(key.clone(), value.clone());
            }
            Some((ns, field)) if ns == vendor_name => {
                had_tool_keys = true;
                match registry.iter().find(|f| f.field == field) {
                    Some(known) => {
                        let converted = convert(key, value, known.ty)?;
                        lifted.push((known.native, converted));
                    }
                    None => {
                        // `escape_debug` renders the control byte as `\u{…}`;
                        // the raw byte never reaches stderr. Escaped for a
                        // reason `char::is_control` does not cover either: the
                        // bidi and zero-width format characters (U+202E,
                        // U+200B) are not control chars, so a hostile
                        // registry-published key reaches here intact.
                        // `vendor_name` is a closed-set `&'static str`.
                        warnings.push(format!(
                            "unknown metadata key '{}' for client '{vendor_name}': dropped (typo?)",
                            key.escape_debug()
                        ));
                    }
                }
            }
            // Foreign tool namespace: not for this vendor, drop silently.
            Some(_) => {
                had_tool_keys = true;
            }
        }
    }

    // Keep the registry's declared order regardless of BTreeMap iteration
    // order, so the emitted YAML is stable when fields are added.
    lifted.sort_by_key(|(native, _)| registry.iter().position(|f| f.native == *native).unwrap_or(usize::MAX));

    Ok((plain, lifted, warnings, had_tool_keys))
}

/// Project a skill's frontmatter for `target`: lift the vendor's
/// namespaced metadata keys into native typed top-level fields, drop
/// foreign-namespace keys, keep plain metadata, and re-serialize
/// deterministically.
///
/// # Errors
///
/// [`RenderError::InvalidValue`] when a known `<vendor>.<field>` key
/// carries a literal that does not convert to the field's native type.
pub fn project_skill(fm: &SkillFrontmatter, vendor: &dyn Vendor) -> Result<RenderedSkill, RenderError> {
    let (plain_metadata, lifted, mut warnings, _) =
        partition_metadata(&fm.metadata, vendor.skill_fields(), vendor.name())?;

    let mut plain = fm.clone();
    plain.metadata = plain_metadata;

    // Serialize the cleaned frontmatter, then append the lifted native
    // keys. serde_yaml's Mapping preserves insertion order, so the output
    // is: struct fields (declaration order), `extra` keys (BTreeMap
    // order), lifted keys (registry order) — fully deterministic.
    let mut mapping = to_mapping(&plain);
    append_lifted(&mut mapping, lifted, vendor.name(), &[], &mut warnings);

    Ok(RenderedSkill {
        frontmatter_yaml: serialize_mapping(&mapping),
        warnings,
    })
}

/// Project a rule's `metadata` map for `vendor`. Pure partition — the
/// vendor decides how (and whether) to emit the result.
///
/// # Errors
///
/// [`RenderError::InvalidValue`] for a known key with a bad literal.
pub fn project_rule(fm: &RuleFrontmatter, vendor: &dyn Vendor) -> Result<RuleProjection, RenderError> {
    let (plain_metadata, lifted, warnings, had_tool_keys) =
        partition_metadata(&fm.metadata, vendor.rule_fields(), vendor.name())?;
    let mut cleaned = fm.clone();
    cleaned.metadata = plain_metadata;
    Ok(RuleProjection {
        lifted,
        cleaned,
        warnings,
        had_tool_keys,
    })
}

/// The scope a `Degraded`-scoping client cannot express, said in prose so the
/// model self-gates on it instead of applying the rule everywhere.
///
/// A class-2 compensating render (`adr_vendor_support_tiers`): ordinary body
/// text, deterministic, and gone when the artifact is uninstalled. Empty
/// `paths` yields an empty string; otherwise a blockquote line plus the blank
/// line that keeps the author's body starting cleanly.
pub fn scope_notice(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let globs: Vec<String> = paths.iter().map(|p| format!("`{}`", code_span_safe(p))).collect();
    format!(
        "> Applies only when working on files matching {}.\n\n",
        globs.join(", ")
    )
}

/// Neutralize a publisher-supplied glob for the markdown code span it is
/// about to sit in: a backtick would close the span early and let the rest of
/// the value render as live markdown, and a control character (newline
/// included) would break the notice out of its single line. Both collapse to
/// a space.
///
/// Deliberately *not* `vendor::single_line`, which HTML-escapes `<`/`>` —
/// right inside an HTML comment, wrong inside a code span.
fn code_span_safe(glob: &str) -> Cow<'_, str> {
    if glob.chars().any(|c| c.is_control() || c == '`') {
        Cow::Owned(
            glob.chars()
                .map(|c| if c.is_control() || c == '`' { ' ' } else { c })
                .collect(),
        )
    } else {
        Cow::Borrowed(glob)
    }
}

/// The generic projection of an agent's metadata map for one vendor.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentProjection {
    /// The vendor's lifted `(native key, value)` pairs, registry order.
    pub lifted: Vec<(&'static str, Value)>,
    /// The frontmatter with every tool-namespaced metadata key removed
    /// (common fields, plain metadata, and forward-compat extras kept).
    pub cleaned: AgentFrontmatter,
    /// Typo-guard warnings (unknown own-namespace keys).
    pub warnings: Vec<String>,
    /// Whether any tool-namespaced key was present at all — `false` means
    /// a canonical-style vendor can install the source bytes verbatim.
    pub had_tool_keys: bool,
}

/// Project an agent's `metadata` map for `vendor`. Pure partition — the
/// vendor decides which common fields to emit and how (the per-vendor
/// emit matrix lives in the `agent_index` impls).
///
/// # Errors
///
/// [`RenderError::InvalidValue`] for a known key with a bad literal.
pub fn project_agent(fm: &AgentFrontmatter, vendor: &dyn Vendor) -> Result<AgentProjection, RenderError> {
    let (plain_metadata, lifted, warnings, had_tool_keys) =
        partition_metadata(&fm.metadata, vendor.agent_fields(), vendor.name())?;
    let mut cleaned = fm.clone();
    cleaned.metadata = plain_metadata;
    Ok(AgentProjection {
        lifted,
        cleaned,
        warnings,
        had_tool_keys,
    })
}

/// Render an agent document in **canonical style** for a vendor whose
/// native format equals the canonical one (Claude): `None` when the agent
/// carries no tool-namespaced metadata (verbatim install), else the
/// cleaned full frontmatter (own keys lifted — a collision on a native in
/// `expected_overrides` replaces the projected common field silently —
/// foreign keys dropped, plain metadata kept) re-serialized over the
/// verbatim body.
///
/// # Errors
///
/// [`RenderError::InvalidValue`] for a known key with a bad literal.
pub fn render_agent_canonical(
    parsed: &ParsedAgent,
    vendor: &dyn Vendor,
    expected_overrides: &[&str],
) -> Result<Option<RenderedDoc>, RenderError> {
    let projection = project_agent(&parsed.frontmatter, vendor)?;
    if !projection.had_tool_keys {
        return Ok(None);
    }

    let mut warnings = projection.warnings;
    let mut mapping = to_mapping(&projection.cleaned);
    append_lifted(
        &mut mapping,
        projection.lifted,
        vendor.name(),
        expected_overrides,
        &mut warnings,
    );

    let mut document = String::new();
    if !mapping.is_empty() {
        document.push_str("---\n");
        document.push_str(&serialize_mapping(&mapping));
        document.push_str("---\n");
    }
    document.push_str(&parsed.body);
    Ok(Some(RenderedDoc { document, warnings }))
}

/// Build an agent frontmatter block (`---\n…---\n`, or empty when nothing
/// is emitted) from explicit native `(key, value)` pairs plus the vendor's
/// lifted keys — the shared mechanics behind the transforming vendors'
/// `agent_index` impls (OpenCode, Copilot). Override-aware: a lifted key
/// whose native name is in `expected_overrides` replaces the projected
/// common pair silently; any other collision warns.
pub fn agent_frontmatter_block(
    natives: Vec<(&'static str, Value)>,
    lifted: Vec<(&'static str, Value)>,
    vendor_name: &str,
    expected_overrides: &[&str],
    warnings: &mut Vec<String>,
) -> String {
    let mut mapping = serde_yaml::Mapping::new();
    for (key, value) in natives {
        mapping.insert(Value::String(key.to_string()), value);
    }
    append_lifted(&mut mapping, lifted, vendor_name, expected_overrides, warnings);

    if mapping.is_empty() {
        return String::new();
    }
    let mut block = String::from("---\n");
    block.push_str(&serialize_mapping(&mapping));
    block.push_str("---\n");
    block
}

/// Render a rule index in **canonical style** for a vendor that reads the
/// canonical frontmatter natively (Claude): `None` when the rule carries
/// no tool-namespaced metadata (verbatim install), else the cleaned
/// frontmatter (own keys lifted, foreign keys dropped, plain keys kept)
/// re-serialized over the verbatim body.
///
/// # Errors
///
/// [`RenderError::InvalidValue`] for a known key with a bad literal.
pub fn render_rule_canonical(parsed: &ParsedRule, vendor: &dyn Vendor) -> Result<Option<RenderedDoc>, RenderError> {
    let projection = project_rule(&parsed.frontmatter, vendor)?;
    if !projection.had_tool_keys {
        return Ok(None);
    }

    let mut warnings = projection.warnings;
    let mut mapping = to_mapping(&projection.cleaned);
    append_lifted(&mut mapping, projection.lifted, vendor.name(), &[], &mut warnings);

    let mut document = String::new();
    if !mapping.is_empty() {
        document.push_str("---\n");
        document.push_str(&serialize_mapping(&mapping));
        document.push_str("---\n");
    }
    document.push_str(&parsed.body);
    Ok(Some(RenderedDoc { document, warnings }))
}

/// Render a full `SKILL.md` document for `target`, or `None` when the
/// canonical bytes should be installed verbatim: the document carries no
/// tool-namespaced metadata, or it does not parse as a skill at all (a
/// foreign artifact is copied untouched).
///
/// # Errors
///
/// [`RenderError::InvalidValue`] when a known `<vendor>.<field>` key
/// carries an unconvertible literal — never silently install a broken
/// projection.
pub fn render_skill_doc(doc: &str, vendor: &dyn Vendor) -> Result<Option<RenderedDoc>, RenderError> {
    let path = std::path::Path::new("SKILL.md");
    // Split once; `parse_doc` would re-run the same frontmatter scan.
    let Ok((fm_yaml, body)) = SkillFrontmatter::split(doc, path) else {
        return Ok(None);
    };
    let Ok(fm) = SkillFrontmatter::from_yaml(&fm_yaml, path) else {
        return Ok(None);
    };
    if !has_tool_namespaced_metadata(&fm) {
        return Ok(None);
    }
    let rendered = project_skill(&fm, vendor)?;

    let mut document = String::with_capacity(rendered.frontmatter_yaml.len() + body.len() + 8);
    document.push_str("---\n");
    document.push_str(&rendered.frontmatter_yaml);
    document.push_str("---\n");
    document.push_str(&body);
    Ok(Some(RenderedDoc {
        document,
        warnings: rendered.warnings,
    }))
}

/// Render a full `SKILL.md` document in the **universal** Agent-Skills shape:
/// every tool-namespaced metadata key (of any known vendor) is dropped,
/// nothing is lifted, and plain metadata / body survive unchanged. `None`
/// when the canonical bytes install verbatim (no tool-namespaced metadata, or
/// the document does not parse as a skill).
///
/// Vendor-independent **by construction**: no vendor field registry is
/// consulted here, so nothing per-vendor can leak into the ONE
/// `$HOME/.agents/skills/<name>` file every pool member writes.
///
/// **Its one production caller is [`ClientTarget::Agents`]**, the vendor-neutral
/// client that reserves no metadata namespace. Every other vendor — pool member
/// or not — routes `skill_index` through [`render_skill_doc`], because a vendor
/// that reserves a namespace must drop an unknown own-namespace key **by name**
/// and this renderer returns `warnings: Vec::new()` unconditionally, having no
/// vendor to attribute a warning to. That is a diagnostic difference only: with
/// an empty `skill_fields()` registry the two emit byte-identical documents
/// (`append_lifted` is a no-op on an empty lift list, and both keep exactly the
/// keys `split_namespaced` calls plain), and warnings never reach disk.
///
/// So this function is also the pool's **independent byte anchor**: the pool
/// identity tests compare each member's `.document` against it rather than
/// against each other, which is what keeps those assertions non-vacuous.
///
/// Infallible: with no vendor context and no field registry, no metadata
/// value is ever converted, so there is no [`RenderError`] path — the result
/// is a plain [`Option`].
pub fn render_universal_skill_doc(doc: &str) -> Option<RenderedDoc> {
    let path = std::path::Path::new("SKILL.md");
    let (fm_yaml, body) = SkillFrontmatter::split(doc, path).ok()?;
    let fm = SkillFrontmatter::from_yaml(&fm_yaml, path).ok()?;
    if !has_tool_namespaced_metadata(&fm) {
        return None;
    }
    // Drop every tool-namespaced key (nothing is lifted), keep plain metadata.
    // `split_namespaced` is the single source of truth for "is this a known
    // tool key" — the same predicate the vendor-aware partition uses — so the
    // output stays byte-identical to what an empty-registry vendor emits.
    let mut plain = fm.clone();
    plain.metadata = fm
        .metadata
        .iter()
        .filter(|(k, _)| split_namespaced(k).is_none())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let frontmatter_yaml = serialize_mapping(&to_mapping(&plain));
    let mut document = String::with_capacity(frontmatter_yaml.len() + body.len() + 8);
    document.push_str("---\n");
    document.push_str(&frontmatter_yaml);
    document.push_str("---\n");
    document.push_str(&body);
    Some(RenderedDoc {
        document,
        warnings: Vec::new(),
    })
}

/// Rewrite the frontmatter `name` of a skill document to `binding`, or
/// `None` when nothing needs rewriting: the names already agree, or the
/// document does not parse as a skill at all (a foreign artifact is
/// copied untouched).
///
/// Why: a `--name` rebinding installs under `skills/<binding>/`, and the
/// Agent Skills directory-equality rule (enforced at build time in
/// `skill_package`) requires the frontmatter `name` to equal that
/// directory name — without the rewrite, a rebound skill ships a stale
/// `name` that can collide with the original at the client level.
///
/// Operates on the raw frontmatter mapping so every other key — known,
/// metadata, unknown — survives; only the `name` value changes.
/// Deterministic (identical input yields identical output), so the
/// installer's untracked-clobber preview and the real install agree.
pub fn rebind_skill_name(doc: &str, binding: &str) -> Option<String> {
    let path = std::path::Path::new("SKILL.md");
    let (fm_yaml, body) = SkillFrontmatter::split(doc, path).ok()?;
    let fm = SkillFrontmatter::from_yaml(&fm_yaml, path).ok()?;
    if fm.name.as_str() == binding {
        return None;
    }
    let mut mapping: serde_yaml::Mapping = serde_yaml::from_str(&fm_yaml).ok()?;
    mapping.insert(Value::String("name".to_string()), Value::String(binding.to_string()));

    let mut document = String::with_capacity(doc.len() + 16);
    document.push_str("---\n");
    document.push_str(&serialize_mapping(&mapping));
    document.push_str("---\n");
    document.push_str(&body);
    Some(document)
}

/// Serialize a struct to a YAML mapping (a struct always serializes to a
/// mapping; the fallback keeps the arm total without panicking).
fn to_mapping<T: serde::Serialize>(value: &T) -> serde_yaml::Mapping {
    match serde_yaml::to_value(value) {
        Ok(Value::Mapping(m)) => m,
        Ok(_) | Err(_) => serde_yaml::Mapping::new(),
    }
}

/// Append lifted native keys to `mapping`, warning when a lifted key
/// overrides an existing (legacy top-level) key. The namespaced metadata
/// value always wins. A collision on a native named in
/// `expected_overrides` is **documented precedence** (a vendor key
/// overriding a projected common field, e.g. `claude.model` over `model`)
/// and replaces silently — no warning.
fn append_lifted(
    mapping: &mut serde_yaml::Mapping,
    lifted: Vec<(&'static str, Value)>,
    vendor_name: &str,
    expected_overrides: &[&str],
    warnings: &mut Vec<String>,
) {
    for (native, value) in lifted {
        let native_key = Value::String(native.to_string());
        if mapping.contains_key(&native_key) && !expected_overrides.contains(&native) {
            warnings.push(format!(
                "metadata key '{vendor_name}.{native}' overrides the top-level '{native}' frontmatter key"
            ));
        }
        mapping.insert(native_key, value);
    }
}

/// Serialize a YAML mapping to a deterministic string. `serde_yaml`
/// serialization of scalar/string/sequence values is itself
/// deterministic; this wrapper only exists to keep the unreachable error
/// arm in one place.
fn serialize_mapping(mapping: &serde_yaml::Mapping) -> String {
    serde_yaml::to_string(mapping).unwrap_or_else(|_| {
        // Serializing an in-memory mapping of plain values cannot fail;
        // return an empty document rather than panicking in library code.
        let mut s = String::new();
        let _ = writeln!(s, "{{}}");
        s
    })
}

/// Validate the namespaced metadata of a skill against **every** supported
/// target: a publish-time gate. Returns the union of per-target warnings
/// (deduplicated, in target order).
///
/// # Errors
///
/// The first [`RenderError`] from any target — a known key with a bad
/// literal must fail the publish before the artifact reaches a registry.
pub fn validate_namespaced_metadata(fm: &SkillFrontmatter) -> Result<Vec<String>, RenderError> {
    let mut warnings = Vec::new();
    for target in ClientTarget::ALL {
        let rendered = project_skill(fm, target.vendor())?;
        for w in rendered.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
    }
    // Migration nudge: a known tool-specific field authored as a top-level
    // frontmatter key (it landed in `extra`) should move into namespaced
    // metadata.
    for key in fm.extra.keys() {
        let claude_fields = ClientTarget::Claude.vendor().skill_fields();
        // Match either spelling (registry key or native key), but always
        // advise the canonical registry key — `field` and `native` diverge
        // for `when-to-use`, and only `claude.<field>` is recognized.
        if let Some(f) = claude_fields
            .iter()
            .find(|f| f.native == key.as_str() || f.field == key.as_str())
        {
            warnings.push(format!(
                "top-level frontmatter key '{key}' is not an agentskills field; author it as metadata 'claude.{}' instead",
                f.field
            ));
        }
    }
    Ok(warnings)
}

/// Validate an agent's tool-namespaced `metadata` keys against every
/// supported target: a publish-time gate. Returns the union of per-target
/// typo-guard warnings plus a migration nudge for vendor-namespaced keys
/// authored top-level (in `extra` — the modeled common fields `model` /
/// `tools` are legitimate top-level keys and never nudged).
///
/// # Errors
///
/// The first [`RenderError`] from any target — a known key with a bad
/// literal must fail the publish before the artifact reaches a registry.
pub fn validate_agent_metadata(fm: &AgentFrontmatter) -> Result<Vec<String>, RenderError> {
    let mut warnings = Vec::new();
    for target in ClientTarget::ALL {
        let projection = project_agent(fm, target.vendor())?;
        for w in projection.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
    }
    // Migration nudge: a tool-namespaced key authored top-level in the
    // agent frontmatter is never projected — it belongs inside `metadata`.
    for key in fm.extra.keys() {
        if split_namespaced(key).is_some() {
            // Escaped: `split_namespaced` constrains only the namespace half,
            // so the field half is arbitrary frontmatter text. Unlike the skill
            // nudge above, whose key must equal a registry field name.
            warnings.push(format!(
                "top-level agent frontmatter key '{}' is not projected; author it inside 'metadata' instead",
                key.escape_debug()
            ));
        }
    }
    Ok(warnings)
}

/// Validate a rule's tool-namespaced `metadata` keys against every
/// supported target: a publish-time gate. Returns typo-guard warnings
/// plus a migration nudge for vendor keys authored top-level.
///
/// # Errors
///
/// [`RenderError::InvalidValue`] for a known key with a bad literal
/// (today only `copilot.exclude-agent`).
pub fn validate_rule_metadata(fm: &RuleFrontmatter) -> Result<Vec<String>, RenderError> {
    let mut warnings = Vec::new();
    for target in ClientTarget::ALL {
        let projection = project_rule(fm, target.vendor())?;
        for w in projection.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
    }
    // Migration nudge: a tool-namespaced key authored top-level in the
    // rule frontmatter is never projected — it belongs inside `metadata`.
    for key in fm.extra.keys() {
        if split_namespaced(key).is_some() {
            // Escaped for the same reason as the agent nudge above.
            warnings.push(format!(
                "top-level rule frontmatter key '{}' is not projected; author it inside 'metadata' instead",
                key.escape_debug()
            ));
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fm(doc: &str) -> SkillFrontmatter {
        SkillFrontmatter::parse_doc(doc, Path::new("SKILL.md")).expect("parse")
    }

    // ── rebind_skill_name ──────────────────────────────────────────

    #[test]
    fn rebind_rewrites_only_the_name_key() {
        let doc =
            "---\nname: foo\ndescription: d\nlicense: MIT\nmetadata:\n  keywords: a,b\nfuture-key: kept\n---\n# body\n";
        let out = rebind_skill_name(doc, "bar").expect("mismatch must rebind");
        assert!(out.contains("name: bar"), "{out}");
        assert!(!out.contains("name: foo"), "{out}");
        assert!(out.contains("license: MIT"), "known key kept: {out}");
        assert!(out.contains("keywords: a,b"), "metadata kept: {out}");
        assert!(out.contains("future-key: kept"), "unknown key kept: {out}");
        assert!(out.ends_with("# body\n"), "body preserved: {out}");
    }

    #[test]
    fn rebind_is_none_when_names_agree() {
        let doc = "---\nname: foo\ndescription: d\n---\n";
        assert!(rebind_skill_name(doc, "foo").is_none());
    }

    #[test]
    fn rebind_is_none_for_a_foreign_document() {
        // No frontmatter / not a skill — copied untouched, never rewritten.
        assert!(rebind_skill_name("# plain markdown\n", "bar").is_none());
        assert!(rebind_skill_name("---\ndescription: no name\n---\n", "bar").is_none());
    }

    #[test]
    fn rebind_is_deterministic() {
        let doc = "---\nname: foo\ndescription: d\nmetadata:\n  keywords: a\n---\nbody\n";
        let a = rebind_skill_name(doc, "bar").unwrap();
        let b = rebind_skill_name(doc, "bar").unwrap();
        assert_eq!(a, b, "rebind must be byte-identical across runs");
    }

    fn rule(doc: &str) -> ParsedRule {
        RuleFrontmatter::parse_doc(doc, Path::new("r.md")).expect("parse")
    }

    const NAMESPACED: &str = r#"---
name: next
description: Suggest the next command.
metadata:
  keywords: workflow,planning
  claude.disable-model-invocation: "true"
  claude.model: opus
  opencode.future-flag: "x"
---
# body
"#;

    #[test]
    fn claude_lifts_native_typed_fields() {
        let r = project_skill(&fm(NAMESPACED), ClientTarget::Claude.vendor()).expect("render");
        // Native bool, not the string "true".
        assert!(r.frontmatter_yaml.contains("disable-model-invocation: true"));
        assert!(!r.frontmatter_yaml.contains("disable-model-invocation: 'true'"));
        assert!(r.frontmatter_yaml.contains("model: opus"));
        // The namespaced keys are gone from metadata; plain metadata stays.
        assert!(!r.frontmatter_yaml.contains("claude."));
        assert!(r.frontmatter_yaml.contains("keywords: workflow,planning"));
        // The foreign opencode key is dropped silently — no warning.
        assert!(!r.frontmatter_yaml.contains("future-flag"));
        assert!(r.warnings.is_empty(), "no warnings expected: {:?}", r.warnings);
    }

    #[test]
    fn opencode_render_is_clean_universal_with_warning_for_own_unknown_key() {
        let r = project_skill(&fm(NAMESPACED), ClientTarget::OpenCode.vendor()).expect("render");
        // No tool key survives; opencode's registry is empty so its own
        // namespaced key warns (typo guard).
        assert!(!r.frontmatter_yaml.contains("claude."));
        assert!(!r.frontmatter_yaml.contains("opencode."));
        assert!(!r.frontmatter_yaml.contains("disable-model-invocation"));
        assert!(r.frontmatter_yaml.contains("keywords: workflow,planning"));
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("opencode.future-flag"));
    }

    #[test]
    fn copilot_skill_render_matches_opencode_universal_shape() {
        // OpenCode and Copilot both read only universal fields: identical
        // rendered frontmatter (the unified universal render).
        let oc = project_skill(&fm(NAMESPACED), ClientTarget::OpenCode.vendor()).expect("render");
        let cp = project_skill(&fm(NAMESPACED), ClientTarget::Copilot.vendor()).expect("render");
        assert_eq!(oc.frontmatter_yaml, cp.frontmatter_yaml);
        assert!(cp.warnings.is_empty(), "foreign namespaces drop silently");
    }

    #[test]
    fn bad_bool_literal_is_render_error() {
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.user-invocable: \"yes\"\n---\n";
        let err = project_skill(&fm(doc), ClientTarget::Claude.vendor()).expect_err("bad bool");
        let msg = err.to_string();
        assert!(msg.contains("claude.user-invocable"), "{msg}");
        assert!(msg.contains("'true' or 'false'"), "{msg}");
    }

    #[test]
    fn bad_enum_literals_are_render_errors() {
        for (key, value) in [
            ("claude.effort", "ultra"),
            ("claude.context", "thread"),
            ("claude.shell", "zsh"),
        ] {
            let doc = format!("---\nname: s\ndescription: d\nmetadata:\n  {key}: \"{value}\"\n---\n");
            let err = project_skill(&fm(&doc), ClientTarget::Claude.vendor()).expect_err("bad enum");
            assert!(err.to_string().contains(key), "{err}");
        }
        // The valid literals pass.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.effort: xhigh\n  claude.context: fork\n  claude.shell: bash\n---\n";
        let r = project_skill(&fm(doc), ClientTarget::Claude.vendor()).expect("valid enums");
        assert!(r.frontmatter_yaml.contains("effort: xhigh"));
        assert!(r.frontmatter_yaml.contains("context: fork"));
        assert!(r.frontmatter_yaml.contains("shell: bash"));
    }

    #[test]
    fn unknown_target_key_warns_and_drops() {
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.modle: opus\n---\n";
        let r = project_skill(&fm(doc), ClientTarget::Claude.vendor()).expect("render");
        assert!(!r.frontmatter_yaml.contains("modle"));
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("claude.modle"));
    }

    /// The dropped key is escaped. It is **registry-fetched** skill metadata,
    /// and on the plain install path this warning reaches a terminal verbatim
    /// through `tracing::warn!` (only `--format json` escaped it, via serde).
    ///
    /// Both hostile shapes, because they need different guards: ESC drives the
    /// terminal, and U+202E is **not** `char::is_control`, so an ESC-only test
    /// would pass while the bidi case still shipped.
    ///
    /// Driven through `partition_metadata` rather than a YAML fixture so the
    /// raw code points reach the sink unaltered by the parser.
    #[test]
    fn an_unknown_target_key_is_escaped_in_the_warning() {
        let metadata =
            std::collections::BTreeMap::from([("claude.\u{1b}[2J\u{202e}evil".to_string(), "opus".to_string())]);
        let (_, _, warnings, _) =
            partition_metadata(&metadata, ClientTarget::Claude.vendor().skill_fields(), "claude").expect("partition");
        let w = warnings.first().expect("an unknown own-namespace key must warn");
        assert!(!w.contains('\u{1b}'), "raw ESC must not reach the terminal: {w:?}");
        assert!(!w.contains('\u{202e}'), "raw U+202E must not reach the terminal: {w:?}");
        assert!(
            w.contains(r"claude.\u{1b}[2J\u{202e}evil"),
            "the escaped key must still name what the author has to fix: {w:?}"
        );
    }

    /// The sibling sink of the test above, and the one that matters more: the
    /// *value* of a KNOWN key is unconstrained registry-fetched text, whereas
    /// the key is a closed-set `<vendor>.<field>`. `RenderError::InvalidValue`
    /// reaches a terminal through `SkillErrorKind::MetadataInvalid`, which
    /// carries it as `#[source]`, so `{err:#}` prints it.
    ///
    /// Escaping only the warning and leaving this raw is how the hole reopens.
    #[test]
    fn an_invalid_metadata_value_is_escaped_in_the_error() {
        let metadata =
            std::collections::BTreeMap::from([("claude.effort".to_string(), "\u{1b}[2J\u{202e}evil".to_string())]);
        let err = partition_metadata(&metadata, ClientTarget::Claude.vendor().skill_fields(), "claude")
            .expect_err("an unconvertible literal must be refused");
        let msg = err.to_string();
        assert!(!msg.contains('\u{1b}'), "raw ESC must not reach the terminal: {msg:?}");
        assert!(
            !msg.contains('\u{202e}'),
            "raw U+202E must not reach the terminal: {msg:?}"
        );
        assert!(
            msg.contains(r"\u{1b}[2J\u{202e}evil"),
            "the escaped literal must still name what the author has to fix: {msg:?}"
        );
    }

    #[test]
    fn when_to_use_lifts_to_native_underscore_key() {
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  claude.when-to-use: planning time\n---\n";
        let r = project_skill(&fm(doc), ClientTarget::Claude.vendor()).expect("render");
        assert!(r.frontmatter_yaml.contains("when_to_use: planning time"));
        assert!(!r.frontmatter_yaml.contains("when-to-use"));
    }

    #[test]
    fn render_is_deterministic_and_identity_detection_works() {
        let f = fm(NAMESPACED);
        let a = project_skill(&f, ClientTarget::Claude.vendor()).expect("render");
        let b = project_skill(&f, ClientTarget::Claude.vendor()).expect("render");
        assert_eq!(a, b, "re-render must be byte-identical");
        assert!(has_tool_namespaced_metadata(&f));

        let plain = fm("---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  vendor.x: y\n---\n");
        // `vendor.` is not a known tool namespace ⇒ plain metadata.
        assert!(!has_tool_namespaced_metadata(&plain));
    }

    // ── D1a: universal skill renderer / shared-pool invariant ─────────────

    #[test]
    fn generic_client_reserves_no_metadata_namespace() {
        // Principle 9: an `agents.*` metadata key predates the generic client
        // and must keep installing verbatim. If `agents` ever joined
        // KNOWN_NAMESPACES, such a key would be reclassified as tool-namespaced
        // and silently dropped, changing the rendered bytes of an already-
        // installed skill.
        assert!(
            !KNOWN_NAMESPACES.contains(&"agents"),
            "the generic client must not reserve a metadata namespace"
        );
        assert_eq!(
            KNOWN_NAMESPACES.len(),
            ClientTarget::ALL.len() - 1,
            "every client except the generic one reserves its vendor namespace"
        );
        // End to end: a plain `agents.*` key leaves the doc on the verbatim
        // fast path (no tool-namespaced metadata ⇒ `None` ⇒ install as-is).
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  agents.foo: bar\n---\nbody\n";
        assert!(
            render_universal_skill_doc(doc).is_none(),
            "an `agents.*` key must stay plain metadata and install verbatim"
        );
        for client in ClientTarget::ALL {
            assert!(
                client.vendor().skill_index(doc).expect("no error").is_none(),
                "{client} must leave a plain `agents.*` key on the verbatim path"
            );
        }
    }

    #[test]
    fn pool_vendors_declare_no_skill_fields() {
        // Every pool vendor resolves every skill to the ONE shared
        // `$HOME/.agents/skills/<name>` pool, so the pooled SKILL.md must stay
        // vendor-independent. Every pool vendor but `agents` routes `skill_index`
        // through the vendor-AWARE `render_skill_doc` (so an unknown
        // own-namespace key is dropped by name rather than silently), which means
        // a `skill_fields()` entry added to a pool vendor really would lift — and
        // that member's bytes would diverge from its siblings', invalidating
        // every recorded `content_hash` for the one shared file. This asserts the
        // registry stays empty, so that divergence cannot be introduced at all.
        use crate::config::scope::ConfigScope;

        // The pool roster, named rather than counted. `checked` below is
        // derived from `skills_root`, so deriving the expectation from
        // `skills_root` too would make this assertion vacuous — a vendor
        // silently LEAVING the pool would shrink both sides and still pass,
        // which is precisely the drift the old `assert_eq!(checked, 4)`
        // existed to catch. The roster is the independent anchor: it changes
        // only when a human decides membership changed, and naming the members
        // makes the failure say who joined or left instead of `5 != 4`.
        const POOL_ROSTER: &[ClientTarget] = &[
            ClientTarget::Codex,
            ClientTarget::Gemini,
            ClientTarget::Zed,
            ClientTarget::Amp,
            ClientTarget::Agents,
            // Antigravity pools at PROJECT scope only — its global skills live
            // under its own `~/.gemini/config/skills`. The invariant below
            // probes the project root, which is what makes it a member here.
            ClientTarget::Antigravity,
            // Goose renders INTO the pool at both scopes — upstream labels its
            // own `.goose/skills` back-compat and names `.agents/skills` the
            // recommended location. Warp is pool-*capable* but renders
            // natively, so it is deliberately NOT here: this roster is "who
            // writes the shared tree", not "who can read it".
            ClientTarget::Goose,
        ];

        let ws = Path::new("/w");
        let pool = Path::new(".agents/skills");
        let mut checked: Vec<&'static str> = Vec::new();
        for target in ClientTarget::ALL {
            let vendor = target.vendor();
            if vendor.skills_root(ws, ConfigScope::Project).ends_with(pool) {
                assert!(
                    vendor.skill_fields().is_empty(),
                    "pool vendor '{}' shares `.agents/skills` — it must declare no skill_fields (the shared SKILL.md is vendor-independent)",
                    vendor.name()
                );
                checked.push(vendor.name());
            }
        }
        let expected: Vec<&'static str> = POOL_ROSTER.iter().map(|c| c.vendor().name()).collect();
        assert_eq!(
            checked, expected,
            "the set of vendors resolving into .agents/skills must equal the declared pool roster — \
             a vendor joining or leaving the pool is a deliberate change, not a silent one"
        );
    }

    #[test]
    fn pool_vendors_render_byte_identical_skill_bytes() {
        // The four shared-pool vendors write to ONE `.agents/skills/<name>`
        // file, so their `skill_index` must emit byte-identical bytes for the
        // same source. Proven on a fixture carrying own- and foreign-namespace
        // vendor keys (which forces the render path off the verbatim fast path).
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  claude.model: opus\n  codex.reasoning-effort: high\n---\n# body\n";
        let pool = [
            ClientTarget::Codex,
            ClientTarget::Gemini,
            ClientTarget::Zed,
            ClientTarget::Amp,
        ];
        let rendered: Vec<String> = pool
            .iter()
            .map(|c| {
                c.vendor()
                    .skill_index(doc)
                    .expect("render")
                    .expect("namespaced metadata ⇒ rendered")
                    .document
            })
            .collect();
        for w in rendered.windows(2) {
            assert_eq!(w[0], w[1], "pool vendors must emit byte-identical skill bytes");
        }
        // The vendor-less universal renderer agrees with every pool vendor.
        let universal = render_universal_skill_doc(doc)
            .expect("namespaced metadata ⇒ rendered")
            .document;
        assert_eq!(
            rendered[0], universal,
            "pool skill_index must equal the universal render"
        );
        // Every tool-namespaced key is stripped; plain metadata survives.
        assert!(
            !universal.contains("claude.") && !universal.contains("codex."),
            "no tool-namespaced key survives: {universal}"
        );
        assert!(universal.contains("keywords: a,b"), "plain metadata kept: {universal}");
        // THE GOLDEN LITERAL. Every assertion above is relative — each renderer
        // is compared against another renderer, and both sides share
        // `SkillFrontmatter::split`, `from_yaml`, `to_mapping`,
        // `serialize_mapping` and the fence concat. A change to any one of those
        // moves BOTH sides together, so every relative assertion still passes
        // while the pool file's bytes silently change and every member's
        // recorded `content_hash` is invalidated. This literal is the only
        // tripwire for that class. Update it only when the byte change is
        // intended, and treat an "unrelated" edit that trips it as the finding.
        assert_eq!(
            universal, "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n---\n# body\n",
            "the shared-pool document bytes changed"
        );
    }

    #[test]
    fn every_pool_capable_vendor_renders_the_universal_skill_bytes() {
        // The test above covers the vendors that render into the pool by
        // default. `[options.vendors.<name>].shared_skills` lets a user move
        // ANY pool-capable client in there — cursor, copilot, opencode — onto
        // the same one `.agents/skills/<name>` directory, where every member
        // records its own `content_hash` against the same bytes. So the
        // identity has to hold for the whole capability roster, not just the
        // default members, and it is where a future `cursor.*` skill field
        // would break first.
        //
        // Derived from `pool_capable` rather than a literal list, so a vendor
        // added to the roster is covered without editing this.
        //
        // What it actually catches, since the two routes differ: a vendor
        // declaring `skill_fields` cannot reach here at all — `pool_capable`
        // ANDs an empty registry, so that drift is refused upstream. What
        // survives is a vendor whose `skill_index` OVERRIDE does something
        // other than delegate to `render_skill_doc` (prepend a provenance
        // header, rewrite the body — `rule_index` already varies that way).
        // Mutation-proven against exactly that.
        let doc = "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n  claude.model: opus\n  codex.reasoning-effort: high\n---\n# body\n";
        let universal = render_universal_skill_doc(doc)
            .expect("namespaced metadata ⇒ rendered")
            .document;
        //
        // `agents` is on the roster and its assertion is a TAUTOLOGY — its
        // `skill_index` IS `render_universal_skill_doc`, so it compares a
        // function to itself. Named rather than skipped: a reader who spots it
        // and "fixes" the test by dropping the member removes the guard that
        // catches the day `AgentsVendor` stops delegating.
        let mut checked = 0usize;
        for client in ClientTarget::ALL {
            if !client.vendor().pool_capable() {
                continue;
            }
            checked += 1;
            let rendered = client
                .vendor()
                .skill_index(doc)
                .expect("render")
                .expect("namespaced metadata ⇒ rendered")
                .document;
            assert_eq!(
                rendered,
                universal,
                "'{}' is pool-capable, so shared_skills can put its skills in the same \
                 directory as every other member — its render must be byte-identical to \
                 the universal one, or opting it in silently rewrites its siblings' bytes \
                 and invalidates their recorded content_hash",
                client.vendor().name()
            );
        }
        assert!(
            checked >= 8,
            "the capability roster shrank unexpectedly ({checked} clients)"
        );
    }

    #[test]
    fn switching_to_the_vendor_aware_renderer_moved_no_document_bytes() {
        // These five shipped with `skill_index` calling `render_universal_skill_doc`
        // directly, which cannot warn — an own-namespace key vanished with no
        // diagnostic. They now call `render_skill_doc`, and this is the proof
        // that the switch changed diagnostics ONLY: for every document shape,
        // the vendor-aware render must equal what the universal renderer emits,
        // which is byte-for-byte what these vendors wrote before the switch.
        //
        // It matters more here than for any other renderer change: all five
        // write the ONE shared `.agents/skills/<name>/SKILL.md`, and a single
        // byte of drift invalidates every sibling client's recorded
        // `content_hash` for that file. The named list is historical — it is the
        // set that switched, not a set derived from any current predicate — so
        // it is deliberately not derived from `pool_capable` (which does not
        // even reach Antigravity).
        const SWITCHED: &[ClientTarget] = &[
            ClientTarget::Amp,
            ClientTarget::Antigravity,
            ClientTarget::Codex,
            ClientTarget::Gemini,
            ClientTarget::Zed,
        ];
        // Chosen for the boundaries a divergence could hide behind, not for
        // volume. The one that earned its place: entry 1 puts a PLAIN dotted key
        // (`vendor.x` — unknown prefix, so not a tool key) beside a reserved one.
        // A mutation that drifts only on `doc.contains("agents.")` passed the
        // whole install suite before this entry existed, because nothing
        // exercised the line between "dotted but plain" and "dotted and
        // reserved". Entry 2 routes an unknown top-level key through the
        // `extra` bucket, the one part of the frontmatter neither renderer
        // touches but both re-serialize.
        let corpus = [
            // Own-namespace key: the ONLY place the two renderers differ at all
            // (one warns, one does not), so the byte claim rests here.
            "---\nname: s\ndescription: d\nmetadata:\n  codex.made-up: x\n---\n# body\n",
            // Plain dotted keys + a reserved one + an undotted plain key,
            // together. `agents.foo` is the sharp case: `agents` IS a client
            // name, deliberately left unreserved, so it must survive while
            // `codex.made-up` next to it is dropped.
            "---\nname: s\ndescription: d\nmetadata:\n  agents.foo: keep-me\n  vendor.x: keep-me-too\n  codex.made-up: drop-me\n  keywords: a,b\n---\n# body\n",
            // Unknown top-level frontmatter key ⇒ `extra`, re-emitted by both.
            "---\nname: s\ndescription: d\nfuture_field: kept\nmetadata:\n  zed.made-up: x\n---\n# body\n",
            // Several reserved namespaces at once, and key ordering: `zzz`/`aaa`
            // prove the emitted order comes from the map, not from input order.
            "---\nname: s\ndescription: d\nmetadata:\n  zzz: last\n  amp.a: 1\n  antigravity.b: 2\n  gemini.c: 3\n  aaa: first\n---\nbody\n",
            // Unicode in key and value, and a body with no trailing newline.
            "---\nname: s\ndescription: d\nmetadata:\n  gemini.wränkel: „quoted“ ✓\n---\n# ünïcode body",
            // Foreign namespace only — dropped silently by both, no warning.
            "---\nname: s\ndescription: d\nmetadata:\n  claude.model: opus\n---\n# body\n",
            // Verbatim fast path (no tool key) and a non-skill document: both
            // renderers must agree on `None` too, since `None` installs the
            // canonical bytes and `Some` installs generated ones.
            "---\nname: s\ndescription: d\nmetadata:\n  keywords: a,b\n---\n# body\n",
            "# not a skill at all\n",
        ];
        let mut rendered_shapes = 0usize;
        for doc in corpus {
            let expected = render_universal_skill_doc(doc).map(|d| d.document);
            rendered_shapes += usize::from(expected.is_some());
            for client in SWITCHED {
                let vendor = client.vendor();
                let actual = vendor
                    .skill_index(doc)
                    .map(|o| o.map(|d| d.document))
                    // Not `unwrap`: with an empty `skill_fields()` registry the
                    // only fallible call in `render_skill_doc` is unreachable.
                    // If a registry is ever populated this must be revisited,
                    // not silently unwrapped.
                    .unwrap_or_else(|e| panic!("'{}' must not error on an empty registry: {e}", vendor.name()));
                assert_eq!(
                    actual,
                    expected,
                    "'{}' must emit the universal pool bytes unchanged — this is the \
                     shared `.agents/skills` file, and one byte of drift invalidates every \
                     sibling's content_hash. Document:\n{doc}",
                    vendor.name()
                );
            }
        }
        // A corpus of `None == None` pairs would assert nothing. Only the
        // entries that actually render carry signal, so their count is pinned.
        assert!(
            rendered_shapes >= 6,
            "too few corpus entries reach the render path ({rendered_shapes}) — the rest compare None to None"
        );
    }

    #[test]
    fn universal_render_is_none_for_a_plain_or_foreign_skill() {
        // No tool-namespaced metadata ⇒ verbatim install (None), like the
        // vendor-aware path. A `vendor.x` key is plain (unknown namespace).
        assert!(
            render_universal_skill_doc(
                "---\nname: s\ndescription: d\nmetadata:\n  keywords: a\n  vendor.x: y\n---\nbody\n"
            )
            .is_none()
        );
        // Not a skill at all ⇒ copied untouched.
        assert!(render_universal_skill_doc("# plain markdown\n").is_none());
    }

    #[test]
    fn rendered_skill_doc_reparses() {
        let doc = render_skill_doc(NAMESPACED, ClientTarget::Claude.vendor())
            .expect("render")
            .expect("namespaced metadata present");
        assert!(doc.document.starts_with("---\n"));
        let again = SkillFrontmatter::parse_doc(&doc.document, Path::new("SKILL.md")).expect("reparse");
        assert_eq!(again.name.as_str(), "next");
        // Plain skill ⇒ identity.
        assert!(
            render_skill_doc(
                "---\nname: s\ndescription: d\n---\nbody\n",
                ClientTarget::Claude.vendor()
            )
            .expect("render")
            .is_none()
        );
    }

    #[test]
    fn top_level_override_warns_but_namespaced_wins() {
        // `model` authored top-level (lands in extra) AND namespaced.
        let doc = "---\nname: s\ndescription: d\nmodel: haiku\nmetadata:\n  claude.model: opus\n---\n";
        let r = project_skill(&fm(doc), ClientTarget::Claude.vendor()).expect("render");
        assert!(r.frontmatter_yaml.contains("model: opus"));
        assert!(!r.frontmatter_yaml.contains("haiku"));
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("overrides"));
    }

    #[test]
    fn validate_namespaced_metadata_unions_warnings_and_fails_on_bad_literal() {
        let ok = fm(NAMESPACED);
        let warnings = validate_namespaced_metadata(&ok).expect("valid");
        // opencode.future-flag is unknown for opencode (its registry is
        // empty) ⇒ exactly one warning.
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("opencode.future-flag"));

        let bad = fm("---\nname: s\ndescription: d\nmetadata:\n  claude.effort: warp\n---\n");
        assert!(validate_namespaced_metadata(&bad).is_err());
    }

    #[test]
    fn validate_lints_legacy_top_level_claude_keys() {
        let legacy = fm("---\nname: s\ndescription: d\nuser-invocable: true\n---\n");
        let warnings = validate_namespaced_metadata(&legacy).expect("valid");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("claude.user-invocable"), "{:?}", warnings);
    }

    #[test]
    fn migration_nudge_suggests_canonical_registry_key_not_native_spelling() {
        // `when_to_use` is the *native* spelling; the registry key is
        // `when-to-use`. The nudge must advise the key the renderer
        // actually knows, or following it lands in the typo-drop path.
        let legacy = fm("---\nname: s\ndescription: d\nwhen_to_use: planning\n---\n");
        let warnings = validate_namespaced_metadata(&legacy).expect("valid");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("claude.when-to-use"), "{:?}", warnings);
    }

    // ── Agent projection ─────────────────────────────────────────────────

    fn agent(doc: &str) -> ParsedAgent {
        crate::skill::AgentFrontmatter::parse_doc(doc, Path::new("rev.md")).expect("parse")
    }

    #[test]
    fn comma_list_trims_and_drops_empties_in_input_order() {
        let v = comma_list_value(" b , a ,, c,");
        let Value::Sequence(items) = v else { panic!("sequence") };
        let strs: Vec<&str> = items.iter().filter_map(|i| i.as_str()).collect();
        assert_eq!(strs, vec!["b", "a", "c"], "input order kept, empties dropped");
        assert_eq!(comma_list_value(""), Value::Sequence(vec![]));
    }

    #[test]
    fn integer_and_float_conversion_and_errors() {
        assert_eq!(
            convert("k", "12", FieldType::Integer).unwrap(),
            Value::Number(12.into())
        );
        assert!(convert("k", "12.5", FieldType::Integer).is_err());
        assert!(convert("k", "0.25", FieldType::Float).is_ok());
        assert!(convert("k", "NaN", FieldType::Float).is_err(), "non-finite rejected");
        assert!(convert("k", "inf", FieldType::Float).is_err());
        assert!(convert("k", "warm", FieldType::Float).is_err());
    }

    #[test]
    fn project_agent_partitions_and_detects_tool_keys() {
        let p = agent(
            "---\nname: rev\ndescription: d\nmodel: sonnet\nmetadata:\n  keywords: a,b\n  claude.memory: project\n  copilot.tools: \"x\"\n---\nbody\n",
        );
        let claude = project_agent(&p.frontmatter, ClientTarget::Claude.vendor()).expect("project");
        assert!(claude.had_tool_keys);
        assert_eq!(claude.lifted.len(), 1, "only the claude key lifts for claude");
        assert_eq!(claude.lifted[0].0, "memory");
        assert_eq!(
            claude.cleaned.metadata.get("keywords").map(String::as_str),
            Some("a,b"),
            "plain metadata survives the clean"
        );
        assert!(!claude.cleaned.metadata.contains_key("claude.memory"));

        let plain = agent("---\nname: rev\ndescription: d\n---\nbody\n");
        let proj = project_agent(&plain.frontmatter, ClientTarget::Claude.vendor()).expect("project");
        assert!(!proj.had_tool_keys, "no tool keys ⇒ verbatim-capable");
    }

    #[test]
    fn render_agent_canonical_verbatim_and_override_paths() {
        // No tool keys ⇒ None (verbatim).
        let plain = agent("---\nname: rev\ndescription: d\nmodel: sonnet\n---\nbody\n");
        assert!(
            render_agent_canonical(&plain, ClientTarget::Claude.vendor(), &["model", "tools"])
                .expect("render")
                .is_none()
        );

        // claude.model overrides the common model — silently (expected).
        let over = agent("---\nname: rev\ndescription: d\nmodel: sonnet\nmetadata:\n  claude.model: opus\n---\nbody\n");
        let out = render_agent_canonical(&over, ClientTarget::Claude.vendor(), &["model", "tools"])
            .expect("render")
            .expect("tool keys present");
        assert!(out.document.contains("model: opus"));
        assert!(!out.document.contains("sonnet"));
        assert!(
            out.warnings.is_empty(),
            "expected override is silent: {:?}",
            out.warnings
        );

        // A collision NOT in expected_overrides still warns (extra key).
        let legacy =
            agent("---\nname: rev\ndescription: d\nmemory: user\nmetadata:\n  claude.memory: project\n---\nbody\n");
        let out = render_agent_canonical(&legacy, ClientTarget::Claude.vendor(), &["model", "tools"])
            .expect("render")
            .expect("rendered");
        assert!(out.document.contains("memory: project"), "namespaced wins");
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("overrides"));
    }

    #[test]
    fn agent_frontmatter_block_is_deterministic_and_override_aware() {
        let natives = vec![
            ("description", Value::String("d".to_string())),
            ("model", Value::String("sonnet".to_string())),
        ];
        let lifted = vec![("model", Value::String("anthropic/claude-sonnet-4-5".to_string()))];
        let mut warnings = Vec::new();
        let a = agent_frontmatter_block(natives.clone(), lifted.clone(), "opencode", &["model"], &mut warnings);
        assert!(a.starts_with("---\n") && a.ends_with("---\n"));
        assert!(a.contains("model: anthropic/claude-sonnet-4-5"));
        assert!(!a.contains("sonnet\n"), "common value replaced");
        assert!(warnings.is_empty(), "{warnings:?}");
        let b = agent_frontmatter_block(natives, lifted, "opencode", &["model"], &mut warnings);
        assert_eq!(a, b, "re-render byte-identical");
        // Empty mapping ⇒ no block at all.
        assert_eq!(
            agent_frontmatter_block(vec![], vec![], "opencode", &[], &mut warnings),
            ""
        );
    }

    #[test]
    fn validate_agent_metadata_unions_and_nudges() {
        // Unknown own-namespace key warns; common top-level fields never nudge.
        let ok = agent(
            "---\nname: rev\ndescription: d\nmodel: sonnet\ntools: Read\nmetadata:\n  opencode.future: \"x\"\n---\nbody\n",
        );
        let warnings = validate_agent_metadata(&ok.frontmatter).expect("valid");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("opencode.future"));

        // Bad literal fails the gate.
        let bad = agent("---\nname: rev\ndescription: d\nmetadata:\n  claude.effort: warp\n---\nbody\n");
        assert!(validate_agent_metadata(&bad.frontmatter).is_err());

        // Vendor key authored top-level: migration nudge.
        let legacy = agent("---\nname: rev\ndescription: d\nclaude.memory: project\n---\nbody\n");
        let warnings = validate_agent_metadata(&legacy.frontmatter).expect("valid");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("author it inside 'metadata'"));
    }

    /// The agent and rule migration nudges gate on `split_namespaced(key)`,
    /// which constrains only the namespace half — `claude.<anything>` passes
    /// and `key` comes straight from `fm.extra`. Unlike the skill nudge, whose
    /// `key` must equal a registry field name exactly, these two are wide open.
    ///
    /// Reachable under a plain `grim install` through `pack_local_artifact`
    /// (`src/skill/local_pack.rs`) on the path-dependency path, so a cloned
    /// repo delivers it — the `state.json` threat model.
    ///
    /// ESC and U+202E together: an upstream layer already escapes ESC here
    /// while leaving the bidi override intact, which is exactly how an
    /// ESC-only test ships the hole.
    #[test]
    fn top_level_namespaced_key_nudges_are_escaped() {
        let hostile = "\"claude.\\e[2J\\u202Eevil\": x\n";
        let clean = |w: &str| !w.contains('\u{1b}') && !w.contains('\u{202e}');

        let a = agent(&format!("---\nname: rev\ndescription: d\n{hostile}---\nbody\n"));
        let aw = validate_agent_metadata(&a.frontmatter).expect("valid");
        assert!(aw.iter().all(|w| clean(w)), "agent nudge leaked raw bytes: {aw:?}");

        let r = rule(&format!("---\nname: r\ndescription: d\n{hostile}---\nbody\n"));
        let rw = validate_rule_metadata(&r.frontmatter).expect("valid");
        assert!(rw.iter().all(|w| clean(w)), "rule nudge leaked raw bytes: {rw:?}");

        // The escaped key must still name what the author has to fix.
        assert!(
            aw.iter()
                .chain(&rw)
                .any(|w| w.contains(r"claude.\u{1b}[2J\u{202e}evil"))
        );
    }

    #[test]
    fn validate_agent_metadata_rejects_bad_wave1_literals() {
        // The publish gate projects against EVERY target, so a bad literal in
        // any wave-1 vendor's typed registry fails the gate — not just Claude:
        // cursor.readonly is a bool, gemini.temperature a float, gemini.max-turns
        // an int.
        for doc in [
            "---\nname: rev\ndescription: d\nmetadata:\n  cursor.readonly: \"maybe\"\n---\nbody\n",
            "---\nname: rev\ndescription: d\nmetadata:\n  gemini.temperature: \"warm\"\n---\nbody\n",
            "---\nname: rev\ndescription: d\nmetadata:\n  gemini.max-turns: \"many\"\n---\nbody\n",
        ] {
            assert!(
                validate_agent_metadata(&agent(doc).frontmatter).is_err(),
                "bad literal must fail the publish gate: {doc}"
            );
        }
    }

    // ── Rule projection ──────────────────────────────────────────────────

    #[test]
    fn plain_rule_is_identity_for_canonical_vendor() {
        let parsed = rule("---\npaths: [\"**/*.rs\"]\nkeywords: rust\n---\n# R\nbody\n");
        let out = render_rule_canonical(&parsed, ClientTarget::Claude.vendor()).expect("render");
        assert!(out.is_none(), "no tool-namespaced metadata ⇒ verbatim");
    }

    #[test]
    fn rule_with_foreign_vendor_key_renders_cleaned_for_claude() {
        let parsed = rule(
            "---\npaths: [\"**/*.rs\"]\nkeywords: rust\nmetadata:\n  copilot.exclude-agent: code-review\n---\n# R\nbody\n",
        );
        let out = render_rule_canonical(&parsed, ClientTarget::Claude.vendor())
            .expect("render")
            .expect("tool keys present ⇒ rendered");
        // Foreign vendor key dropped; canonical scoping + plain keys kept.
        assert!(!out.document.contains("copilot.exclude-agent"));
        assert!(out.document.contains("**/*.rs"));
        assert!(out.document.contains("keywords: rust"));
        assert!(out.document.ends_with("# R\nbody\n"));
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        // Deterministic re-render.
        let again = render_rule_canonical(&parsed, ClientTarget::Claude.vendor())
            .expect("render")
            .expect("rendered");
        assert_eq!(out, again);
    }

    #[test]
    fn rule_with_only_foreign_metadata_and_no_other_frontmatter_drops_the_block() {
        let parsed = rule("---\nmetadata:\n  copilot.exclude-agent: code-review\n---\nbody\n");
        let out = render_rule_canonical(&parsed, ClientTarget::Claude.vendor())
            .expect("render")
            .expect("rendered");
        assert_eq!(out.document, "body\n", "empty frontmatter block is omitted");
    }

    #[test]
    fn unknown_own_namespace_rule_key_warns_for_claude() {
        let parsed = rule("---\nmetadata:\n  claude.unknown-thing: x\n---\nbody\n");
        let out = render_rule_canonical(&parsed, ClientTarget::Claude.vendor())
            .expect("render")
            .expect("rendered");
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("claude.unknown-thing"));
        assert!(!out.document.contains("unknown-thing"));
    }

    #[test]
    fn validate_rule_metadata_checks_all_vendors_and_lints_top_level_keys() {
        // Valid metadata key for copilot ⇒ no error; unknown key warns once.
        let ok = rule("---\nmetadata:\n  copilot.exclude-agent: cloud-agent\n  claude.foo: x\n---\nbody\n");
        let warnings = validate_rule_metadata(&ok.frontmatter).expect("valid");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("claude.foo"));

        // Bad literal fails.
        let bad = rule("---\nmetadata:\n  copilot.exclude-agent: everything\n---\nbody\n");
        assert!(validate_rule_metadata(&bad.frontmatter).is_err());

        // Vendor key authored top-level: migration nudge.
        let legacy = rule("---\ncopilot.exclude-agent: code-review\n---\nbody\n");
        let warnings = validate_rule_metadata(&legacy.frontmatter).expect("valid");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("author it inside 'metadata'"));
    }

    // ── scope_notice: the class-2 compensating render ──

    #[test]
    fn scope_notice_states_every_glob_and_ends_with_a_blank_line() {
        assert_eq!(
            scope_notice(&["**/*.rs".to_string()]),
            "> Applies only when working on files matching `**/*.rs`.\n\n"
        );
        assert_eq!(
            scope_notice(&["**/*.rs".to_string(), "**/*.toml".to_string()]),
            "> Applies only when working on files matching `**/*.rs`, `**/*.toml`.\n\n"
        );
    }

    #[test]
    fn scope_notice_is_empty_for_an_unscoped_rule() {
        assert_eq!(scope_notice(&[]), "", "nothing scoped ⇒ nothing to restate");
    }

    #[test]
    fn scope_notice_neutralizes_a_glob_that_would_escape_its_code_span() {
        // A publisher-supplied path cannot close the span and continue as
        // live markdown, nor break the notice onto a second line.
        let out = scope_notice(&["a`b\nc".to_string()]);
        assert_eq!(out, "> Applies only when working on files matching `a b c`.\n\n");
        assert_eq!(out.trim_end().lines().count(), 1, "one line, not two: {out:?}");
    }

    #[test]
    fn scope_notice_is_deterministic() {
        let paths = ["**/*.rs".to_string(), "b/**".to_string()];
        assert_eq!(scope_notice(&paths), scope_notice(&paths));
    }
}
