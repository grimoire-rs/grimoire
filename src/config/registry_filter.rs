// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Per-registry browse filter: compiled `include`/`exclude` glob lists that
//! narrow what `grim search`, the TUI, and the MCP `grim_search` show for a
//! given `[[registries]]` source. Never touches resolution, locking, or
//! install — a direct reference to an excluded package still resolves.
//!
//! Every pattern is tested against **two** candidate strings: the bare
//! repository path (`acme/tools`) and the fully-qualified reference
//! (`ghcr.io/acme/tools`). A hit on either counts, so a bare pattern
//! matches on every host and a host-qualified pattern matches on that host
//! only; the entry's own `oci`/`index` locator is never part of either.
//!
//! Precedence (design C-002, ADR D2): a row is shown iff (the include list
//! is empty, or **either** candidate matches at least one include pattern)
//! AND **neither** candidate matches any exclude pattern. Exclude-wins is
//! applied once, to the combined per-list verdicts — not as two
//! whole-filter verdicts OR-ed together. This is the Artifactory model, not
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

use globset::{Candidate, Glob, GlobBuilder, GlobSet, GlobSetBuilder};

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

    /// Whether the catalog row identified by `registry` + `repository`
    /// passes the filter (design C-002).
    ///
    /// Every pattern is tested against **two** candidate strings — the bare
    /// repository path and [`qualified_candidate`]'s
    /// `{registry}/{repository}` — and a hit on either counts:
    ///
    /// ```text
    /// bare        = repository
    /// fq          = qualified_candidate(registry, repository)
    /// include_hit = include_is_empty || include ~ bare || include ~ fq
    /// exclude_hit = exclude ~ bare || exclude ~ fq
    /// visible     = include_hit && !exclude_hit
    /// ```
    ///
    /// Exclude-wins is applied **once**, to the combined per-list verdicts —
    /// never as two whole-filter verdicts OR-ed together, which would let
    /// `include = ["acme/tools"]` with `exclude = ["quay.io/acme/tools"]`
    /// admit the `quay.io` row (design C-003). Order-independent within either
    /// list.
    ///
    /// Argument order is pinned (design C-004): `(registry, repository)`, the same
    /// order as `CatalogEntry`'s fields and as its `repo()` format. **A
    /// swapped call compiles.**
    pub(crate) fn matches(&self, registry: &str, repository: &str) -> bool {
        // The unfiltered filter admits every row, so nothing below it can
        // change the verdict: an empty include skips its check outright and
        // an empty `GlobSet` matches nothing. Everything the rest of this
        // function does for such a filter is discarded work — measured at
        // 71.7 ns and 2 allocations per row, against 0.0 ns and 0 here.
        // It is the dominant shape in the field: `--registry` and both
        // legacy fallbacks construct `RegistryFilter::default()`, and this
        // module has never shipped in a release, so every existing user's
        // config is in exactly this state.
        if self.include_is_empty && self.exclude.is_empty() {
            return true;
        }

        // Implementation note carried forward from the single-candidate
        // form: `GlobSet::is_match` builds a `Candidate` unconditionally,
        // *before* its own empty short-circuit, so the naive form prepares
        // one per call. Build one `globset::Candidate` per string and call
        // `is_match_candidate` four times — two evaluations of each of the
        // two already-compiled sets. No new `GlobSet`, no merged set.
        let qualified = qualified_candidate(registry, repository);
        let bare = Candidate::new(repository);
        let fq = Candidate::new(&qualified);

        let include_hit =
            self.include_is_empty || self.include.is_match_candidate(&bare) || self.include.is_match_candidate(&fq);
        let exclude_hit = self.exclude.is_match_candidate(&bare) || self.exclude.is_match_candidate(&fq);

        include_hit && !exclude_hit
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

/// The **qualified** match candidate for one catalog row (design C-001) — one of the
/// two strings [`RegistryFilter::matches`] tests every pattern against; the
/// other is the bare repository path.
///
/// ```text
/// qualified_candidate("ghcr.io", "acme/tools") -> "ghcr.io/acme/tools"
/// qualified_candidate("",        "acme/tools") -> "acme/tools"
/// ```
///
/// Exactly two clauses, in that order: a non-empty `registry` prefixes, an
/// empty one returns `repository` unchanged. The carve-out is load-bearing —
/// **no candidate may ever begin with `/`**, which would match no authored
/// pattern and fail silently.
///
/// **The premise this rests on (design C-031): `CatalogEntry.registry` carries
/// no `/`** — it is a bare host, and `repository` carries the entire namespaced
/// path. **The guarantor for an `oci` source is `registry_resolve`'s
/// `trim_locator`**, applied at every
/// `ResolvedRegistry` construction site, with `load_catalog` passing `reg.url`
/// straight through. It is *not* `split_host_namespace`: that function's
/// fall-through arm returns the string **whole** when the namespace half is
/// empty, which its own pin states outright
/// (`split_host_namespace("ghcr.io/") == ("ghcr.io/", None)`). For an index
/// source the guarantor is `IndexPackage::into_entry`, which splits on the
/// first `/` and rejects an empty registry. That is what makes a bare pattern
/// host-agnostic and what keeps an `oci` entry and an `index` entry agreeing
/// on the same row; drop the trim and every authored bare pattern silently
/// re-aims with no diagnostic.
///
/// One rule for both source kinds: `matches` has no access to
/// `ResolvedRegistry.kind` and must not gain any (design C-008). The entry's own
/// `oci`/`index` locator is deliberately NOT an input, which is the whole
/// point:
///
/// - Editing a locator can no longer re-aim the patterns written against it.
///   Under the old locator-relative rule, moving `oci = "ghcr.io/acme"` to
///   `oci = "ghcr.io"` silently turned `include = ["platform/**"]` into a
///   filter matching nothing — a valid config, exit 0, empty catalog.
/// - A case difference between the locator and the row's registry can no
///   longer make a strip quietly not fire, disabling an entry's filter (an
///   exclude-only entry then failed OPEN, with no diagnostic). **That is true
///   of the deleted strip, and it is not the whole story: the qualified
///   candidate introduces a NEW case sensitivity, on the host itself.**
///   `oci = "GHCR.io/acme"` keeps its casing into `CatalogEntry.registry`
///   (`trim_locator` never case-folds — the stored url is identity), so the
///   qualified candidate is `GHCR.io/acme/tools` and `compile_pattern` leaves
///   `case_insensitive` at `false`. `include = ["ghcr.io/**"]` then admits
///   nothing (the C-019 diagnostic fires, exit 0) and
///   `exclude = ["QUAY.IO/**"]` hides nothing, **silently**. Documented and
///   accepted this round, not fixed — design **S-023**, routed to the owner.
/// - An `oci` and an `index` entry agree, so one pattern means one thing
///   wherever it is written. A bare pattern is host-agnostic; a
///   host-qualified pattern selects one host.
///
/// `CatalogEntry::repo()` has **no** such carve-out — it is an unconditional
/// `format!("{registry}/{repository}")` feeding `grim search` JSON and the
/// index catalog key, both frozen. The two therefore agree byte-for-byte on
/// every entry with a non-empty registry — which is every entry a catalog
/// build produces — and only there. Do not "fix" the divergence in either
/// direction.
///
/// This does **not** equal `tree::display_split`'s second element, which
/// strips the longest locator across the whole configured set. The tree is a
/// display; this is a path matcher.
pub(crate) fn qualified_candidate(registry: &str, repository: &str) -> String {
    if registry.is_empty() {
        return repository.to_string();
    }
    // Exact capacity, not `format!`: `format!("{registry}/{repository}")`
    // sizes its buffer from the 1-byte `"/"` literal, undershoots, and
    // reallocs — measured at 2.00 allocations per row against the base
    // shape's 1.00, in all six filter shapes, and recovering 31–51 % of the
    // whole per-row cost this change added.
    let mut candidate = String::with_capacity(registry.len() + 1 + repository.len());
    candidate.push_str(registry);
    candidate.push('/');
    candidate.push_str(repository);
    candidate
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
    //
    // These pre-date dual-candidate matching and are about include/exclude
    // precedence, not about the host — but they pass `"ghcr.io"` rather than
    // `""` because an empty registry makes `qualified_candidate` return
    // `repository`, so `bare == fq` and all four `is_match_candidate` calls
    // evaluate two identical strings: the assertions could not tell the new
    // rule from the old one. Every pattern below is `acme/…`, which
    // `literal_separator(true)` forbids matching a `ghcr.io/`-prefixed
    // candidate, so the verdicts are unchanged. `""` survives only where it
    // is the subject (`qualified_candidate_returns_an_empty_registry_row_unchanged_c001`).

    #[test]
    fn registry_filter_both_empty_matches_everything() {
        let filter = RegistryFilter::new(&[], &[]).expect("empty lists always compile");
        assert!(filter.matches("ghcr.io", "anything"));
        assert!(filter.matches("ghcr.io", "acme/platform/foo"));
    }

    #[test]
    fn registry_filter_include_only() {
        let filter = RegistryFilter::new(&["acme/platform".to_string()], &[]).expect("compiles");
        assert!(filter.matches("ghcr.io", "acme/platform"));
        assert!(filter.matches("ghcr.io", "acme/platform/foo"));
        assert!(!filter.matches("ghcr.io", "acme/other"));
    }

    #[test]
    fn registry_filter_exclude_only() {
        let filter = RegistryFilter::new(&[], &["acme/internal/**".to_string()]).expect("compiles");
        assert!(!filter.matches("ghcr.io", "acme/internal/foo"));
        assert!(filter.matches("ghcr.io", "acme/other"));
        // An exclude-only filter never synthesizes an include allow-list,
        // and `acme/internal/**` (verbatim, already wildcarded — never
        // auto-expanded) does not match the bare name itself.
        assert!(filter.matches("ghcr.io", "acme/internal"));
    }

    #[test]
    fn registry_filter_exclude_wins_on_overlap() {
        let filter = RegistryFilter::new(&["acme/platform".to_string()], &["acme/platform/legacy".to_string()])
            .expect("compiles");
        assert!(filter.matches("ghcr.io", "acme/platform"));
        assert!(filter.matches("ghcr.io", "acme/platform/foo"));
        assert!(
            !filter.matches("ghcr.io", "acme/platform/legacy/thing"),
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
        assert!(filter.matches("ghcr.io", "acme/platform/foo"));
        assert!(!filter.matches("ghcr.io", "acme/platform/foo/bar"));
    }

    #[test]
    fn registry_filter_matches_is_order_independent() {
        // Plan C-004 / design C-006: the same verdict regardless of declared order
        // within either list. This chain has shipped an order-dependent
        // correctness bug before (ripgrep#1079, root-caused to
        // aho-corasick 0.6.10); grim's lockfile carries aho-corasick 1.1.4,
        // so this pins a property rather than guarding a live defect.
        //
        // Design C-006: the loop runs over `(registry, repository)` PAIRS, at least
        // one per host, so both candidates are exercised on both orderings —
        // an order dependence reachable only through the qualified candidate
        // would otherwise be invisible here.
        let forward = RegistryFilter::new(
            &["acme/platform".to_string(), "quay.io/acme/tools".to_string()],
            &["acme/platform/legacy".to_string(), "ghcr.io/acme/tools/old".to_string()],
        )
        .expect("compiles");
        let reversed = RegistryFilter::new(
            &["quay.io/acme/tools".to_string(), "acme/platform".to_string()],
            &["ghcr.io/acme/tools/old".to_string(), "acme/platform/legacy".to_string()],
        )
        .expect("compiles");
        for (registry, repository) in [
            ("ghcr.io", "acme/platform"),
            ("ghcr.io", "acme/platform/legacy/x"),
            ("ghcr.io", "acme/tools"),
            ("ghcr.io", "acme/tools/old/x"),
            ("ghcr.io", "acme/other"),
            ("quay.io", "acme/platform"),
            ("quay.io", "acme/tools"),
            ("quay.io", "acme/tools/old/x"),
            ("quay.io", "acme/other"),
        ] {
            assert_eq!(
                forward.matches(registry, repository),
                reversed.matches(registry, repository),
                "verdict for {registry}/{repository} must not depend on declared order"
            );
        }
    }

    // **From here to the end of the module, an UNQUALIFIED `C-0NN` / `S-0NN`
    // id — in a comment, a section header or a test-name suffix — indexes
    // `.agents/specs/design_registry_filter_candidate.md`.** Anything spelled
    // `plan C-0NN` / `Plan S-0NN` indexes
    // `.agents/plans/plan_registry_browse_filters.md`, which is what every id
    // above this line means. The two numbering spaces overlap in range and
    // both are cited in this file, so `..._s010` / `..._s011` below are the
    // design record's S-010 / S-011, not the plan's.

    // ── design C-002 / C-003 / C-004 / C-005: dual-candidate matching ───────

    #[test]
    fn matches_pins_its_argument_order_c004() {
        // **One half of a two-part guard; the other half is elsewhere and
        // neither is redundant** (design C-004, corrected). This test calls
        // `matches` itself, so it structurally cannot observe how production
        // calls it — it kills a swap *inside* `matches`
        // (`qualified_candidate(repository, registry)`). What kills a
        // transposed *call site* (`matches(&e.repository, &e.registry)`,
        // which COMPILES) is the three host-qualified C-009 browse tests in
        // `catalog::catalog_service`. Before this pass both mutations
        // survived the entire suite; do not let a later simplification
        // collapse either half.
        //
        // A browse-level test over *wildcard-free* patterns cannot
        // discriminate, which is why the C-009 half has to be
        // host-qualified: `expand_pattern` appends `{,/**}` to a
        // wildcard-free pattern, so under the transposition
        // `acme/platform{,/**}` still matches the transposed qualified
        // candidate `acme/platform/ghcr.io` and the whole expected vector of
        // `a_pattern_is_written_against_the_repository_path_not_the_locator`
        // survives, in order.
        //
        // The explicit `**` is therefore LOAD-BEARING, not decoration.
        // `"ghcr.io/**"` already carries glob syntax, so `expand_pattern`
        // passes it through verbatim and it gets no downward expansion —
        // which is what makes the second assertion a statement about
        // ARGUMENT ORDER rather than about candidate content. Do not
        // "simplify" it to a bare `"ghcr.io"`: that expands to
        // `ghcr.io{,/**}`, which matches the transposed call's bare
        // candidate outright, and the pair below stops describing one
        // coherent rule.
        let filter = RegistryFilter::new(&["ghcr.io/**".to_string()], &[]).expect("compiles");
        assert!(
            filter.matches("ghcr.io", "acme/tools"),
            "(registry, repository) is the pinned order — the same order as CatalogEntry's fields"
        );
        assert!(
            !filter.matches("acme/tools", "ghcr.io"),
            "a transposed call must NOT match: this half kills a swap inside `matches`; the C-009 browse tests kill a transposed call site"
        );
    }

    #[test]
    fn exclude_beats_include_across_candidates_c003() {
        // Design C-003, the discriminating case: exclude-wins is applied ONCE to the
        // combined per-list verdicts, never as two whole-filter verdicts
        // OR-ed together. Verified against globset 0.4.20 with grim's pinned
        // constructor: include ~ bare = true, include ~ fq = false,
        // exclude ~ bare = false, exclude ~ fq = true. A naive
        // `matches(bare) || matches(fq)` returns TRUE for the first row,
        // because the bare-candidate verdict on its own is
        // "include hit && no exclude hit".
        let filter =
            RegistryFilter::new(&["acme/tools".to_string()], &["quay.io/acme/tools".to_string()]).expect("compiles");
        assert!(
            !filter.matches("quay.io", "acme/tools"),
            "the exclude hits via the qualified candidate, so the row is HIDDEN"
        );
        assert!(
            filter.matches("ghcr.io", "acme/tools"),
            "and the exclude is host-scoped, so every other host's row stays VISIBLE"
        );
    }

    #[test]
    fn precedence_both_lists_empty_admits_every_row_c005() {
        // Design C-005 row 1. Asserted with a non-empty registry as well as the
        // empty one, because `include_is_empty` short-circuits before either
        // candidate is built and that is the branch `--registry` relies on
        // (design C-030 / ADR D9).
        let filter = RegistryFilter::new(&[], &[]).expect("empty lists always compile");
        assert!(filter.matches("ghcr.io", "acme/tools"));
        assert!(filter.matches("quay.io", "anything/at-all"));
        assert!(filter.matches("", "acme/platform/foo"));
    }

    #[test]
    fn precedence_include_only_admits_a_hit_on_either_candidate_c005() {
        // Design C-005 row 2: visible iff some include pattern hits `bare` OR `fq`.
        let bare = RegistryFilter::new(&["acme/tools".to_string()], &[]).expect("compiles");
        assert!(bare.matches("ghcr.io", "acme/tools"), "hit via the bare candidate");
        assert!(!bare.matches("ghcr.io", "other/thing"), "no hit on either candidate");

        let qualified = RegistryFilter::new(&["quay.io/**".to_string()], &[]).expect("compiles");
        assert!(
            qualified.matches("quay.io", "acme/tools"),
            "hit via the qualified candidate"
        );
        assert!(!qualified.matches("ghcr.io", "acme/tools"), "and only on that host");
    }

    #[test]
    fn precedence_exclude_only_hides_a_hit_on_either_candidate_c005() {
        // Design C-005 row 3: visible iff NO exclude pattern hits either candidate.
        // The empty include list must stay empty — implemented as *skipping*
        // the include check, never as a synthetic `**` (ADR D2), which the
        // plan C-019 diagnostic gates on.
        let filter = RegistryFilter::new(&[], &["quay.io/**".to_string()]).expect("compiles");
        assert!(filter.matches("ghcr.io", "acme/tools"), "everything except");
        assert!(!filter.matches("quay.io", "acme/tools"), "the excluded host");
        assert!(
            filter.include_patterns().is_empty(),
            "an exclude-only filter must never synthesize an include allow-list"
        );
    }

    #[test]
    fn precedence_both_lists_need_an_include_hit_and_no_exclude_hit_c005() {
        // Design C-005 row 4, all four corners of the conjunction.
        let filter = RegistryFilter::new(&["acme/**".to_string()], &["**/legacy/**".to_string()]).expect("compiles");
        assert!(filter.matches("ghcr.io", "acme/tools"), "include hit, no exclude hit");
        assert!(
            !filter.matches("ghcr.io", "acme/legacy/thing"),
            "include hit, exclude hit — exclude wins"
        );
        assert!(!filter.matches("ghcr.io", "other/thing"), "no include hit");
        assert!(
            !filter.matches("ghcr.io", "other/legacy/thing"),
            "no include hit and an exclude hit"
        );
    }

    #[test]
    fn a_repository_named_like_a_host_is_admitted_and_removable_s010() {
        // Design S-010, the accepted false-positive caveat AND its remedy. A
        // repository literally named `ghcr.io/foo` hosted on quay.io is
        // admitted by `include = ["ghcr.io/**"]` through its BARE candidate —
        // the cost of not adding a host-detection heuristic.
        let include_only = RegistryFilter::new(&["ghcr.io/**".to_string()], &[]).expect("compiles");
        assert!(
            include_only.matches("quay.io", "ghcr.io/foo"),
            "the caveat: the bare candidate spells a host"
        );

        // The remedy removes exactly that row and no other: the exclude hits
        // via the qualified candidate `quay.io/ghcr.io/foo`, and the genuine
        // ghcr.io row's bare candidate (`foo`) does not begin with `quay.io/`.
        let remedied =
            RegistryFilter::new(&["ghcr.io/**".to_string()], &["quay.io/ghcr.io/foo".to_string()]).expect("compiles");
        assert!(!remedied.matches("quay.io", "ghcr.io/foo"), "exactly that row");
        assert!(remedied.matches("ghcr.io", "foo"), "and no other");
    }

    #[test]
    fn a_port_qualified_host_cannot_false_positive_s011() {
        // Design S-011: `localhost:5000/**` is reachable through the qualified
        // candidate only, so unlike S-010's `ghcr.io/**` it has no
        // false-positive surface at all — the OCI `<name>` grammar forbids
        // `:`, so no repository path can spell a port-bearing host.
        let filter = RegistryFilter::new(&["localhost:5000/**".to_string()], &[]).expect("compiles");
        assert!(filter.matches("localhost:5000", "acme/tools"), "the qualified hit");
        for repository in ["acme/tools", "localhost/5000/acme/tools", "acme/localhost-5000/tools"] {
            assert!(
                !filter.matches("ghcr.io", repository),
                "no grammar-valid repository path hits this pattern off-host; {repository:?} did"
            );
        }

        // **Where the guarantee actually lives, stated so it is not
        // over-read.** It is the input grammar's, not the matcher's: `matches`
        // does not validate its arguments, so a hand-built row carrying an
        // ILLEGAL `:` in the repository path *would* hit through the bare
        // candidate. Nothing can produce one — `Identifier::parse` and both
        // catalog constructors reject it upstream — which is exactly why
        // design S-011 is a caveat about reachability rather than a guard.
        assert!(
            filter.matches("ghcr.io", "localhost:5000/acme/tools"),
            "pinned deliberately: the matcher is grammar-blind, and design S-011 rests on the grammar"
        );
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

    // ── design C-001: candidate derivation ───────────────────────────

    #[test]
    fn qualified_candidate_prefixes_a_non_empty_registry_c001() {
        // Design C-001 clause 1. One rule for both source kinds: an `oci` entry at
        // any depth and an `index` entry yield the same pair of candidates
        // for the same row, because the entry's own locator is not an input
        // to either — only the row's own `(registry, repository)` is.
        assert_eq!(qualified_candidate("ghcr.io", "acme/tools"), "ghcr.io/acme/tools");
        assert_eq!(
            qualified_candidate("ghcr.io", "acme/platform/foo"),
            "ghcr.io/acme/platform/foo"
        );
    }

    #[test]
    fn qualified_candidate_returns_an_empty_registry_row_unchanged_c001() {
        // Design C-001 clause 2, and the reason it exists: **no candidate may ever
        // begin with `/`.** A leading-slash candidate matches no authored
        // pattern and fails silently — a valid config, exit 0, empty catalog.
        for repository in ["acme/tools", "acme/platform/foo", "michael-herwig/arcana/hex"] {
            let candidate = qualified_candidate("", repository);
            assert_eq!(candidate, repository, "an empty registry prefixes nothing");
            assert!(
                !candidate.starts_with('/'),
                "the carve-out is what keeps this true; got {candidate:?}"
            );
        }
    }

    #[test]
    fn the_qualified_candidate_carries_the_host_and_the_bare_one_does_not_c007() {
        // Design C-007, rewritten rather than recompiled. Its predecessor
        // asserted `!candidate.contains('.')` — the property this change
        // DELETES for the qualified candidate, and keeps for the bare one.
        //
        // **Only the qualified half is asserted here.** The bare candidate is
        // `repository` verbatim, with no production function between the row
        // and the matcher, so "the bare candidate carries no registry host"
        // can only ever restate this loop's own literals — an assertion no
        // mutation can fail. The bare half is covered behaviourally by
        // `precedence_include_only_admits_a_hit_on_either_candidate_c005`,
        // whose `quay.io/**` pattern does not match the `ghcr.io` row: that
        // verdict is false the moment the bare candidate starts carrying a
        // host.
        for (registry, repository) in [
            ("ghcr.io", "acme/platform/foo"),
            ("quay.io", "michael-herwig/arcana/hex"),
        ] {
            let qualified = qualified_candidate(registry, repository);
            assert!(
                qualified.starts_with(&format!("{registry}/")),
                "the qualified candidate carries it, first segment first; got {qualified:?}"
            );
            assert!(
                qualified.ends_with(repository),
                "and nothing else about the row changes; got {qualified:?}"
            );
        }
    }

    #[test]
    fn a_locator_edit_cannot_re_aim_an_existing_pattern() {
        // The footgun this rule exists to remove: under locator-relativity,
        // moving `oci = "ghcr.io/acme"` to `oci = "ghcr.io"` re-pointed every
        // pattern in the entry, so `include = ["acme/platform/**"]` silently
        // matched nothing — valid config, exit 0, empty catalog. Neither
        // candidate takes the locator as an input, so there is nothing left
        // to re-aim: the verdict is a function of the ROW alone.
        let filter = RegistryFilter::new(&["acme/platform/**".to_string()], &[]).expect("the pattern must compile");
        assert!(filter.matches("ghcr.io", "acme/platform/foo"));
    }
}
