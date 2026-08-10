// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Per-registry browse filter: compiled `include`/`exclude` glob lists that
//! narrow what `grim search`, the TUI, and the MCP `grim_search` show for a
//! given `[[registries]]` source. Never touches resolution, locking, or
//! install — a direct reference to an excluded package still resolves.
//!
//! Precedence (plan C-004, ADR D2): a row is shown iff (the include list is
//! empty, or the candidate matches at least one include pattern) AND the
//! candidate matches no exclude pattern. This is the Artifactory model, not
//! Cargo's mutually-exclusive `include`/`exclude` and not gitignore's
//! ordered last-match-wins.
//!
//! Every [`Glob`] in this codebase must be built through [`compile_pattern`]
//! — never `Glob::new` directly. Three of [`GlobBuilder`]'s five settings
//! are pinned away from globset's default and all three are load-bearing:
//! `empty_alternates(true)`, without which `acme{,/**}` compiles cleanly
//! yet never matches bare `acme`; `literal_separator(true)`, without which
//! `*` swallows `/` and becomes indistinguishable from `**`; and
//! `backslash_escape(true)`, whose globset default is *platform-conditional*
//! — `!is_separator('\\')`, so enabled where `\` is not a path separator
//! and disabled on Windows. Leaving that one at its default would make a
//! committed `grimoire.toml` mean two different things across one team's
//! machines, and would contradict this module's own dialect: `\` is in
//! [`GLOB_METACHARACTERS`], so a backslash-bearing pattern is classified as
//! already-authored glob syntax and passed through [`expand_pattern`]
//! verbatim — then compiled on Windows as a literal path separator. The
//! `ignore` crate — BurntSushi's gitignore-semantics reference
//! implementation, and this module's cited precedent — pins
//! `backslash_escape(true)` unconditionally for exactly that reason.
//!
//! A pattern must clear [`pattern_within_limits`] before it reaches
//! [`compile_pattern`]: globset's regex emitter recurses per `{…}` level,
//! so a deep enough pattern aborts the process rather than returning an
//! `Err`. [`compile_set`] is where that order is enforced, and it is the
//! only path both the browse filter and config-load validation take. See
//! `.agents/adr/adr_registry_browse_filters.md` D4 (and its 2026-08-09
//! amendment) and `.agents/research/research_registry_browse_filters.md` §1.
//!
//! Everything below is `pub(crate)` — this module is validated from and
//! consumed by sibling `config`/`catalog`/`command` modules, matching the
//! qualifier `project_config.rs`'s own cross-module seams (e.g.
//! `validate_registries`) already use in this single binary crate.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// The characters that mark a pattern as already-authored glob syntax
/// (plan C-003). A pattern containing none of them is wildcard-free.
const GLOB_METACHARACTERS: [char; 7] = ['*', '?', '[', ']', '{', '}', '\\'];

/// Expand a wildcard-free pattern to also match everything beneath it;
/// pass any other pattern through unchanged (plan C-003, ADR D4).
///
/// A pattern containing none of `* ? [ ] { } \` is wildcard-free and
/// expands to `"{p}{,/**}"` (matches the bare name and everything under
/// it); any other pattern is returned verbatim. An empty pattern is never
/// passed here — [`crate::config::project_config::validate_registries`]
/// rejects it first (plan C-006).
fn expand_pattern(pattern: &str) -> String {
    if pattern.contains(GLOB_METACHARACTERS) {
        pattern.to_string()
    } else {
        format!("{pattern}{{,/**}}")
    }
}

/// Longest accepted pattern. A browse pattern is a repository-path glob,
/// and the OCI distribution spec caps a repository name at 255 characters
/// — 1 KiB leaves four times that for the surrounding glob syntax while
/// bounding the *flat* blowup (a huge `{a,b,c,…}` list, a huge character
/// class) that nesting depth cannot see.
const MAX_PATTERN_BYTES: usize = 1024;

/// Deepest accepted `{…}` nesting. globset's *parser* is iterative but its
/// regex *emitter* (`tokens_to_regex`) recurses once per nesting level, so
/// a deep enough pattern overflows the stack and **aborts the process**
/// (SIGABRT, exit 134) with no `Err` anyone can classify. The cost is
/// **160 bytes of stack per level**, linear: bisected against this
/// module's own builder settings, the deepest surviving nesting is
/// **13 069 on a 2 MiB thread** (13 070 aborts) and **52 391 on 8 MiB**
/// (52 392 aborts) — grim's two real stacks being the 8 MiB main thread,
/// where command code runs, and the 2 MiB Tokio workers the MCP
/// tool-call path lands on. Authored filters nest one or two levels, so
/// 32 is orders of magnitude above any real pattern and orders below the
/// cliff.
///
/// This cap is deliberately **not** subsumed by [`MAX_PATTERN_BYTES`]: at
/// 1 KiB the deepest reachable nesting is ~511, which does not overflow
/// today, so the byte cap alone would look sufficient and a later widening
/// of it — one token — would silently restore the abort. This constant
/// pins the mechanism rather than a side effect of it.
const MAX_BRACE_DEPTH: usize = 32;

/// Longest accepted `include`/`exclude` **list**, summed over its patterns
/// **as [`expand_pattern`] compiles them** — a wildcard-free pattern is
/// charged six bytes more than authored, because that is what enters the
/// program. Counting authored bytes instead under-charges by up to 7×, and
/// 65 500 one-byte patterns then clear the budget and abort globset's build.
/// The two caps above bound one pattern; a list is what
/// [`compile_set`] turns into a single regex program, and nothing bounded
/// that. `grimoire.toml` is found by silent walk-up from cwd, so
/// `git clone && grim search` in a hostile repo compiles whatever the repo
/// shipped: measured before this cap, 7 000 × 1 020-byte wildcard-dense
/// patterns — every one of them inside both per-pattern caps — is a 7 MiB
/// config that peaks at **3.8 GB RSS**, an OOM-kill on the ≤2 GB runner
/// most CI tiers default to. The same file under this cap peaks at
/// **27 MB**.
///
/// 64 KiB is far above any authored list. A source can only ever render
/// `MAX_CATALOG_REPOS` (500) rows, so enumerating one pattern per visible
/// repository at a realistic ~40 bytes each is 23 KiB expanded; the budget is
/// nearly three times that, and still admits 64 maximal-length patterns that
/// already carry glob syntax (63 wildcard-free ones). Its own worst
/// case costs 69 MB (measured, same rig).
///
/// It bounds one list, not the file — the residual is every entry's two
/// lists, itself bounded by `config::FILE_SIZE_LIMIT_BYTES` (8 MiB). The
/// worst config the two limits jointly admit, 126 entries each at budget,
/// peaks at 933 MB: a 4.1× cut, back under the 2 GB line, and linear in
/// total pattern bytes where the single giant set was not. Tightening
/// past that is the file limit's job, not this constant's.
const MAX_PATTERN_LIST_BYTES: usize = 64 * 1024;

/// Whether `pattern` is small enough and shallow enough to hand to
/// [`compile_pattern`] safely. Call this **first**: `compile_pattern`
/// cannot report the deep-nesting case, because the stack overflow aborts
/// the process before any `Result` exists.
///
/// The brace walk is tokenizer-shaped, not a naive counter, because a
/// naive one **under**-counts — the unsafe direction. `backslash_escape`
/// is pinned on, so `\}` is a literal; and globset reads a `}` inside a
/// `[…]` class as a class member. Decrementing on either masks real
/// nesting: `{\}`×200 + `}`×200 walks to depth 1 against a real nesting
/// of 200, and once [`MAX_PATTERN_BYTES`] is widened that shape reaches
/// the abort (measured: SIGABRT at n≈16 000, walk still reporting 1).
/// So the walk skips the character after an unescaped `\`, and skips a
/// class body whole — including globset's two first-position quirks, a
/// leading `!`/`^` and a `]` that opens rather than closes.
///
/// The one construct it still reads differently from globset is an
/// **unclosed** `[`, whose body it swallows to the end of the pattern.
/// That cannot hide an abort: `allow_unclosed_class` is at its `false`
/// default, so such a pattern is an `Err` out of the iterative parser
/// before [`compile_pattern`]'s emitter ever recurses.
///
/// # Errors
///
/// A plain failure description that does **not** quote the pattern,
/// matching [`crate::config::project_config::validate_filter_pattern`]'s
/// contract — its callers interpolate the escaped pattern themselves.
pub(crate) fn pattern_within_limits(pattern: &str) -> Result<(), String> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(format!(
            "must not exceed {MAX_PATTERN_BYTES} bytes (is {})",
            pattern.len()
        ));
    }
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // Whatever follows is a literal, brace included.
            '\\' => {
                chars.next();
            }
            '[' => {
                // globset's `parse_class`, in the same order: an optional
                // leading `!`/`^` negates, and only *after* it can a `]`
                // still be a class member rather than the close.
                if matches!(chars.peek(), Some('!' | '^')) {
                    chars.next();
                }
                if matches!(chars.peek(), Some(']')) {
                    chars.next();
                }
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if deepest > MAX_BRACE_DEPTH {
        return Err(format!(
            "must not nest '{{' more than {MAX_BRACE_DEPTH} levels deep (is {deepest})"
        ));
    }
    Ok(())
}

/// Compile one authored glob pattern (plan C-002). The **only** place a
/// [`Glob`] is constructed in this codebase — every call site, present and
/// future, must route through this function so `empty_alternates(true)` is
/// never forgotten.
///
/// `empty_alternates(true)` is mandatory: without it, `acme{,/**}` compiles
/// without error but silently fails to match bare `acme` (verified against
/// globset 0.4.20 — see `research_registry_browse_filters.md` §1).
/// `literal_separator(true)` is equally mandatory: `*` and `?` stop at a
/// `/`, and only `**` crosses one. This is globset's non-default, chosen so
/// the dialect matches gitignore/rsync/ripgrep, so the `**` in
/// `acme/platform/**` means something, and so a pattern that is guessed
/// wrong fails **narrow** rather than admitting a whole subtree. Under the
/// `false` default `acme/*` also matches `acme/foo/bar`, making `*` and
/// `**` indistinguishable (ADR D4, amended 2026-08-09).
///
/// `backslash_escape(true)` is the third: globset defaults it to
/// `!is_separator('\\')`, so a pattern authored on Linux and committed to a
/// shared `grimoire.toml` would compile with different meaning on a
/// teammate's Windows checkout — `\*` a literal `*` on one, a path
/// separator followed by a wildcard on the other. Pinned for the same
/// reason the `ignore` crate pins it (module docs above).
///
/// The remaining two settings stay at their defaults, both already
/// correct: `case_insensitive` is `false` — OCI repository names are
/// lowercase by spec — and `allow_unclosed_class` is `false`, so an
/// unclosed `[` is `ErrorKind::UnclosedClass` from globset's *iterative*
/// parser rather than a literal `[`. That default is what lets
/// [`pattern_within_limits`] skip a `[…]` body without having to bound
/// the unclosed case: a pattern it mis-reads that way cannot reach the
/// recursive emitter at all.
///
/// # Errors
///
/// Returns the underlying [`globset::Error`] when `pattern` (after
/// [`expand_pattern`]) fails to parse. Callers must clear
/// [`pattern_within_limits`] first — a pattern past those limits aborts the
/// process here instead of erroring.
pub(crate) fn compile_pattern(pattern: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(&expand_pattern(pattern))
        .empty_alternates(true)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
}

/// Compile one authored pattern list into a [`GlobSet`], routing every
/// entry through [`pattern_within_limits`] and then [`compile_pattern`] so
/// the limits and both non-default builder settings hold for the set too.
/// An empty list compiles to a `GlobSet` that matches nothing — see
/// [`RegistryFilter::include_is_empty`] for why that is tracked separately
/// rather than read off the set.
///
/// **This is the one function a browse filter is ever built through**, which
/// is why it is `pub(crate)`: config load validates a pattern by compiling
/// it here ([`crate::config::project_config::validate_filter_pattern`])
/// rather than by re-deriving the same steps, so "what load accepts" and
/// "what browse builds" cannot drift into two answers. `Glob::build` alone
/// is not that answer — `GlobSetBuilder::build` compiles the patterns into a
/// regex set and has its own failure modes (globset's regex emitter recurses
/// per `{…}` level, and `regex` caps both nesting and program size), so a
/// pattern can parse cleanly and still fail here.
///
/// It is also the only place that can see a *list*, so it is where
/// [`MAX_PATTERN_LIST_BYTES`] is enforced — the two per-pattern caps say
/// nothing about how many patterns compile into one program.
///
/// **That budget is enforced at load, and every write path that can reach it
/// rejects on it.** [`crate::config::project_config::validate_registries`]
/// calls this function *twice* per field: once per pattern (through
/// [`crate::config::project_config::validate_filter_pattern`], a one-element
/// list, so the per-pattern caps name the offending pattern) and once over
/// the whole list, which is the only call that can trip
/// `MAX_PATTERN_LIST_BYTES`. An over-budget list is therefore exit **78** from
/// a config file and exit **65** from `grim config registry add`/`set`, whose
/// repeated `--include`/`--exclude` flags accumulate into one list at the CLI
/// write boundary, where nothing is written.
///
/// `grim config set registry.<alias>.include` **cannot reach this budget**, and
/// the reason is arithmetic rather than a missing check: that path *replaces*
/// the list with a one-element `vec![value]` and validates exactly that
/// element, so the most it can ever charge is one [`MAX_PATTERN_BYTES`]
/// pattern (plus `expand_pattern`'s 6 bytes) against a
/// [`MAX_PATTERN_LIST_BYTES`] budget 64× larger. Do not "fix" that asymmetry
/// by summing against the stored list — the per-list semantic is deliberate
/// and pinned by a regression test; a `set` that charged the whole list would
/// reject single patterns that load-time validation accepts.
///
/// The browse-time fail-open (a `warn` and an unfiltered source, per
/// C-008/D11) is what remains for a **programmatically-built**
/// [`crate::config::declaration::RegistryConfig`] that never passed
/// validation — not for anything a user can author. Do not read that
/// fallback as "the aggregate cap is advisory at load" and delete this check
/// as redundant: it is a DoS control, and `grimoire.toml` is found by silent
/// walk-up from cwd.
///
/// # Errors
///
/// A plain failure description that does **not** quote the pattern, matching
/// [`crate::config::project_config::validate_filter_pattern`]'s contract —
/// callers interpolate the escaped pattern themselves. `globset::Error` is
/// not usable as the error type: it has no public constructor, so it could
/// never carry a [`pattern_within_limits`] rejection.
pub(crate) fn compile_set(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    let mut list_bytes = 0usize;
    for pattern in patterns {
        // Before `compile_pattern`, never after: an over-deep pattern aborts
        // the process inside globset's emitter, so there is no `Err` to
        // inspect afterwards.
        pattern_within_limits(pattern)?;
        // Running total, checked before the pattern is added: the point of
        // the budget is to never *build* the oversized program, so the sum
        // cannot be taken up front over the whole slice and the pattern that
        // crosses the line is the one named.
        //
        // Charged on the **expanded** pattern, which is what `compile_pattern`
        // below actually builds: `expand_pattern` appends `{,/**}` to every
        // wildcard-free pattern, so authored bytes under-count by up to 7× —
        // 65 500 one-byte patterns clear a 64 KiB authored budget and then
        // abort globset's build outright. Re-expanding here rather than adding
        // a constant keeps `expand_pattern` the only place that knows the
        // suffix; the extra allocation is one per pattern on a bounded
        // config-load path.
        list_bytes += expand_pattern(pattern).len();
        if list_bytes > MAX_PATTERN_LIST_BYTES {
            return Err(format!(
                "must not exceed {MAX_PATTERN_LIST_BYTES} bytes summed across the list as compiled \
                 (is at least {list_bytes}; a pattern carrying no glob syntax compiles 6 bytes longer than authored)"
            ));
        }
        // `kind()`, not the error itself — its `Display` embeds the
        // auto-expanded glob, and the caller already quotes the authored
        // pattern (`validate_filter_pattern`'s doc comment carries the full
        // argument for why that copy is unwanted).
        builder.add(compile_pattern(pattern).map_err(|err| format!("is not a valid glob: {}", err.kind()))?);
    }
    builder
        .build()
        .map_err(|err| format!("does not compile as part of a glob set: {}", err.kind()))
}

/// A compiled per-registry browse filter: one [`GlobSet`] each for
/// `include`/`exclude`, plus the verbatim authored patterns — a `GlobSet`
/// is write-only, and `grim context` (plan C-020) needs the authored
/// strings back out.
///
/// `PartialEq`/`Eq` are hand-implemented over [`Self::include_patterns`] /
/// [`Self::exclude_patterns`] only (`globset::GlobSet` derives neither).
/// This is total, not an approximation: [`Self::new`] takes no input beyond
/// the two pattern slices, so the compiled `GlobSet`s and
/// `include_is_empty` are pure functions of those vectors — pattern
/// equality *is* filter equality. Do not "fix" this by deriving instead.
/// That argument holds only while `new`'s inputs stay the two slices, so
/// `eq` destructures exhaustively: a field added here — or a third `new`
/// parameter feeding one — is a compile error rather than a silently wrong
/// comparison. It has to be, because `ResolvedRegistry` derives `PartialEq`
/// on top and the TUI reload path compares it: a wrong `eq` would surface
/// as a stale tree, not a failing test.
#[derive(Debug, Clone)]
pub(crate) struct RegistryFilter {
    include: GlobSet,
    exclude: GlobSet,
    /// Whether the authored `include` list was empty — empty include is
    /// implemented as *skipping* the include check, never as compiling a
    /// synthetic `**` (plan C-004).
    include_is_empty: bool,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
}

impl PartialEq for RegistryFilter {
    fn eq(&self, other: &Self) -> bool {
        // Exhaustive on both sides on purpose — see the type's doc comment.
        // The three bindings dropped here are derived from the two pattern
        // vectors; the compiler is what enforces that they still are.
        let Self {
            include: _,
            exclude: _,
            include_is_empty: _,
            include_patterns,
            exclude_patterns,
        } = self;
        let Self {
            include: _,
            exclude: _,
            include_is_empty: _,
            include_patterns: other_include_patterns,
            exclude_patterns: other_exclude_patterns,
        } = other;
        include_patterns == other_include_patterns && exclude_patterns == other_exclude_patterns
    }
}

impl Eq for RegistryFilter {}

/// The unfiltered filter: no authored patterns, so [`Self::matches`] is
/// unconditionally `true`. Equivalent to `new(&[], &[]).unwrap()`, but
/// infallible by construction — an empty pattern list has nothing to
/// compile — so no caller needs an `.expect()` to say "unfiltered".
impl Default for RegistryFilter {
    fn default() -> Self {
        Self {
            include: GlobSet::empty(),
            exclude: GlobSet::empty(),
            include_is_empty: true,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }
}

impl RegistryFilter {
    /// Compile a filter from authored `include`/`exclude` pattern lists
    /// (plan C-004).
    ///
    /// # Errors
    ///
    /// Returns the first pattern's [`compile_set`] failure description.
    /// Unreachable from a parsed config —
    /// [`crate::config::project_config::validate_registries`] (plan C-006)
    /// rejects at load, through this very function, anything that would fail
    /// here. It stays fallible for programmatically-built pattern lists, and
    /// because a list can fail as a *whole* (the regex set is built from all
    /// patterns at once) where every pattern passed validation alone.
    pub(crate) fn new(include: &[String], exclude: &[String]) -> Result<Self, String> {
        Ok(Self {
            include: compile_set(include)?,
            exclude: compile_set(exclude)?,
            include_is_empty: include.is_empty(),
            include_patterns: include.to_vec(),
            exclude_patterns: exclude.to_vec(),
        })
    }

    /// Whether `candidate` passes the filter (plan C-004): `true` iff
    /// (the authored include list was empty, or `candidate` matches at
    /// least one include pattern) AND `candidate` matches no exclude
    /// pattern. Exclude wins on overlap. Order-independent within either
    /// list.
    pub(crate) fn matches(&self, candidate: &str) -> bool {
        // Built once and shared: `GlobSet::is_match` builds a `Candidate`
        // unconditionally, *before* its own empty short-circuit, so the
        // two-call form prepares it twice for every catalog row. globset's
        // docs name `is_match_candidate` the amortization path for exactly
        // this.
        let candidate = globset::Candidate::new(candidate);
        (self.include_is_empty || self.include.is_match_candidate(&candidate))
            && !self.exclude.is_match_candidate(&candidate)
    }

    /// The verbatim authored `include` patterns, in declaration order —
    /// read back out by `grim context` (plan C-020).
    pub(crate) fn include_patterns(&self) -> &[String] {
        &self.include_patterns
    }

    /// The verbatim authored `exclude` patterns, in declaration order —
    /// read back out by `grim context` (plan C-020).
    pub(crate) fn exclude_patterns(&self) -> &[String] {
        &self.exclude_patterns
    }
}

/// Derive the source-relative match candidate for one catalog row (plan
/// C-005, ADR D3):
///
/// ```text
/// repo      = "{registry}/{repository}"
/// candidate = repo.strip_prefix("{source_url}/").unwrap_or(repo)
/// ```
///
/// Equals the second element of `tree::display_split`
/// (`src/tui/tree.rs:592`) for the same row **only when no other configured
/// source's url is a prefix of this row** — a later WP asserts that case in
/// a test so the two cannot drift. `display_split` strips the longest match
/// across the *whole* configured set, this strips the declaring entry's own
/// url, so with both `ghcr.io` and `ghcr.io/acme` configured
/// `ghcr.io/acme/tools/foo` displays as `tools/foo` while the `ghcr.io`
/// entry matches it as `acme/tools/foo`. Deliberate: a pattern means the
/// same thing wherever its entry's url points (plan C-005, ADR D3).
///
/// An index source has no single registry root to be relative to, so its
/// candidate is the fully-qualified ref (the `strip_prefix` falls through
/// to `repo` unchanged).
pub(crate) fn browse_candidate(source_url: &str, registry: &str, repository: &str) -> String {
    let repo = format!("{registry}/{repository}");
    // `oci = "ghcr.io/acme/"` passes validation, and an unstripped trailing
    // slash made the `strip_prefix('/')` below fail — every candidate fell
    // through to the fully-qualified ref, silently disabling the entry's
    // filter (an exclude-only entry then fails OPEN, with no diagnostic).
    // Same normalization `registry_resolve::normalize_locator` already
    // applies for dedup, so this introduces no new concept.
    let source_url = source_url.trim_end_matches('/');
    // Two `strip_prefix` calls rather than one against `format!("{source_url}/")`:
    // same result, one allocation instead of three on the fall-through, and
    // this runs per catalog row.
    match repo.strip_prefix(source_url).and_then(|r| r.strip_prefix('/')) {
        Some(relative) => relative.to_string(),
        None => repo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── C-002: the pinned glob constructor ──────────────────────────

    #[test]
    fn compile_pattern_bare_name_matches_via_empty_alternates() {
        // Plan C-002 / ADR D4, the load-bearing test. Without
        // `empty_alternates(true)`, `acme{,/**}` compiles without error but
        // silently fails to match bare `acme` (verified against globset
        // 0.4.20 in research_registry_browse_filters.md §1) — this test
        // fails the moment that flag is dropped, not just when the
        // constructor is missing entirely.
        let glob = compile_pattern("acme")
            .expect("a wildcard-free pattern must compile")
            .compile_matcher();
        assert!(glob.is_match("acme"), "the bare name itself must match");
        assert!(glob.is_match("acme/foo"), "must match beneath the name");
        assert!(glob.is_match("acme/foo/bar"), "must match nested beneath the name");
        assert!(
            !glob.is_match("acmesomething"),
            "must not match a longer name sharing the prefix"
        );
    }

    #[test]
    fn compile_pattern_star_stops_at_path_separator() {
        // Plan C-002 / ADR D4 (amended 2026-08-09): `literal_separator` is
        // `true`, so `*` stays inside one path segment and only `**`
        // crosses a `/`. Under globset's `false` default this test's second
        // assertion silently inverts — `acme/*` would match `acme/foo/bar`
        // — which is exactly the drift it exists to catch.
        let star = compile_pattern("acme/*")
            .expect("a pattern with an explicit wildcard must compile")
            .compile_matcher();
        assert!(star.is_match("acme/foo"), "'*' must match one segment");
        assert!(
            !star.is_match("acme/foo/bar"),
            "'*' must NOT cross '/' — literal_separator(true) is mandatory"
        );
        assert!(!star.is_match("acme"), "the pattern requires the literal '/' segment");

        // `**` is what crosses, and the contrast is the whole point of the
        // setting: under the `false` default these two globs are identical.
        let globstar = compile_pattern("acme/**")
            .expect("a pattern with an explicit wildcard must compile")
            .compile_matcher();
        assert!(globstar.is_match("acme/foo"));
        assert!(globstar.is_match("acme/foo/bar"), "'**' must cross '/'");
    }

    #[test]
    fn compile_pattern_case_sensitive_by_default() {
        // ADR D4: `case_insensitive` stays at its `false` default — OCI
        // repository names are lowercase by spec.
        let glob = compile_pattern("acme").expect("compiles").compile_matcher();
        assert!(!glob.is_match("ACME"), "matching must stay case-sensitive");
    }

    #[test]
    fn compile_pattern_rejects_unclosed_alternate_group_without_panicking() {
        // Malformed patterns are `Result::Err`, never a panic (research
        // artifact §1) — `validate_filter_pattern` (plan C-006) relies on
        // this to surface a clean config error instead of aborting.
        let err = compile_pattern("acme{unclosed").expect_err("an unclosed alternate group must not compile");
        assert!(
            !err.to_string().is_empty(),
            "the underlying globset::Error must carry a message"
        );
    }

    #[test]
    fn compile_pattern_escapes_backslash_on_every_platform() {
        // `backslash_escape` is pinned `true` rather than left at globset's
        // platform-conditional default (`!is_separator('\\')` — on where `\`
        // is not a path separator, OFF on Windows). `grimoire.toml` is a
        // committed file: one pattern must mean one thing everywhere.
        //
        // Load-bearing on Windows only, and deliberately so: on this
        // platform the pin equals the default, so dropping it cannot change
        // this assertion. The second half is what keeps the test honest
        // locally — it pins what the Windows default *would* do, so the pair
        // reads "compile_pattern must not be on that branch".
        let glob = compile_pattern(r"acme\*x")
            .expect("an escaped metacharacter must compile")
            .compile_matcher();
        assert!(glob.is_match("acme*x"), r"'\*' must be a literal '*', not a wildcard");
        assert!(!glob.is_match("acmeYx"), r"'\*' must not match as a wildcard");

        let unescaped = GlobBuilder::new(r"acme\*x")
            .empty_alternates(true)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .expect("compiles")
            .compile_matcher();
        assert!(
            !unescaped.is_match("acme*x"),
            "the Windows default must give a DIFFERENT verdict — that divergence is why the setting is pinned"
        );
    }

    // ── H1: pre-compile pattern limits ──────────────────────────────

    #[test]
    fn pattern_within_limits_rejects_stack_overflowing_brace_nesting() {
        // The abort this guard exists for. Verified against the unguarded
        // `compile_pattern` before the guard landed: globset's regex emitter
        // (`tokens_to_regex`) recurses once per `{…}` level, and this input
        // overflowed the stack — `fatal runtime error: stack overflow`,
        // SIGABRT, exit 134 — with no `Err` a caller could classify.
        let pattern = format!("{}a{}", "{".repeat(12_000), "}".repeat(12_000));
        assert!(
            pattern_within_limits(&pattern).is_err(),
            "a pattern that overflows globset's emitter must be rejected before it is compiled"
        );
    }

    #[test]
    fn pattern_within_limits_rejects_brace_depth_inside_the_byte_budget() {
        // The depth cap is NOT redundant with the byte cap, and this is the
        // test that proves it: 67 bytes, far inside `MAX_PATTERN_BYTES`, and
        // still rejected. Without this the byte cap alone would look
        // sufficient (at 1 KiB the deepest reachable nesting is ~511, which
        // does not overflow today), and a one-token widening of that cap
        // would silently restore the abort.
        let pattern = format!(
            "{}a{}",
            "{".repeat(MAX_BRACE_DEPTH + 1),
            "}".repeat(MAX_BRACE_DEPTH + 1)
        );
        assert!(pattern.len() < MAX_PATTERN_BYTES, "must stay inside the byte budget");
        let reason = pattern_within_limits(&pattern).expect_err("over-deep nesting must be rejected");
        assert!(
            reason.contains("nest"),
            "the reason must name the depth limit: {reason}"
        );
    }

    #[test]
    fn pattern_within_limits_rejects_escape_masked_brace_depth() {
        // W-4: `backslash_escape(true)` is pinned, so `\}` is a literal `}`
        // and closes nothing. A walk that decrements on it reports depth 1
        // for a pattern globset nests `n` levels deep. Measured against the
        // pre-fix walk: `{\}`×200 + `}`×200 → walk depth 1, real nesting
        // 200, accepted against a cap of 32.
        let n = MAX_BRACE_DEPTH + 8;
        let pattern = format!("{}a{}", r"{\}".repeat(n), "}".repeat(n));
        assert!(pattern.len() < MAX_PATTERN_BYTES, "must stay inside the byte budget");
        let reason = pattern_within_limits(&pattern).expect_err("escape-masked nesting must be rejected");
        assert!(
            reason.contains("nest"),
            "the reason must name the depth limit: {reason}"
        );
    }

    #[test]
    fn pattern_within_limits_rejects_class_masked_brace_depth() {
        // W-4, the other mask: a `}` inside a character class is a class
        // member, not a group close. Same inversion as the escaped form.
        let n = MAX_BRACE_DEPTH + 8;
        let pattern = format!("{}a{}", "{[}]".repeat(n), "}".repeat(n));
        assert!(pattern.len() < MAX_PATTERN_BYTES, "must stay inside the byte budget");
        let reason = pattern_within_limits(&pattern).expect_err("class-masked nesting must be rejected");
        assert!(
            reason.contains("nest"),
            "the reason must name the depth limit: {reason}"
        );
    }

    #[test]
    fn pattern_within_limits_counts_through_a_first_position_class_bracket() {
        // globset reads the `]` immediately after `[` as a class MEMBER,
        // not the close, so the body here is `]}` and that `}` closes
        // nothing. A walk that took the first `]` for the close would
        // decrement on the `}` and report depth 1 for real nesting of n —
        // the same inversion as the escaped and classed forms, reached
        // through globset's first-position quirk instead.
        let n = MAX_BRACE_DEPTH + 8;
        let pattern = format!("{}a{}", "{[]}]".repeat(n), "}".repeat(n));
        assert!(pattern.len() < MAX_PATTERN_BYTES, "must stay inside the byte budget");
        let reason = pattern_within_limits(&pattern).expect_err("first-position-']' nesting must be rejected");
        assert!(
            reason.contains("nest"),
            "the reason must name the depth limit: {reason}"
        );
    }

    #[test]
    fn compile_set_rejects_a_list_over_the_aggregate_budget() {
        // H-1: every pattern here clears both per-pattern caps, and the list
        // as a whole is what compiles into one regex program. Unbounded, a
        // 7 MiB `grimoire.toml` of maximal wildcard-dense patterns peaked at
        // 3.8 GB RSS on this host — from a file found by silent walk-up.
        let patterns: Vec<String> = (0..128).map(|_| "a".repeat(MAX_PATTERN_BYTES)).collect();
        for pattern in &patterns {
            assert!(
                pattern_within_limits(pattern).is_ok(),
                "each pattern must clear the per-pattern caps on its own"
            );
        }
        // Not `expect_err`: the `Ok` side is a `GlobSet`, whose `Debug` is
        // the whole compiled DFA — tens of KiB of noise on a failure.
        let Err(reason) = compile_set(&patterns) else {
            panic!("an over-budget list must be rejected");
        };
        assert!(
            reason.contains("exceed"),
            "the reason must name the aggregate limit: {reason}"
        );
    }

    #[test]
    fn compile_set_admits_a_list_at_the_aggregate_budget() {
        // The limit is inclusive, like both per-pattern caps, and one byte
        // over is what fails — so the budget cannot drift into "one maximal
        // pattern fewer than documented" without this failing.
        //
        // Every pattern here carries a `*`, which makes `expand_pattern` the
        // identity on it — the budget charges compiled bytes, so a
        // wildcard-free fixture would sit 6 bytes per pattern above what it
        // claims and "exactly on the budget" would be a lie.
        let full: Vec<String> = (0..MAX_PATTERN_LIST_BYTES / MAX_PATTERN_BYTES)
            .map(|_| format!("{}*", "a".repeat(MAX_PATTERN_BYTES - 1)))
            .collect();
        assert_eq!(
            full.iter().map(|p| expand_pattern(p).len()).sum::<usize>(),
            MAX_PATTERN_LIST_BYTES,
            "the fixture must sit exactly on the budget"
        );
        assert!(compile_set(&full).is_ok(), "the budget itself must be accepted");

        let mut over = full;
        over.push("*".to_string());
        assert!(compile_set(&over).is_err(), "one byte over the budget must be rejected");
    }

    #[test]
    fn registry_filter_budget_is_per_list_not_per_entry() {
        // T-3, the *tightening* direction, which nothing else defends. Every
        // other budget test compiles ONE list, so sharing a single budget
        // across an entry's `include` and `exclude` — threading a running
        // total through `new`, say — halves the accepted config surface with
        // the whole suite still green. That is a Principle 9 break with
        // nothing red: a committed `grimoire.toml` that loads today would
        // start exiting 78.
        //
        // The constant's own doc names the residual as "every entry's two
        // lists", bounded by `config::FILE_SIZE_LIMIT_BYTES`, not by this cap
        // — so 2 × the budget on one entry is admitted **by design**, and this
        // is where that design is written down in executable form.
        let full: Vec<String> = (0..MAX_PATTERN_LIST_BYTES / MAX_PATTERN_BYTES)
            .map(|_| format!("{}*", "a".repeat(MAX_PATTERN_BYTES - 1)))
            .collect();
        assert_eq!(
            full.iter().map(|p| expand_pattern(p).len()).sum::<usize>(),
            MAX_PATTERN_LIST_BYTES,
            "each list must sit exactly on the budget for the pair to prove anything"
        );
        // Not `is_ok()` alone: the `Err` is the message, and the `Ok` is a
        // pair of GlobSets whose `Debug` is the whole compiled DFA.
        if let Err(reason) = RegistryFilter::new(&full, &full) {
            panic!("include and exclude must each get the full budget, independently: {reason}");
        }
    }

    #[test]
    fn compile_set_budget_is_per_list_not_per_pattern() {
        // The gap the two per-pattern caps leave: every pattern here is
        // legal on its own, which is exactly why `pattern_within_limits`
        // cannot be the place this is caught.
        let patterns: Vec<String> = (0..1000).map(|i| format!("acme/{i}/{}", "x".repeat(100))).collect();
        assert!(
            patterns.iter().all(|p| pattern_within_limits(p).is_ok()),
            "the fixture must be legal pattern by pattern"
        );
        assert!(
            compile_set(&patterns).is_err(),
            "a list of individually-legal patterns must still be bounded as a list"
        );
    }

    #[test]
    fn compile_set_budgets_the_expanded_pattern_not_the_authored_one() {
        // P4: what gets built is `expand_pattern(p)`, and that appends
        // `{,/**}` — six bytes — to every wildcard-free pattern, so charging
        // authored bytes under-counts by up to 7× and the budget stops
        // bounding the program it exists to bound. Measured on globset 0.4.20
        // with this module's builder settings, 65 500 one-byte patterns clear
        // the authored budget and then abort the build — `Regex("error
        // building NFA")` at 277 MB peak RSS, which is precisely the
        // allocation the cap refuses elsewhere.
        //
        // 10 000 × one byte: 10 KB authored, 70 KB expanded.
        let inflating: Vec<String> = (0..10_000).map(|_| "a".to_string()).collect();
        assert!(
            inflating.iter().map(String::len).sum::<usize>() < MAX_PATTERN_LIST_BYTES,
            "the fixture must clear the budget on authored bytes, or it proves nothing"
        );
        let Err(reason) = compile_set(&inflating) else {
            panic!("a list that exceeds the budget only once expanded must still be rejected");
        };
        assert!(
            reason.contains("exceed"),
            "the reason must name the aggregate limit: {reason}"
        );
    }

    #[test]
    fn pattern_within_limits_rejects_over_long_pattern() {
        // Flat blowup — a huge alternation list or character class — nests
        // nothing, so the depth cap cannot see it. One byte over the cap is
        // rejected; the cap itself is accepted.
        let over = "a".repeat(MAX_PATTERN_BYTES + 1);
        let reason = pattern_within_limits(&over).expect_err("an over-long pattern must be rejected");
        assert!(
            reason.contains("exceed"),
            "the reason must name the byte limit: {reason}"
        );
        assert!(
            pattern_within_limits(&"a".repeat(MAX_PATTERN_BYTES)).is_ok(),
            "the cap itself must be accepted — the limit is inclusive"
        );
    }

    #[test]
    fn pattern_within_limits_admits_real_patterns() {
        // Every pattern this module's own tests and the plan's worked
        // examples use must pass the guard untouched.
        for pattern in [
            "acme",
            "acme/platform/**",
            "acme/{platform,tools}/**",
            "acme/{platform,tools/{a,b}}/**",
            r"acme\x",
            // The two constructs the walk now skips must not become
            // rejections: neither shape nests anything.
            "acme[ab]/**",
            r"acme\{x",
        ] {
            assert!(
                pattern_within_limits(pattern).is_ok(),
                "must admit the authored pattern {pattern:?}"
            );
        }
        assert!(
            pattern_within_limits(&format!(
                "{}a{}",
                "{".repeat(MAX_BRACE_DEPTH),
                "}".repeat(MAX_BRACE_DEPTH)
            ))
            .is_ok(),
            "the depth cap itself must be accepted — the limit is inclusive"
        );
    }

    // ── C-003: auto-expansion rule ──────────────────────────────────

    #[test]
    fn expand_pattern_wildcard_free_gets_brace_alternation() {
        assert_eq!(expand_pattern("acme"), "acme{,/**}");
        assert_eq!(expand_pattern("acme/platform"), "acme/platform{,/**}");
    }

    #[test]
    fn expand_pattern_passes_wildcarded_patterns_through_verbatim() {
        // Any of `* ? [ ] { } \` marks a pattern as already-authored glob
        // syntax — passed through unchanged, never auto-expanded.
        for pattern in ["acme/*", "acme?", "acme[ab]", "acme{a,b}", r"acme\x"] {
            assert_eq!(
                expand_pattern(pattern),
                pattern,
                "must pass {pattern:?} through verbatim"
            );
        }
    }

    #[test]
    fn compile_pattern_expands_wildcard_free_entry_end_to_end() {
        // Plan C-003: `acme/platform` matches `acme/platform` and
        // `acme/platform/foo` once auto-expanded; C-006 never lets an empty
        // pattern reach this function.
        let glob = compile_pattern("acme/platform").expect("compiles").compile_matcher();
        assert!(glob.is_match("acme/platform"));
        assert!(glob.is_match("acme/platform/foo"));
    }

    // ── C-004: `RegistryFilter` ──────────────────────────────────────

    #[test]
    fn registry_filter_both_empty_matches_everything() {
        let filter = RegistryFilter::new(&[], &[]).expect("empty lists always compile");
        assert!(filter.matches("anything"));
        assert!(filter.matches("acme/platform/foo"));
    }

    #[test]
    fn registry_filter_include_only() {
        let filter = RegistryFilter::new(&["acme/platform".to_string()], &[]).expect("compiles");
        assert!(filter.matches("acme/platform"));
        assert!(filter.matches("acme/platform/foo"));
        assert!(!filter.matches("acme/other"));
    }

    #[test]
    fn registry_filter_exclude_only() {
        let filter = RegistryFilter::new(&[], &["acme/internal/**".to_string()]).expect("compiles");
        assert!(!filter.matches("acme/internal/foo"));
        assert!(filter.matches("acme/other"));
        // An exclude-only filter never synthesizes an include allow-list,
        // and `acme/internal/**` (verbatim, already wildcarded — never
        // auto-expanded) does not match the bare name itself.
        assert!(filter.matches("acme/internal"));
    }

    #[test]
    fn registry_filter_exclude_wins_on_overlap() {
        let filter = RegistryFilter::new(&["acme/platform".to_string()], &["acme/platform/legacy".to_string()])
            .expect("compiles");
        assert!(filter.matches("acme/platform"));
        assert!(filter.matches("acme/platform/foo"));
        assert!(
            !filter.matches("acme/platform/legacy/thing"),
            "exclude must win where include and exclude both match"
        );
    }

    #[test]
    fn registry_filter_exactly_one_package_s002() {
        // Plan C-004 / S-002: combining the two lists admits exactly one
        // package and nothing beneath it.
        let filter = RegistryFilter::new(
            &["acme/platform/foo".to_string()],
            &["acme/platform/foo/**".to_string()],
        )
        .expect("compiles");
        assert!(filter.matches("acme/platform/foo"));
        assert!(!filter.matches("acme/platform/foo/bar"));
    }

    #[test]
    fn registry_filter_matches_is_order_independent() {
        // Plan C-004: the same verdict regardless of declared order within
        // either list. This chain has shipped an order-dependent
        // correctness bug before (ripgrep#1079, root-caused to
        // aho-corasick 0.6.10); grim's lockfile carries aho-corasick 1.1.4,
        // so this pins a property rather than guarding a live defect.
        let forward = RegistryFilter::new(
            &["acme/platform".to_string(), "acme/tools".to_string()],
            &["acme/platform/legacy".to_string(), "acme/tools/old".to_string()],
        )
        .expect("compiles");
        let reversed = RegistryFilter::new(
            &["acme/tools".to_string(), "acme/platform".to_string()],
            &["acme/tools/old".to_string(), "acme/platform/legacy".to_string()],
        )
        .expect("compiles");
        for candidate in [
            "acme/platform",
            "acme/platform/legacy/x",
            "acme/tools",
            "acme/tools/old/x",
            "acme/other",
        ] {
            assert_eq!(
                forward.matches(candidate),
                reversed.matches(candidate),
                "verdict for {candidate} must not depend on declared order"
            );
        }
    }

    #[test]
    fn registry_filter_eq_over_source_patterns() {
        let a = RegistryFilter::new(&["acme/platform".to_string()], &[]).expect("compiles");
        let b = RegistryFilter::new(&["acme/platform".to_string()], &[]).expect("compiles");
        let c = RegistryFilter::new(&["acme/other".to_string()], &[]).expect("compiles");
        assert_eq!(a, b);
        assert_ne!(a, c);

        // Vary ONLY the exclude side. Without this, deleting the
        // `exclude_patterns` conjunct from `eq` leaves the whole suite
        // green — and `ResolvedRegistry` derives `PartialEq` on top of it,
        // so a wrong `eq` surfaces as a stale TUI tree, not a test failure.
        let exclude_a = RegistryFilter::new(&[], &["acme/internal/**".to_string()]).expect("compiles");
        let exclude_b = RegistryFilter::new(&[], &["acme/legacy/**".to_string()]).expect("compiles");
        assert_ne!(exclude_a, exclude_b, "a differing exclude list must compare unequal");
    }

    #[test]
    fn registry_filter_retains_source_patterns_verbatim() {
        // Plan C-004 / C-020: `include_patterns`/`exclude_patterns` return
        // the authored strings back out, in declaration order.
        let filter = RegistryFilter::new(
            &["acme/platform".to_string(), "acme/tools".to_string()],
            &["acme/platform/legacy".to_string()],
        )
        .expect("compiles");
        assert_eq!(
            filter.include_patterns(),
            ["acme/platform".to_string(), "acme/tools".to_string()]
        );
        assert_eq!(filter.exclude_patterns(), ["acme/platform/legacy".to_string()]);
    }

    // ── C-005: candidate derivation ──────────────────────────────────

    #[test]
    fn browse_candidate_three_row_table() {
        // Plan C-005 / ADR D3, the plan's own worked table. The drift
        // assertion against `tree::display_split` for the same row cannot
        // be authored from this file: `display_split` is `pub(super)`
        // (visible only inside `crate::tui`, not from `crate::config`), and
        // `src/tui/tree.rs` is WP-D's file in the plan's parallelization
        // table (test-only), not WP-A's — see the specify-phase report's
        // design-gap section.
        assert_eq!(
            browse_candidate("ghcr.io", "ghcr.io", "acme/platform/foo"),
            "acme/platform/foo"
        );
        assert_eq!(
            browse_candidate("ghcr.io/acme", "ghcr.io", "acme/platform/foo"),
            "platform/foo"
        );
        assert_eq!(
            browse_candidate("https://index.grimoire.rs", "ghcr.io", "acme/foo"),
            "ghcr.io/acme/foo"
        );
    }

    #[test]
    fn browse_candidate_ignores_a_trailing_slash_on_the_source_url() {
        // `oci = "ghcr.io/acme/"` passes validation, and without the trim
        // the second `strip_prefix('/')` failed and every candidate fell
        // through to the fully-qualified ref — silently disabling an
        // exclude-only filter, which fails OPEN with no diagnostic.
        for source_url in ["ghcr.io/acme", "ghcr.io/acme/", "ghcr.io/acme//"] {
            assert_eq!(
                browse_candidate(source_url, "ghcr.io", "acme/platform/foo"),
                "platform/foo",
                "a trailing '/' on {source_url:?} must not change the candidate"
            );
        }
    }
}
