// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Multi-registry resolution: the ordered browse set and qualified
//! `alias/repo` reference expansion.
//!
//! Two pure functions sit above [`Identifier`] and the single
//! `default_registry` precedence chain (`command::resolve_default_registry`):
//!
//! - [`resolve_registries`] builds the ordered, deduped set of registries a
//!   browse/search spans (`search`/`tui`/`mcp`). Only the `--registry` flag
//!   collapses the set — to exactly the flag's registries (repeatable /
//!   comma-separated, first is primary); `$GRIM_DEFAULT_REGISTRY` is a tier-3
//!   fallback (folded in only when no `[[registries]]` are declared).
//! - [`resolve_reference`] expands a user reference: an explicit registry
//!   parses strictly, a qualified `alias/repo` substitutes the alias's url,
//!   and a bare short id expands against the primary registry.
//!
//! The `alias/repo` form is collision-safe: [`Identifier::parse`] already
//! rejects a bare `alias/repo` with `MissingRegistry`, so alias substitution
//! only rescues inputs that fail today. The colon form `alias:repo` is never
//! interpreted as a qualified reference — it collides with `repo:tag`.

use crate::config::declaration::RegistryConfig;
use crate::config::registry_filter::RegistryFilter;
use crate::oci::Identifier;
use crate::oci::identifier::error::IdentifierError;

/// How a resolved browse source lists its packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An OCI registry — listing via the `/v2/_catalog` endpoint.
    Registry,
    /// An HTTP(S) package index serving compiled static files (`all.json`).
    IndexHttp,
    /// A git repository holding `index/**/metadata.json`.
    IndexGit,
}

impl SourceKind {
    /// Whether this source is a package index (any transport).
    pub fn is_index(self) -> bool {
        matches!(self, Self::IndexHttp | Self::IndexGit)
    }
}

/// Classify an `index` locator into its transport, or `None` when the
/// locator matches neither form (validation rejects those at parse time).
///
/// Git forms are checked first so `https://host/repo.git` clones rather
/// than being read as a static-file base.
pub fn classify_index(locator: &str) -> Option<SourceKind> {
    let l = locator.trim();
    if l.is_empty() {
        return None;
    }
    if l.starts_with("git+") || l.starts_with("ssh://") || l.starts_with("git@") || l.ends_with(".git") {
        return Some(SourceKind::IndexGit);
    }
    if l.starts_with("http://") || l.starts_with("https://") {
        return Some(SourceKind::IndexHttp);
    }
    None
}

/// Dedup key for a declared locator (issue #28): trim whitespace, strip
/// trailing slashes, and case-fold the scheme + host portion. The path
/// stays case-sensitive (OCI namespaces and index paths are identity).
/// Used only as the `seen` key — the stored url stays as first-declared.
fn normalize_locator(locator: &str) -> String {
    let trimmed = locator.trim().trim_end_matches('/');
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, trimmed),
    };
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let mut out = String::new();
    if let Some(s) = scheme {
        out.push_str(&s.to_ascii_lowercase());
        out.push_str("://");
    }
    out.push_str(&host.to_ascii_lowercase());
    if let Some(p) = path {
        out.push('/');
        out.push_str(p);
    }
    out
}

/// Trim trailing slashes off a declared locator on its way into
/// [`ResolvedRegistry::url`]. `oci = "ghcr.io/acme/"` passes validation, and
/// the stored url is what every consumer string-compares or concatenates
/// against — the browse candidate, `tui::tree`'s configured-prefix split,
/// the credential lookup, alias substitution. Normalizing once, here, is
/// what keeps those answers consistent; each consumer re-normalizing on its
/// own is how the filter and the tree came to disagree about the same row.
///
/// Only the trailing slashes: [`normalize_locator`] also case-folds the
/// host, and that stays a dedup-key concern — the stored url is identity
/// the user reads back out of `grim context`.
fn trim_locator(locator: &str) -> &str {
    locator.trim_end_matches('/')
}

/// One browse source in the resolved set, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRegistry {
    /// The registry host (and optional namespace) — or, for an index
    /// source, the index locator.
    pub url: String,
    /// The configured alias, when one was declared.
    pub alias: Option<String>,
    /// Whether this is the primary registry short identifiers expand
    /// against. Exactly one entry in a resolved set carries it.
    pub is_default: bool,
    /// How this source lists its packages (`_catalog` vs package index).
    pub kind: SourceKind,
    /// This entry's compiled browse filter (plan C-004/C-009).
    ///
    /// **Browse/display only — never consulted by reference resolution,
    /// lock, or install.** It rides into the resolution path as inert data
    /// (`ResolvedRegistry` is a field of [`crate::fetch::FetchScope`], which
    /// `resolve_reference` consumes), so the prohibition belongs on the
    /// field, not only on `registry_filter`'s module doc.
    ///
    /// Never `Option`: an entry with no authored `include`/`exclude` compiles to
    /// a filter whose `matches` is unconditionally `true` (plan C-004,
    /// `registry_filter_both_empty_matches_everything`), so "unfiltered" is
    /// already representable by the value — wrapping it in `Option` would
    /// let the same state exist two ways (`None` vs `Some(empty)`), which
    /// is exactly what `PartialEq`/`Eq` (derived here, compared throughout
    /// `src/tui/app.rs`'s tests) would then have to treat as unequal. The
    /// `--registry`-forced branch below carries the same empty filter as
    /// its existing `alias: None`, and every consumer (`grim context`
    /// reading the patterns back, `load_catalog`'s per-row filter) can call
    /// straight through with no `Option`-unwrapping at the call site.
    pub filter: RegistryFilter,
}

impl ResolvedRegistry {
    /// This entry's injective root identity (C-023) — what names its root in
    /// the TUI tree.
    ///
    /// A one-line delegation to [`row_source_of`], the single source of truth
    /// it shares with `CatalogGroup::key`, so the two cannot drift. Returns
    /// only `Alias` or `Locator`: `Local` and `Unattributed` exist for
    /// `TuiRow.source` and are unreachable from a configured entry.
    pub(crate) fn key(&self) -> RowSource {
        row_source_of(self.alias.as_deref(), &self.url)
    }
}

/// Which browse source a row is attributed to — the injective root identity
/// two entries at one locator need (C-022).
///
/// **Injective on `(alias, locator)` by construction**, in both directions:
///
/// - *Across variants* — `Alias { .. }`, `Locator("acme.example")` and
///   `Local` are distinct values, so no `PartialEq`, `Ord` or `Hash`
///   comparison can merge an alias with another entry's locator or with the
///   `Local` sentinel.
/// - *Within `Alias`* — it carries the whole dedup pair, so two entries that
///   survive [`resolve_registries`] differ in at least one component and
///   therefore in the value.
///
/// Nothing validates either collision away: rejecting an alias that equals
/// another entry's locator would narrow an input that parses on a released
/// build (Principle 9).
///
/// `PartialOrd`/`Ord` are derived for container convenience only. No consumer
/// sorts a `RowSource` today, and the derived order follows variant
/// declaration order — `Local < Alias < Locator < Unattributed` — which is
/// **arbitrary and must not be relied on**. It is not `registry_order`'s
/// precedence, and a `BTreeMap<RowSource, _>` keyed on it would silently
/// group every aliased entry ahead of every unaliased one.
///
/// "Its locator" is [`ResolvedRegistry::url`] exactly as stored —
/// [`trim_locator`]-trimmed at construction, never case- or slash-folded. It
/// is deliberately *not* [`normalize_locator`]'s form: that is the `seen`
/// dedup key, a different concept (C-029), and the two must not be unified.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RowSource {
    /// The synthetic local / dev-record group. Not a configured entry.
    Local,
    /// A configured entry that declared an alias. Carries the locator too:
    /// one alias may be declared at two locators across scopes (S-022b), and
    /// both survive [`resolve_registries`]' `(normalize_locator, alias)`
    /// dedup — so the alias alone does not identify the entry.
    Alias { alias: String, locator: String },
    /// A configured entry that declared no alias, identified by its locator.
    Locator(String),
    /// No source attribution — the row re-attributes by longest configured prefix.
    Unattributed,
}

impl RowSource {
    /// Render into the tree's existing `String`-keyed space (`Node` keys,
    /// `registry_labels`, `registry_order` all stay `String`, so no tree
    /// refactor). The tags keep the rendering injective too, which is what
    /// those maps need:
    ///
    /// | Variant | Rendering |
    /// |---|---|
    /// | `Local` | `"Local"` — the literal, unchanged |
    /// | `Alias { alias, locator }` | `"alias:{alias}/{locator}"` |
    /// | `Locator(l)` | `"locator:{l}"` |
    /// | `Unattributed` | `""` — never reaches the tree |
    ///
    /// **`/` is a sound separator, not a guess:**
    /// [`crate::config::project_config::validate_registries`] rejects an
    /// alias containing `/`, so the first `/` after the `alias:` tag is
    /// unambiguously the boundary — the alias half stays recoverable, and no
    /// `(alias, locator)` pair can spell another pair's rendering.
    ///
    /// **A tagged key must never be displayed** — that is a requirement on
    /// the consumer, not a property of this function. `TuiState::registry_label`
    /// must strip the tag on a lookup miss (E-10.1): an `alias:` key splits at
    /// the **first** `/` — never the last, since a locator contains `/` and an
    /// alias cannot — and the miss path rebuilds `"{alias} ({locator})"`,
    /// byte-identical to what `registry_labels` produces on a hit. A `locator:`
    /// key strips its tag and returns the locator unchanged.
    pub(crate) fn root_key(&self) -> String {
        match self {
            Self::Local => "Local".to_string(),
            Self::Alias { alias, locator } => format!("alias:{alias}/{locator}"),
            Self::Locator(locator) => format!("locator:{locator}"),
            Self::Unattributed => String::new(),
        }
    }
}

/// The single constructor both `key()` producers delegate to (C-022) — an
/// [`RowSource::Alias`] carrying both halves when an alias was declared, a
/// [`RowSource::Locator`] otherwise.
///
/// **Precondition:** `Some("")` is unreachable from a parsed config —
/// [`crate::config::project_config::validate_registries`] rejects an empty or
/// untrimmed alias at load — so there is no branch for it here.
pub(crate) fn row_source_of(alias: Option<&str>, locator: &str) -> RowSource {
    match alias {
        Some(alias) => RowSource::Alias {
            alias: alias.to_string(),
            locator: locator.to_string(),
        },
        None => RowSource::Locator(locator.to_string()),
    }
}

/// Build the ordered, deduped registry browse set.
///
/// Precedence:
/// 1. The `--registry` flag(s) (`forced`) collapse the set to exactly those
///    registries, in order and deduped (first is primary), preserving the
///    historical "flag-registries are the only ones searched" behavior. Only
///    the flag collapses; `$GRIM_DEFAULT_REGISTRY` (`env_default`) is a
///    default for tier 3, not a collapse trigger.
/// 2. Otherwise the declared `[[registries]]` are authoritative — project
///    entries then global entries, deduped by normalized locator **and**
///    alias (trailing slashes and host case ignored; first occurrence wins).
///    Two entries naming one locator under two aliases are two views of that
///    source and both browse; only a genuine repeat — same alias, same
///    locator — collapses, which is how a project entry shadows its global
///    twin.
///    Exactly one is marked primary: the first `default = true`, else the
///    first entry. When `[[registries]]` is non-empty the `env_default` is
///    NOT added to the browse set (it stays the short-id default only).
/// 3. When no `[[registries]]` are declared anywhere, the legacy single
///    `default_registry` chain applies: `env_default`
///    (`$GRIM_DEFAULT_REGISTRY`) then project, then global, then the
///    built-in `fallback`.
pub fn resolve_registries(
    forced: &[String],
    project: &[RegistryConfig],
    project_default: Option<&str>,
    global: &[RegistryConfig],
    global_default: Option<&str>,
    fallback: &str,
    env_default: Option<&str>,
) -> Vec<ResolvedRegistry> {
    // 1. The --registry flag(s) force exactly those registries, in order and
    // deduped (first is primary). The env var is NOT a collapse trigger — it
    // is folded in only at tier 3 below.
    let mut forced_set: Vec<ResolvedRegistry> = Vec::new();
    let mut forced_seen = std::collections::BTreeSet::new();
    for url in forced.iter().filter(|s| !s.is_empty()) {
        if forced_seen.insert(normalize_locator(url)) {
            forced_set.push(ResolvedRegistry {
                url: trim_locator(url).to_string(),
                alias: None,
                is_default: forced_set.is_empty(),
                kind: SourceKind::Registry,
                filter: RegistryFilter::default(),
            });
        }
    }
    if !forced_set.is_empty() {
        return forced_set;
    }

    // 2. Declared `[[registries]]` are authoritative when present. Each
    // entry is either a registry (`url`) or an index (`index`) source —
    // validation enforces exactly-one-of; an unclassifiable programmatic
    // index locator degrades to the HTTP transport rather than panicking.
    let mut out: Vec<ResolvedRegistry> = Vec::new();
    // An entry is identified by its normalized locator **and its alias** —
    // what the user wrote, not where they wrote it. Two entries naming one
    // locator under two aliases are two *views* of that source (a wide one
    // beside a narrow one, which is the whole point of per-entry browse
    // filters) and both browse, in either file or across them. The key
    // deliberately excludes the filters, so a field added to `RegistryConfig`
    // later cannot quietly change what counts as a duplicate.
    //
    // What still collapses is a genuine repeat: the same alias at the same
    // locator, which is the shape `grim init` writes when it snapshots an
    // index the global config already declares (issue #28 — an alias-less
    // entry over an alias-less one). First occurrence wins, so a project
    // entry shadows its global twin and an alias stays exactly one source.
    //
    // `load_catalog` fans its refresh out per distinct *locator*, not per
    // entry, so sibling views share one cache read and never race each other
    // for the advisory lock (which on a cold cache would serve one of them an
    // empty catalog).
    let mut seen: std::collections::BTreeSet<(String, Option<String>)> = std::collections::BTreeSet::new();
    for rc in project.iter().chain(global.iter()) {
        let (locator, kind) = match (&rc.oci, &rc.index) {
            (Some(oci), _) if !oci.trim().is_empty() => (oci.clone(), SourceKind::Registry),
            // Classified on the *trimmed* locator, the same string the stored
            // `url` is built from below: `index.git/` must not store as a git
            // locator while being served over the static-file transport.
            // `locator` itself stays authored — the two warnings below name
            // the entry the user has to find in their own file.
            (_, Some(index)) if !index.trim().is_empty() => (
                index.clone(),
                classify_index(trim_locator(index)).unwrap_or(SourceKind::IndexHttp),
            ),
            // Invalid entry (validation rejects it on parsed configs) —
            // skip rather than fabricate an empty source.
            _ => continue,
        };
        if seen.insert((normalize_locator(&locator), rc.alias.clone())) {
            // Compiled *here*, at resolve time, so `globset` stays inside
            // `src/config/` and every consumer (search, TUI, MCP,
            // `grim context`) receives an already-compiled filter.
            //
            // The failure arm exists because this function returns `Vec`,
            // not `Result` — the error has nowhere to propagate, so the only
            // choice is which way to fail. It fails *open*, per C-008/D11:
            // both lists are dropped and the entry is browsed unfiltered,
            // rather than dropping the registry, which would read to the
            // user as a broken source.
            //
            // A parsed config cannot reach it: `validate_registries` (plan
            // C-006) exits 78 first, and it validates by calling the same
            // `compile_set` this constructor uses — per pattern *and* once
            // over each whole list, so neither a single bad pattern nor a
            // list that only fails as a set can answer differently here.
            // What is still reachable is a programmatically-built
            // `RegistryConfig`, which never passed through validation.
            let filter = RegistryFilter::new(&rc.include, &rc.exclude).unwrap_or_else(|reason| {
                tracing::warn!(
                    // Escaped: an alias is authored TOML content on its way
                    // to a terminal, and `validate_registries` rejects only
                    // `char::is_control` — U+202E and the rest of the
                    // Unicode format characters reach this line intact.
                    "registry '{}': invalid include/exclude pattern ({reason}); browsing without a filter",
                    rc.alias.as_deref().unwrap_or(&locator).escape_debug()
                );
                RegistryFilter::default()
            });
            out.push(ResolvedRegistry {
                url: trim_locator(&locator).to_string(),
                alias: rc.alias.clone(),
                is_default: rc.default,
                kind,
                filter,
            });
        } else if !rc.include.is_empty() || !rc.exclude.is_empty() {
            // Same alias, same locator, so the entry and the one that won are
            // the same source — the only thing this one declared that the
            // winner does not carry is its browse filter, which silently
            // stops applying. Same voice as the sibling arm above, escaped
            // for the same reason.
            //
            // Gated on that one observable loss. A repeat that names no
            // filter is **deliberately unwarned**: layering a project entry
            // over an identical global one is a legitimate setup, and warning
            // on it would fire on every browse for no recoverable reason. An
            // entry that merely shares a locator no longer reaches here at
            // all — it is a second view and it browses.
            tracing::warn!(
                "registry '{}': repeats an earlier entry for '{}'; ignoring its include/exclude filter",
                rc.alias.as_deref().unwrap_or(&locator).escape_debug(),
                locator.escape_debug()
            );
        }
    }
    if !out.is_empty() {
        normalize_primary(&mut out);
        return out;
    }

    // 3. Legacy single default registry: env > project > global > fallback.
    // The env var is a default (tier 3) and is NOT consulted when
    // `[[registries]]` are declared (tier 2 wins above). Configured values
    // are always registries; only the BUILT-IN fallback may be an index
    // locator (the public package index) — classify it so an unconfigured
    // browse lists through the index instead of an empty `_catalog`.
    if let Some(url) = env_default
        .or(project_default)
        .or(global_default)
        .filter(|s| !s.is_empty())
    {
        return vec![ResolvedRegistry {
            url: trim_locator(url).to_string(),
            alias: None,
            is_default: true,
            kind: SourceKind::Registry,
            filter: RegistryFilter::default(),
        }];
    }
    vec![ResolvedRegistry {
        url: trim_locator(fallback).to_string(),
        alias: None,
        is_default: true,
        kind: classify_index(trim_locator(fallback)).unwrap_or(SourceKind::Registry),
        filter: RegistryFilter::default(),
    }]
}

/// Ensure exactly one entry is marked primary: keep the first `is_default`,
/// clear the rest; if none is marked, promote the first entry.
fn normalize_primary(registries: &mut [ResolvedRegistry]) {
    let first_default = registries.iter().position(|r| r.is_default);
    let primary = first_default.unwrap_or(0);
    for (i, r) in registries.iter_mut().enumerate() {
        r.is_default = i == primary;
    }
}

/// The primary registry's url for short-id expansion: the `is_default`
/// entry when it is a *registry* source, else the first registry-kind
/// entry, else the empty string. Index sources never expand short ids —
/// their locator is not a registry host — so a set holding only index
/// sources yields `""` (parsing then surfaces the missing registry).
pub fn primary_registry(registries: &[ResolvedRegistry]) -> &str {
    registries
        .iter()
        .find(|r| r.is_default && r.kind == SourceKind::Registry)
        .or_else(|| registries.iter().find(|r| r.kind == SourceKind::Registry))
        .map(|r| r.url.as_str())
        .unwrap_or("")
}

/// Expand a user reference into a fully-qualified [`Identifier`].
///
/// - A qualified `alias/repo[...]` whose leading `/`-segment matches a
///   configured alias substitutes that alias's url as the registry.
/// - Any other input expands against the primary registry exactly as a
///   single-registry `add` does today: an explicit registry parses
///   strictly, a bare short id gets the primary registry injected.
///
/// `fallback_primary` supplies the short-id registry when the set has no
/// registry-kind entry at all (an index-only `[[registries]]` — index
/// locators are not registry hosts). Callers pass the documented short-id
/// default chain (`--registry` > `$GRIM_DEFAULT_REGISTRY` > config
/// `default_registry` > the built-in fallback).
///
/// # Errors
///
/// Propagates [`IdentifierError`] for malformed input (invalid characters,
/// bad digest, uppercase repo, traversal segments).
pub fn resolve_reference(
    input: &str,
    registries: &[ResolvedRegistry],
    fallback_primary: &str,
) -> Result<Identifier, IdentifierError> {
    if let Some((first, rest)) = input.split_once('/')
        && !rest.is_empty()
        && let Some(reg) = registries.iter().find(|r| r.alias.as_deref() == Some(first))
    {
        // Substitute the alias's url as an explicit registry, then parse
        // strictly (the substituted form always carries a registry).
        let substituted = format!("{}/{}", reg.url, rest);
        return Identifier::parse(&substituted);
    }
    let primary = match primary_registry(registries) {
        "" => fallback_primary,
        p => p,
    };
    Identifier::parse_with_default_registry(input, primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc(alias: Option<&str>, oci: &str, default: bool) -> RegistryConfig {
        RegistryConfig {
            alias: alias.map(str::to_string),
            oci: Some(oci.to_string()),
            index: None,
            default,
            ..Default::default()
        }
    }

    fn rc_index(alias: Option<&str>, index: &str, default: bool) -> RegistryConfig {
        RegistryConfig {
            alias: alias.map(str::to_string),
            oci: None,
            index: Some(index.to_string()),
            default,
            ..Default::default()
        }
    }

    // ── Index-source classification and resolution ──────────────────────────

    #[test]
    fn classify_index_detects_transports() {
        assert_eq!(classify_index("https://index.grimoire.rs"), Some(SourceKind::IndexHttp));
        assert_eq!(classify_index("http://localhost:8080"), Some(SourceKind::IndexHttp));
        assert_eq!(
            classify_index("https://github.com/acme/index.git"),
            Some(SourceKind::IndexGit)
        );
        assert_eq!(
            classify_index("git+https://github.com/acme/index"),
            Some(SourceKind::IndexGit)
        );
        assert_eq!(
            classify_index("git@github.com:acme/index.git"),
            Some(SourceKind::IndexGit)
        );
        assert_eq!(classify_index("ssh://git@host/index"), Some(SourceKind::IndexGit));
        assert_eq!(
            classify_index("ghcr.io/acme"),
            None,
            "a bare registry url is not an index"
        );
        assert_eq!(classify_index(""), None);
    }

    #[test]
    fn index_entry_resolves_with_index_kind() {
        let set = resolve_registries(
            &[],
            &[
                rc_index(Some("hub"), "https://index.grimoire.rs", true),
                rc(Some("corp"), "registry.corp/team", false),
            ],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2);
        assert_eq!(set[0].url, "https://index.grimoire.rs");
        assert_eq!(set[0].kind, SourceKind::IndexHttp);
        assert!(set[0].is_default);
        assert_eq!(set[1].kind, SourceKind::Registry);
    }

    #[test]
    fn invalid_entry_with_neither_url_nor_index_is_skipped() {
        // Programmatic (unvalidated) configs never fabricate an empty source.
        let empty = RegistryConfig::default();
        let set = resolve_registries(&[], &[empty], None, &[], None, "registry.example", None);
        assert_eq!(set.len(), 1, "falls through to the legacy fallback tier");
        assert_eq!(set[0].url, "registry.example");
    }

    #[test]
    fn forced_registry_collapses_to_single() {
        let set = resolve_registries(
            &["flag.example".to_string()],
            &[rc(Some("acme"), "ghcr.io/acme", true)],
            Some("proj.example"),
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].url, "flag.example");
        assert!(set[0].is_default);
    }

    #[test]
    fn multiple_forced_registries_collapse_to_all_in_order_first_primary() {
        // Several `--registry` values browse all of them at once, in order,
        // with the first as primary. `[[registries]]` and env are ignored.
        let set = resolve_registries(
            &["a.example".to_string(), "b.example".to_string()],
            &[rc(Some("acme"), "ghcr.io/acme", true)],
            Some("proj.example"),
            &[],
            None,
            "registry.example",
            Some("env.example"),
        );
        assert_eq!(set.len(), 2);
        assert_eq!(set[0].url, "a.example");
        assert_eq!(set[1].url, "b.example");
        assert!(set[0].is_default, "first forced registry is primary");
        assert!(!set[1].is_default);
    }

    #[test]
    fn forced_registries_dedupe_keeping_first_and_skip_empty() {
        // Duplicate urls collapse to one (first wins); empty strings are
        // ignored so they never produce a blank registry entry.
        let set = resolve_registries(
            &[
                "".to_string(),
                "a.example".to_string(),
                "a.example".to_string(),
                "b.example".to_string(),
            ],
            &[],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2);
        assert_eq!(set[0].url, "a.example");
        assert_eq!(set[1].url, "b.example");
        assert!(set[0].is_default);
    }

    #[test]
    fn all_empty_forced_registries_fall_through_to_registries_array() {
        // A `forced` slice of only empty strings is not a collapse trigger —
        // resolution falls through to the declared `[[registries]]`.
        let set = resolve_registries(
            &["".to_string()],
            &[rc(Some("acme"), "ghcr.io/acme", true)],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].url, "ghcr.io/acme");
    }

    #[test]
    fn registries_array_is_authoritative_project_then_global() {
        let set = resolve_registries(
            &[],
            &[rc(Some("acme"), "ghcr.io/acme", false)],
            Some("proj.example"), // ignored when [[registries]] present
            &[rc(Some("corp"), "registry.corp/team", false)],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2);
        assert_eq!(set[0].url, "ghcr.io/acme");
        assert_eq!(set[1].url, "registry.corp/team");
        // No `default = true` ⇒ the first entry is primary.
        assert!(set[0].is_default);
        assert!(!set[1].is_default);
    }

    #[test]
    fn explicit_default_flag_picks_primary() {
        let set = resolve_registries(
            &[],
            &[
                rc(Some("acme"), "ghcr.io/acme", false),
                rc(Some("corp"), "registry.corp/team", true),
            ],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(primary_registry(&set), "registry.corp/team");
        assert!(set[1].is_default);
        assert!(!set[0].is_default);
    }

    #[test]
    fn duplicate_url_deduped_first_wins() {
        // Same alias, same locator, one in each file: one entry written
        // twice, so the project one wins and the global one collapses.
        let set = resolve_registries(
            &[],
            &[rc(Some("a"), "ghcr.io/acme", false)],
            None,
            &[rc(Some("a"), "ghcr.io/acme", true)], // same entry, global tier
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1);
        assert!(set[0].is_default, "the survivor is the primary either way");
    }

    #[test]
    fn one_locator_under_two_aliases_is_two_sources_across_scopes_too() {
        // The dedup key is the entry, not the file it lives in: a project
        // entry and a global entry naming one locator under two aliases are
        // two views, exactly as two entries in one file are. Anything else
        // would delete a global view the moment the project config happened
        // to mention the same index.
        let set = resolve_registries(
            &[],
            &[rc(None, "https://index.grimoire.rs", false)],
            None,
            &[
                rc_filtered("wide", "https://index.grimoire.rs", &[], &["mine"]),
                rc_filtered("mine", "https://index.grimoire.rs", &["mine"], &[]),
            ],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 3, "no entry may be dropped: {set:?}");
        assert_eq!(
            set.iter().map(|r| r.alias.as_deref()).collect::<Vec<_>>(),
            [None, Some("wide"), Some("mine")],
            "project first, then the global file's own order"
        );
    }

    #[test]
    fn one_alias_at_two_locators_is_two_sources_c029() {
        // C-029, the symmetric half of the test above: the `seen` key is
        // `(normalize_locator(locator), alias)`, and dropping the LOCATOR
        // half is caught by exactly one test in the whole tree — in
        // `command::release`. `config::registry_resolve`'s own 39 tests all
        // pass without it, which is precisely the coverage gap this closes.
        //
        // Its TUI-level sibling is S-022b: the superseded `source_key` keyed a
        // root by its alias alone, so both of these keyed `"acme"` and merged
        // into one root. [`RowSource::root_key`] carries both halves.
        let set = resolve_registries(
            &[],
            &[rc(Some("acme"), "ghcr.io/acme", false)],
            None,
            &[rc(Some("acme"), "quay.io/acme", false)],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2, "one alias at two locators is two entries: {set:?}");
        assert_eq!(
            set.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            ["ghcr.io/acme", "quay.io/acme"],
            "project first, then global"
        );
    }

    // ── C-022: `RowSource`, the injective root identity ─────────────────────

    #[test]
    fn a_row_source_alias_never_collides_with_another_entrys_locator_c022() {
        // The reproduced defect (handover WP-2): `source_key` returned the
        // alias when present and the locator otherwise, so nothing stopped
        // one entry's alias equalling another entry's locator. Both entries'
        // rows merged under one root labelled with whichever wrote into the
        // `BTreeMap` last — exit 0, no warning. The tags are what close it,
        // and nothing validates the collision away: rejecting such an alias
        // would narrow an input that parses on a released build (Principle 9).
        assert_ne!(
            row_source_of(Some("acme.example"), "x").root_key(),
            row_source_of(None, "acme.example").root_key(),
            "an alias and a locator that spell the same string are two roots"
        );
        assert_ne!(
            row_source_of(Some("acme.example"), "x"),
            row_source_of(None, "acme.example"),
            "injective at the type level too, not only in the rendering"
        );
    }

    #[test]
    fn a_row_source_alias_never_collides_with_the_local_sentinel_c022() {
        // `alias = "Local"` parses on `v0.12.1` and must keep parsing, so the
        // synthetic local group's reserved key has to be unreachable from a
        // configured entry by construction rather than by validation.
        assert_ne!(
            row_source_of(Some("Local"), "x").root_key(),
            RowSource::Local.root_key(),
            "the reserved sentinel must stay unreachable from a configured alias"
        );
        assert_eq!(RowSource::Local.root_key(), "Local", "the literal is unchanged");
    }

    #[test]
    fn root_key_renders_each_variant_verbatim_c022() {
        // The rendering is a format `src/config/` owns and `src/tui/` reads
        // back (C-022's ownership statement), so the tag spellings and the
        // separator are a contract, not an implementation detail — and the
        // injectivity assertions above cannot see them: `alias:{alias}:{loc}`
        // is just as injective as `alias:{alias}/{loc}` and leaves the whole
        // suite green. What breaks under that swap is WP-C's, one merge later:
        // `registry_label`'s miss path splits an `alias:` key at the **first
        // `/`** (E-10.3), which is exact only because `validate_registries`
        // rejects a `/` in an alias and no other separator has that guarantee.
        // Pinned here, at the producer, so the encoding cannot drift out from
        // under the consumer that has not landed yet.
        assert_eq!(RowSource::Local.root_key(), "Local");
        assert_eq!(
            row_source_of(Some("acme"), "ghcr.io/acme").root_key(),
            "alias:acme/ghcr.io/acme"
        );
        assert_eq!(row_source_of(None, "ghcr.io/acme").root_key(), "locator:ghcr.io/acme");
        assert_eq!(RowSource::Unattributed.root_key(), "", "E-3: total, and empty");
    }

    #[test]
    fn one_alias_at_two_locators_is_two_row_sources_s022b() {
        // **S-022b at the type level — the assertion the pre-amendment
        // contract could not pass.** `Alias(String)` carried the alias alone,
        // so both of these rendered `"alias:acme"`: one merged root, the same
        // failure mode as the alias-vs-locator collision reached through the
        // other component of the dedup key. `resolve_registries` admits both
        // entries (C-029 above), so this state is reachable from a config
        // that loads.
        let project = row_source_of(Some("acme"), "ghcr.io/acme");
        let global = row_source_of(Some("acme"), "quay.io/acme");
        assert_ne!(project, global, "one alias at two locators is two identities");
        assert_ne!(
            project.root_key(),
            global.root_key(),
            "and two renderings — the `String`-keyed tree maps see only these"
        );
    }

    #[test]
    fn one_file_may_declare_the_same_locator_twice() {
        // Two views of one source: a wide entry beside a narrow one over the
        // same index. Dedup would drop the second whole — alias, filter and
        // all — and the rows only it admits would be unreachable, which is
        // what per-entry browse filters exist to make reachable. Both entries
        // are in ONE file, so the repetition is authored, not layering.
        let set = resolve_registries(
            &[],
            &[
                rc_filtered("everything-else", "https://index.grimoire.rs", &[], &["mine"]),
                rc_filtered("mine", "https://index.grimoire.rs", &["mine"], &[]),
            ],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2, "both views must survive: {set:?}");
        assert_eq!(set[0].alias.as_deref(), Some("everything-else"));
        assert_eq!(set[1].alias.as_deref(), Some("mine"));
        assert_eq!(
            set[1].filter.include_patterns(),
            ["mine"],
            "its own filter, not the winner's"
        );
        assert!(set[0].is_default, "the first entry is still the only primary");
        assert!(!set[1].is_default);
    }

    #[test]
    fn a_global_file_may_declare_the_same_locator_twice() {
        // Same rule on the other side of the boundary: `seen` is written by
        // project entries only, so a second global entry at one locator is
        // inside one file and stays — the user's own global config is where
        // two views are most likely to be written.
        let set = resolve_registries(
            &[],
            &[],
            None,
            &[
                rc(Some("a"), "https://index.grimoire.rs", false),
                rc(Some("b"), "https://index.grimoire.rs/", false),
            ],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2, "both views must survive: {set:?}");
    }

    #[test]
    fn textual_variants_of_same_locator_dedup_first_wins() {
        // Issue #28: a project snapshot and a global declaration of the SAME
        // index that differ only textually (trailing slash, host case) must
        // collapse to one browse entry — not two catalog groups. Both are
        // alias-less, which is the shape `grim init` writes, so the two are
        // the same entry and the normalization is what has to catch them.
        let set = resolve_registries(
            &[],
            &[rc(None, "https://index.grimoire.rs", true)],
            None,
            &[rc(None, "https://INDEX.grimoire.rs/", false)],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1, "textual variants must dedup: {set:?}");
        assert_eq!(
            set[0].url, "https://index.grimoire.rs",
            "stored url stays as first-declared"
        );
    }

    #[test]
    fn oci_host_case_variants_dedup() {
        // Host case-folds; the repository path stays case-sensitive.
        let set = resolve_registries(
            &[],
            &[rc(Some("a"), "GHCR.io/acme", false)],
            None,
            &[rc(Some("a"), "ghcr.io/acme", false)],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1, "host-case variants must dedup: {set:?}");
        assert_eq!(set[0].url, "GHCR.io/acme", "first occurrence wins");
    }

    #[test]
    fn distinct_paths_do_not_dedup() {
        // The path portion is case-sensitive and identity-relevant — two
        // different namespaces on one host stay two sources.
        let set = resolve_registries(
            &[],
            &[
                rc(Some("a"), "ghcr.io/acme", false),
                rc(Some("b"), "ghcr.io/Acme", false),
            ],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 2, "distinct paths must both survive: {set:?}");
    }

    #[test]
    fn a_bare_host_locator_reaches_the_catalog_without_a_trailing_slash_c031() {
        // **design C-031's guarantor, exercised on the real path.**
        // `CatalogEntry.registry` must carry no `/`, and what enforces that is
        // `trim_locator` — NOT `Catalog::build`'s
        // `split_host_namespace(url).0`, whose fall-through arm returns the
        // string whole when the namespace half is empty
        // (`split_host_namespace("ghcr.io/") == ("ghcr.io/", None)`, pinned in
        // `catalog::registry_catalog`). `load_catalog` passes `reg.url`
        // straight through, so a surviving trailing slash lands in every
        // entry's `registry` field and silently re-aims every authored bare
        // pattern, with no diagnostic.
        //
        // Its own test rather than a case inside
        // `every_construction_site_stores_the_locator_without_trailing_slashes`:
        // that test's first assertion uses `ghcr.io/acme/`, whose namespace
        // half is non-empty, so the split absorbs the slash either way and the
        // assertion would mask this one. Only a **bare host** reaches the
        // fall-through arm.
        let set = resolve_registries(
            &[],
            &[rc(Some("acme"), "ghcr.io/", true)],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(
            set[0].url, "ghcr.io",
            "a bare-host locator must reach the catalog builder already trimmed"
        );
    }

    #[test]
    fn every_construction_site_stores_the_locator_without_trailing_slashes() {
        // W-10. `oci = "ghcr.io/acme/"` passes validation, and `url` is the
        // string every consumer compares against: the browse candidate, the
        // TUI's configured-prefix split (`tree::attribute_registry`), the
        // credential lookup, alias substitution, the catalog cache key.
        // Storing the authored form made those answers disagree — the filter
        // re-normalized locally and the tree did not, so a row matched
        // `platform/foo` while rendering under a root that was not its
        // configured source. Normalizing once here is what keeps them in
        // step, so all four sites that build a `ResolvedRegistry` are pinned.
        let declared = resolve_registries(
            &[],
            &[rc(Some("acme"), "ghcr.io/acme/", true)],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(declared[0].url, "ghcr.io/acme", "declared [[registries]] entry");

        // Alias substitution concatenates on the stored url, so an untrimmed
        // one produced a double-slash reference rather than a wrong-but-valid
        // one — the same defect, in the resolution path instead of display.
        let id = resolve_reference("acme/x:1", &declared, "unused.example").expect("qualified alias resolves");
        assert_eq!(id.to_string(), "ghcr.io/acme/x:1");

        let forced = resolve_registries(
            &["ghcr.io/flag/".to_string()],
            &[],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(forced[0].url, "ghcr.io/flag", "--registry flag");

        let legacy = resolve_registries(&[], &[], Some("proj.example/"), &[], None, "registry.example", None);
        assert_eq!(legacy[0].url, "proj.example", "legacy scalar default_registry");

        let fallback = resolve_registries(&[], &[], None, &[], None, "registry.example/", None);
        assert_eq!(fallback[0].url, "registry.example", "built-in fallback");

        // `kind` is classified from the same trimmed string the url is built
        // from, or an entry would store a `.git` locator while being served
        // over the static-file transport.
        let git_index = resolve_registries(
            &[],
            &[rc_index(Some("hub"), "https://github.com/acme/index.git/", true)],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(git_index[0].url, "https://github.com/acme/index.git");
        assert_eq!(git_index[0].kind, SourceKind::IndexGit);
    }

    #[test]
    fn no_registries_folds_legacy_default() {
        let set = resolve_registries(
            &[],
            &[],
            Some("proj.example"),
            &[],
            Some("glob.example"),
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].url, "proj.example");
        assert!(set[0].is_default);
    }

    #[test]
    fn no_registries_no_default_uses_fallback() {
        let set = resolve_registries(&[], &[], None, &[], None, "registry.example", None);
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].url, "registry.example");
    }

    #[test]
    fn reference_explicit_registry_parses_as_is() {
        let set = resolve_registries(&[], &[], Some("ghcr.io/acme"), &[], None, "registry.example", None);
        let id = resolve_reference("ghcr.io/other/x:1", &set, "unused.example").expect("explicit parses");
        assert_eq!(id.registry(), "ghcr.io");
        assert_eq!(id.to_string(), "ghcr.io/other/x:1");
    }

    #[test]
    fn reference_short_id_expands_against_primary() {
        let set = resolve_registries(&[], &[], Some("ghcr.io/acme"), &[], None, "registry.example", None);
        let id = resolve_reference("code-review:stable", &set, "unused.example").expect("short id expands");
        assert_eq!(id.to_string(), "ghcr.io/acme/code-review:stable");
    }

    #[test]
    fn reference_qualified_alias_substitutes_url() {
        let set = resolve_registries(
            &[],
            &[rc(Some("corp"), "registry.corp/team", false)],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        let id = resolve_reference("corp/internal-tool:1", &set, "unused.example").expect("alias substitutes");
        assert_eq!(id.registry(), "registry.corp");
        assert_eq!(id.to_string(), "registry.corp/team/internal-tool:1");
    }

    #[test]
    fn reference_repo_tag_is_not_treated_as_alias() {
        // `code-review:stable` must NOT be read as `alias:repo` even when a
        // `code-review` alias exists: the colon form has no `/`, so it
        // expands against the primary registry as a bare `repo:tag`. The
        // primary here is a *different* registry (`ghcr.io/acme`), so a hijack
        // would be visible as the alias's url leaking in.
        let set = resolve_registries(
            &[],
            &[
                rc(Some("code-review"), "registry.corp/team", false),
                rc(None, "ghcr.io/acme", true),
            ],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        let id =
            resolve_reference("code-review:stable", &set, "unused.example").expect("repo:tag expands against primary");
        assert_eq!(id.to_string(), "ghcr.io/acme/code-review:stable");
    }

    #[test]
    fn reference_unknown_alias_prefix_expands_against_primary() {
        // `acme/x` where `acme` is not a configured alias is a multi-segment
        // repository path under the primary registry, exactly as today.
        let set = resolve_registries(&[], &[], Some("ghcr.io"), &[], None, "registry.example", None);
        let id = resolve_reference("acme/x:1", &set, "unused.example").expect("repo path expands");
        assert_eq!(id.to_string(), "ghcr.io/acme/x:1");
    }

    #[test]
    fn reference_short_id_with_index_only_set_uses_fallback_primary() {
        // Regression: an index-only `[[registries]]` has no OCI primary, so a
        // short id must expand against the caller's fallback chain instead of
        // producing a registry-less `/name` identifier.
        let set = resolve_registries(
            &[],
            &[rc_index(Some("hub"), "https://index.example", true)],
            None,
            &[],
            None,
            "https://index.example",
            None,
        );
        let id = resolve_reference("grim-essentials", &set, "ghcr.io/grimoire-rs").expect("fallback expands");
        assert_eq!(id.to_string(), "ghcr.io/grimoire-rs/grim-essentials");
    }

    #[test]
    fn reference_short_id_with_empty_fallback_errors_instead_of_corrupting() {
        // Defense-in-depth: even a caller passing an empty fallback must get
        // a MissingRegistry error, never an identifier with an empty registry.
        let set = resolve_registries(
            &[],
            &[rc_index(Some("hub"), "https://index.example", true)],
            None,
            &[],
            None,
            "https://index.example",
            None,
        );
        let err = resolve_reference("grim-essentials", &set, "").expect_err("empty fallback must error");
        assert!(err.to_string().contains("registry"), "unexpected error: {err}");
    }

    // ── New tests for env_default semantics ────────────────────────────────

    #[test]
    fn env_default_does_not_collapse_when_registries_present() {
        // `$GRIM_DEFAULT_REGISTRY` must NOT interpose when `[[registries]]`
        // are declared — tier 2 (`[[registries]]`) wins; env is NOT added to
        // the browse set.
        let set = resolve_registries(
            &[],
            &[
                rc(Some("acme"), "ghcr.io/acme", true),
                rc(Some("corp"), "registry.corp/team", false),
            ],
            None,
            &[],
            None,
            "registry.example",
            Some("env.example"), // env_default must be ignored when [[registries]] present
        );
        assert_eq!(
            set.len(),
            2,
            "[[registries]] declared → browse set must be the full array"
        );
        assert_eq!(set[0].url, "ghcr.io/acme");
        assert_eq!(set[1].url, "registry.corp/team");
        assert!(
            set.iter().all(|r| r.url != "env.example"),
            "env_default must NOT appear in browse set when [[registries]] is non-empty"
        );
    }

    #[test]
    fn env_default_is_single_default_when_no_registries() {
        // When no `[[registries]]` exist, `$GRIM_DEFAULT_REGISTRY` is used as
        // the single-default (tier 3 head), ahead of project and global config.
        let set = resolve_registries(
            &[],
            &[],
            Some("proj.example"),
            &[],
            Some("glob.example"),
            "registry.example",
            Some("env.example"),
        );
        assert_eq!(set.len(), 1);
        assert_eq!(
            set[0].url, "env.example",
            "env_default must head tier-3 when no [[registries]] are declared"
        );
    }

    #[test]
    fn flag_collapses_even_with_registries_and_env() {
        // The `--registry` flag (`forced`) is the sole collapse trigger.
        // Both `[[registries]]` and `env_default` are present; the flag
        // still wins.
        let set = resolve_registries(
            &["flag.example".to_string()],
            &[rc(Some("acme"), "ghcr.io/acme", true)],
            None,
            &[],
            None,
            "registry.example",
            Some("env.example"),
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].url, "flag.example");
    }

    #[test]
    fn env_default_beats_project_and_global_in_tier3() {
        // In tier 3 the precedence chain is env > project > global > fallback.
        let set = resolve_registries(
            &[],
            &[],
            Some("proj.example"),
            &[],
            Some("glob.example"),
            "registry.example",
            Some("env.example"),
        );
        assert_eq!(
            set[0].url, "env.example",
            "env_default must beat project and global in tier 3"
        );
    }

    // ── Browse-filter threading (C-009) ─────────────────────────────────────

    fn rc_filtered(alias: &str, oci: &str, include: &[&str], exclude: &[&str]) -> RegistryConfig {
        RegistryConfig {
            include: include.iter().map(|p| (*p).to_string()).collect(),
            exclude: exclude.iter().map(|p| (*p).to_string()).collect(),
            ..rc(Some(alias), oci, false)
        }
    }

    #[test]
    fn declared_entry_compiles_its_authored_filter_without_swapping_the_lists() {
        // The one thing C-009 owns: `include` reaches the include list and
        // `exclude` reaches the exclude list. Swapping the two arguments at
        // the call site compiles and inverts an allowlist into a denylist,
        // so this asserts each side by its own patterns, never by shape.
        let set = resolve_registries(
            &[],
            &[rc_filtered("acme", "registry.acme", &["acme/*"], &["acme/internal"])],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].filter.include_patterns(), ["acme/*"]);
        assert_eq!(set[0].filter.exclude_patterns(), ["acme/internal"]);
        assert!(set[0].filter.matches("ghcr.io", "acme/tools"));
        assert!(
            !set[0].filter.matches("ghcr.io", "acme/internal"),
            "exclude must win over include"
        );
    }

    #[test]
    fn forced_registries_carry_no_filter() {
        // C-009: the `--registry` branch drops config-derived metadata, the
        // same rule its existing `alias: None` follows. S-019 reads the empty
        // pattern lists back out of exactly this.
        let set = resolve_registries(
            &["registry.flag".to_string()],
            &[rc_filtered("acme", "registry.acme", &["acme/*"], &[])],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].url, "registry.flag");
        assert_eq!(set[0].filter, RegistryFilter::default());
        assert!(set[0].filter.include_patterns().is_empty());
    }

    #[test]
    fn uncompilable_pattern_fails_open_instead_of_dropping_the_entry() {
        // Unreachable in production (C-006 exits 78 at config load), but
        // `resolve_registries` returns `Vec`, not `Result` — so the failure
        // has to go somewhere. C-008's stance: browse unfiltered, never lose
        // the registry.
        let set = resolve_registries(
            &[],
            &[rc_filtered("acme", "registry.acme", &["acme{unclosed"], &[])],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1, "a bad pattern must not drop the registry");
        assert_eq!(set[0].url, "registry.acme");
        assert_eq!(set[0].filter, RegistryFilter::default());
        assert!(set[0].filter.matches("ghcr.io", "anything/at-all"));
    }

    #[test]
    fn stack_overflowing_pattern_fails_open_here_instead_of_aborting() {
        // The other half of the crash path. Load-time validation exits 78
        // before resolution runs, but this call site builds the filter from
        // whatever `RegistryConfig` it is handed — and it reached globset's
        // recursive emitter with no bound at all, so a pattern that got here
        // by any route other than a parsed config killed the process
        // (SIGABRT, exit 134). It is now the fail-open arm's business like
        // any other bad pattern. A regression does not fail this test, it
        // aborts the test binary.
        let pattern = format!("{}a{}", "{".repeat(12_000), "}".repeat(12_000));
        let set = resolve_registries(
            &[],
            &[RegistryConfig {
                include: vec![pattern],
                ..rc(Some("acme"), "registry.acme", false)
            }],
            None,
            &[],
            None,
            "registry.example",
            None,
        );
        assert_eq!(set.len(), 1, "the entry survives, unfiltered");
        assert_eq!(set[0].filter, RegistryFilter::default());
    }

    // ── The two diagnostics `resolve_registries` emits ──────────────────────

    /// A `MakeWriter` sink over a shared buffer — same idiom
    /// `catalog_service`'s tests use to prove a warning reaches tracing.
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

    /// Resolve while capturing everything the call logs.
    fn resolve_capturing(project: &[RegistryConfig], global: &[RegistryConfig]) -> (Vec<ResolvedRegistry>, String) {
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
        let set = resolve_registries(&[], project, None, global, None, "registry.example", None);
        drop(guard);
        let captured = String::from_utf8(logs.lock().expect("log buffer is never poisoned").clone())
            .expect("tracing writes UTF-8");
        (set, captured)
    }

    #[test]
    fn duplicate_locator_warns_when_the_dropped_entry_carries_a_filter() {
        // X1: the drop itself is correct (issue #28 — one entry declared in
        // both files is one source), the silence was not. The project entry
        // takes the global one's include/exclude with it, and the user saw
        // exit 0, an empty stderr, and a filter that had simply stopped
        // applying. The sibling arm three lines up already warns; this one
        // did not exist at all.
        //
        // Both entries carry the SAME alias, which is what makes them the
        // same entry rather than two views — so the filter is the only thing
        // lost and the only thing that can trigger the warning.
        let (set, logs) = resolve_capturing(
            &[rc(Some("acme"), "ghcr.io/acme", true)],
            &[rc_filtered("acme", "GHCR.io/acme/", &["acme/platform/**"], &[])],
        );
        assert_eq!(set.len(), 1, "the dedup itself is unchanged: {set:?}");
        assert!(set[0].filter.include_patterns().is_empty(), "the project entry won");
        assert!(
            logs.contains("registry 'acme': repeats an earlier entry for 'GHCR.io/acme/'"),
            "the dropped entry must be named; captured:\n{logs}"
        );
        assert!(
            logs.contains("include/exclude filter"),
            "the warning must say the filter went with it; captured:\n{logs}"
        );
    }

    #[test]
    fn a_second_alias_on_one_locator_is_kept_and_silent() {
        // What used to be the second warning: `glob/repo` would not resolve,
        // because dedup by locator alone never let that alias into the set.
        // Both entries are kept now, so the alias resolves and there is
        // nothing to report — a warning here would fire on a config that
        // works exactly as written.
        let (set, logs) = resolve_capturing(
            &[rc(Some("proj"), "ghcr.io/acme", true)],
            &[rc(Some("glob"), "ghcr.io/acme", false)],
        );
        assert_eq!(set.len(), 2, "two aliases, two sources: {set:?}");
        assert_eq!(
            set.iter().map(|r| r.alias.as_deref()).collect::<Vec<_>>(),
            [Some("proj"), Some("glob")]
        );
        assert!(logs.is_empty(), "nothing was lost, so nothing is warned:\n{logs}");
    }

    #[test]
    fn a_redundant_identical_duplicate_is_not_warned() {
        // The gate. Declaring the same registry, same alias, no filters, in
        // both scopes is legitimate layering — nothing the user can observe
        // was lost, and warning here would fire on every browse for a config
        // that works exactly as written. Noise trains people to skim past the
        // two warnings above, which are the ones that matter.
        let (set, logs) = resolve_capturing(
            &[rc(Some("acme"), "ghcr.io/acme", true)],
            &[rc(Some("acme"), "ghcr.io/acme/", false)],
        );
        assert_eq!(set.len(), 1, "still deduped: {set:?}");
        assert!(logs.is_empty(), "a redundant identical entry must be silent:\n{logs}");
    }

    #[test]
    fn a_unique_locator_set_warns_about_nothing() {
        // Two genuinely different sources: nothing is dropped, nothing is
        // diagnosed.
        let (set, logs) = resolve_capturing(
            &[rc_filtered("acme", "ghcr.io/acme", &["acme/**"], &[])],
            &[rc(Some("corp"), "registry.corp/team", false)],
        );
        assert_eq!(set.len(), 2);
        assert!(logs.is_empty(), "a valid two-source config must log nothing:\n{logs}");
    }

    #[test]
    fn both_warnings_escape_the_registry_name() {
        // W2: the name is authored TOML echoed to a terminal, and
        // `validate_registries` rejects only `char::is_control` — U+202E
        // (RIGHT-TO-LEFT OVERRIDE) is not, so it arrives here intact and
        // reverses the rest of the line in the user's terminal. Both arms
        // interpolate the same expression, so both are asserted.
        let hostile = "a\u{202e}b";
        let (_, compile_logs) =
            resolve_capturing(&[rc_filtered(hostile, "registry.acme", &["acme{unclosed"], &[])], &[]);
        // The dedup arm needs the SAME alias in both files — a different
        // alias is a second view now, and views are never dropped.
        let (_, dup_logs) = resolve_capturing(
            &[rc(Some(hostile), "ghcr.io/acme", false)],
            &[rc_filtered(hostile, "ghcr.io/acme", &["acme/**"], &[])],
        );
        for logs in [&compile_logs, &dup_logs] {
            assert!(
                !logs.contains('\u{202e}'),
                "the raw bidi override must never reach the terminal; captured:\n{logs}"
            );
            assert!(
                logs.contains(r"a\u{202e}b"),
                "the escaped name must still be shown, or the user cannot tell which entry; captured:\n{logs}"
            );
        }
    }
}
