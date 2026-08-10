# Research: Glob Semantics for Registry Browse Filters

- **Date:** 2026-08-09
- **Question:** For per-registry `include`/`exclude` glob lists (browse-view
  filtering of package/repo name strings) in `[[registries]]` config — is
  `globset` the right crate, what exactly are its match semantics, and what
  is the standard include/exclude precedence convention? These semantics
  ship once and then freeze (Principle 9, additive-only).
- **Answer:** **`globset` is correct — and the answer to the load-bearing
  question (does `acme{,/**}` do what it looks like it does) is "only if you
  remember one non-obvious flag."** Verified by compiling and running
  against globset 0.4.20 (not by reading docs summaries): the default
  builder path (`Glob::new`) silently drops the empty branch of
  `acme{,/**}` — it does not error, it just fails to match bare `acme`.
  Grim's compiled-glob helper must call
  `GlobBuilder::new(pattern).empty_alternates(true).build()`, never
  `Glob::new(pattern)`, or the auto-expansion feature ships broken and
  frozen. Dependency cost is one crate (`bstr`) — everything else globset
  needs is already in `Cargo.lock` via `regex`.
- **Binary under test:** `globset = "0.4"` → resolved 0.4.20, probed with a
  throwaway `cargo run` harness (not the grim binary — no grim code exists
  yet for this feature).
- **Re-verify before citing:** 2028-02-09 (globset's API has been stable for
  years; the crate landscape section has faster-moving entries flagged
  individually below).

---

## 1. `globset` exact semantics — empirically verified, not just documented

I did not trust doc-fetch summaries for this — they disagreed with each
other on second-order details (e.g. whether nested braces are allowed). I
built a 90-line probe (`Cargo.toml` pinning `globset = "0.4"`, resolved to
**0.4.20**, the current release per
[crates.io](https://crates.io/api/v1/crates/globset) — "Last updated: August
4, 2026") and ran real assertions. Every claim below is a command output,
not a paraphrase.

### `*` / `?` and path separators — `literal_separator` default is `false`

```
acme* (default builder) vs acme            -> true
acme* (default builder) vs acme/foo        -> true
acme* (default builder) vs acme/foo/bar    -> true
-- with literal_separator(true) --
acme* (literal_separator=true) vs acme            -> true
acme* (literal_separator=true) vs acme/foo        -> false
acme* (literal_separator=true) vs acmefoo         -> true
```

`*` crosses `/` by default. `GlobBuilder::literal_separator(bool)` — default
**`false`** — is what makes `*`/`?` path-separator-aware.

> **Recommendation overturned, 2026-08-09 (owner decision).** This section
> originally recommended leaving the flag at `false`, arguing that an author
> writing `acme*` means "anything under acme". **grim sets it to `true`.**
> The measurement above stands — it is what the crate does; only the
> recommendation drawn from it was wrong. Three reasons it lost: under
> `false`, `acme/*` also matches `acme/foo/bar`, so `*` and `**` become the
> same pattern and every `**` in the plan's own examples is decorative; the
> auto-expansion rule (`acme` → `acme{,/**}`) only makes sense if `*` is not
> already recursive; and gitignore, rsync and ripgrep all treat `*` as
> segment-local, so a user's existing intuition transfers. A guessed-wrong
> pattern then fails **narrow**, which is the right direction for a feature
> whose purpose is narrowing. See ADR D4's amendment.

### `acme/**` does NOT match bare `acme`

```
acme/** vs acme            -> false
acme/** vs acme/foo        -> true
acme/** vs acme/foo/bar    -> true
```

Confirms the crate's own doc text ("if the glob ends with `/**`, then it
matches all sub-entries... `foo/**` matches `foo/a` and `foo/a/b`, but not
`foo`") —
[docs.rs/globset](https://docs.rs/globset/latest/globset/), module docs.
This is exactly why the bare-name case needs the brace-alternation
auto-expansion (§ below) — `acme/**` alone silently excludes the registry
root/owner name itself.

### `**/foo` DOES match bare `foo`

```
**/foo vs foo             -> true
**/foo vs bar/foo         -> true
**/foo vs foo/bar         -> false
```

Zero leading segments is a legal match for a leading `**/`. Same doc source
as above.

### `acme{,/**}` — the landmine: **silently inert without `empty_alternates(true)`**

This is the pattern grim is considering as the auto-expansion for a
wildcard-free `include`/`exclude` entry (`acme` → `acme` OR anything under
`acme/`). Three variants tested:

```
-- Glob::new("acme{,/**}")  [uses GlobBuilder defaults, i.e. empty_alternates = false] --
acme{,/**} vs acme            -> false   ← WRONG if the intent is "acme itself matches"
acme{,/**} vs acme/foo        -> true
acme{,/**} vs acme/foo/bar    -> true
acme{,/**} vs acmesomething   -> false

-- GlobBuilder::new("acme{,/**}").empty_alternates(false).build()  [explicit, same as default] --
compiles OK, same (wrong) behavior as above — NOT a parse error

-- GlobBuilder::new("acme{,/**}").empty_alternates(true).build() --
acme{,/**} vs acme            -> true    ← correct
acme{,/**} vs acme/foo        -> true
acme{,/**} vs acme/foo/bar    -> true
acme{,/**} vs acmesomething   -> false
```

**The dangerous part: it does not error either way.** A malformed-pattern
class of bug would at least fail loudly at config-load time. This one
doesn't — `acme{,/**}` compiles fine under both settings and just quietly
matches fewer things than the docs (and the pattern's own appearance) imply
when `empty_alternates` is left at its default. Anyone who writes
`Glob::new(pattern)` instead of going through `GlobBuilder` and remembering
this one flag ships a filter that excludes the exact bare name it was meant
to include — and since this precedes a stabilization freeze, that becomes
permanent, silently-wrong behavior with no error message pointing at the
cause.

**Action for the implementation:** grim's compiled-glob constructor must be
`GlobBuilder::new(pattern).empty_alternates(true).build()` unconditionally
— never the bare `Glob::new`. This should have a unit test asserting
`acme{,/**}` matches bare `acme`, not just a docs comment, because the
failure mode is silent.

### Case sensitivity

```
ACME vs acme (default)                    -> false   (case-sensitive by default)
ACME vs acme (case_insensitive=true)      -> true
```

Controlled via `GlobBuilder::case_insensitive(bool)`, default `false`. No
platform-conditional default (unlike `backslash_escape`, which the crate
docs describe as enabled on non-Windows/disabled on Windows — irrelevant
here since these patterns match name strings, not filesystem paths, so
`backslash_escape`'s platform default shouldn't matter to grim either way).

### Malformed pattern → `Result`, never a panic

```
Glob::new("acme{unclosed") -> Err: error parsing glob 'acme{unclosed':
  unclosed alternate group; missing '}' ...
  Debug: Error { glob: Some("acme{unclosed"), kind: UnclosedAlternates }

Glob::new("[unclosed") -> Err: error parsing glob '[unclosed':
  unclosed character class; missing ']'
  Debug: Error { glob: Some("[unclosed"), kind: UnclosedClass }
```

`Glob::new` / `GlobBuilder::build` return `Result<Glob, globset::Error>`.
`Error` carries the original pattern string plus an `ErrorKind` enum
(`UnclosedClass`, `UnclosedAlternates`, `UnopenedAlternates`,
`DanglingEscape`, `InvalidRange`, `Regex(String)`, …). Compilation is
fully fallible — good fit for validating `[[registries]]` config at load
time and surfacing a clean error, not a panic, on a typo'd pattern.

### `GlobSet` vs individual `Glob`

`GlobSet` batches N compiled globs into one matcher:
`GlobSetBuilder::add` + `build()` → `GlobSet::matches(candidate) ->
Vec<usize>`, the indices of every pattern that matched, in one pass over
the candidate — not N independent scans
([docs.rs/globset](https://docs.rs/globset/latest/globset/)). For grim's
use case (an include list and an exclude list, each potentially several
patterns, evaluated per repo name during a browse render) this is the right
abstraction: build one `GlobSet` for `include` and one for `exclude` at
config-load time, then two `is_match`/`matches` calls per repo name instead
of a loop over raw `Glob`s.

### Minor doc correction (not load-bearing)

The crate's own module docs say brace alternation nesting is "not currently
allowed." Empirically, in 0.4.20, `{a,{b,c}}` compiles without error and
correctly matches `a`, `b`, `c` (and correctly does *not* match the literal
substring `{b,c}` or `b,c`). Stale doc text — doesn't affect this decision
since grim's proposed pattern (`acme{,/**}`) has no nesting, but worth
knowing the restriction claim in the docs is out of date.

## 2. Dependency weight — one net-new crate

Fetched globset 0.4.20's actual `Cargo.toml`
([raw.githubusercontent.com/BurntSushi/ripgrep](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/crates/globset/Cargo.toml)):

```toml
[dependencies]
aho-corasick = "1.1.1"
bstr = { version = "1.6.2", default-features = false, features = ["std"] }
log = { version = "0.4.20", optional = true }        # default feature
regex-syntax = { version = "0.8.0", default-features = false, features = ["std"] }
regex-automata = { version = "0.4.18", default-features = false,
                    features = ["std","perf","syntax","meta","nfa","hybrid"] }
```

Grimoire's own `Cargo.lock` (checked directly, not inferred) already carries,
via the existing `regex` dependency:

| Crate | globset wants | grimoire has today |
|---|---|---|
| `aho-corasick` | `^1.1.1` | `1.1.4` ✓ satisfied |
| `regex-syntax` | `^0.8.0` | `0.8.11` ✓ satisfied |
| `regex-automata` | `^0.4.18` | `0.4.16` ✗ needs a lockfile **version bump**, not a new crate |
| `log` | optional, default feature | `0.4.33` already present (used elsewhere) ✓ |

The only entry with no existing counterpart is **`bstr`**. Its own
`Cargo.toml` ([raw.githubusercontent.com/BurntSushi/bstr](https://raw.githubusercontent.com/BurntSushi/bstr/master/Cargo.toml))
depends on `memchr` (already in `Cargo.lock` at `2.8.3`) and an *optional*
`regex-automata` gated behind bstr's `unicode` feature — which globset does
not enable (`default-features = false, features = ["std"]`), so that
optional edge never activates.

**Net effect of adding `globset` to grimoire: exactly one new crate
(`bstr`), plus a `regex-automata` patch-version bump in the lockfile.** This
is about as cheap as a non-stdlib dependency addition gets.

## 3. Alternatives

| Crate | Downloads (total / recent, [crates.io](https://crates.io) API, fetched 2026-08-09) | Verdict |
|---|---|---|
| **`glob`** (rust-lang-nursery) | 545.9M / 112.7M | Oldest, most-downloaded — but that reflects transitive ubiquity (build scripts, old Cargo internals), not fitness for this job. No brace alternation, no batch `GlobSet`-style matching, weaker error types. The ecosystem itself has an open issue considering [deprecating it in favor of globset](https://github.com/rust-lang-nursery/glob/issues/59). Not a fit. |
| **`wax`** | 2.4M / 0.6M | More expressive glob dialect + directory-tree walking; faster in some micro-benchmarks. Far less battle-tested (two orders of magnitude fewer downloads than globset), and its extra feature surface (tree walking) is dead weight for pure string filtering. Not worth the innovation-token spend here. |
| **`ignore`** | 155.7M / 34.3M | Built *on top of* globset for gitignore-file semantics (layered ignore files, directory walking, parallel traversal). Right layer for filesystem ignore-file handling, wrong layer for "evaluate two flat pattern lists against a name string" — it pulls in globset plus `walkdir`, `same_file`, and more. Heavier than the job needs. |
| **Hand-rolled** | — | Would reinvent brace expansion, character classes, and a proper fallible-parse error type — precisely the kind of "this problem is already solved" case the DRY/KISS rules argue against, given globset is already the de facto standard glob engine in the Rust ecosystem (ripgrep, fd, Cargo's own include/exclude use gitignore-style globs). |

**Recommendation: `globset` is right**, and cheaply so — see § 2.

## 4. Include/exclude precedence — three genuinely different industry shapes exist; grim must pick and say so explicitly

This is the part most likely to be assumed rather than checked, and every
tool in this space does it differently:

**(a) Include-first-then-subtract, exclude wins on overlap, empty include ≡
match-all — this is grim's proposed model.** Confirmed for **JFrog
Artifactory**: "Artifactory will only let you upload an artifact to, or
download an artifact from, a repository if its name matches any of the
include patterns, and does not match any of the exclude patterns" —
[jfrog.com/blog](https://jfrog.com/blog/include-and-exclude-patterns/)
(⚠ blog post dated 2015-11-29 — stale, flagged). The *current* official
reference (`includesPattern` defaults to `**/*` i.e. match-all;
`excludesPattern` defaults to empty i.e. exclude nothing) is confirmed via
[docs.jfrog.com — "What are Include/Exclude Patterns"](https://jfrog.com/help/r/how-to-use-include-exclude-patterns/what-are-include/exclude-patterns)
and the current [Artifactory YAML repo-config reference](https://docs.jfrog.com/installation/docs/repositories-configurations-in-artifactory-yaml)
(both current as of this writing; the live page is JS-rendered so I
confirmed the default-value wording via the search index rather than a
direct body-text quote — re-verify by loading the page in a browser if this
becomes contentious).

**(b) Ordered, first-match-wins, no independent sets — rsync `--include`/
`--exclude`.** NOT the same model. rsync's FILTER RULES are evaluated
strictly in the order given; the first rule that matches a path wins and
later rules are never consulted for that path. "The order of the rules is
important because the first rule that matches is the one that takes
effect... if an early rule excludes a file, no include rule that comes
after it can have any effect... you must place any include overrides
somewhere prior to the exclude that it is intended to limit" —
[0ink.net, "rsync filter rules"](https://0ink.net/posts/2023/2023-06-15-rsync-rules.html)
(⚠ dated 2023-06-15, >18 months old — cross-checked against the current
rsync(1) man page structure, which still documents FILTER RULES the same
way; the ordering behavior itself is rsync's decades-old documented design
and not something that has since changed). Corroborating summary via
[man7.org rsync(1)](https://man7.org/linux/man-pages/man1/rsync.1.html).

**(c) Mutually exclusive, not combined — Cargo's own `include`/`exclude`
manifest fields.** A third shape, and the one grim's own users are most
likely to assume by analogy since it's in the same ecosystem: "The options
are mutually exclusive; setting `include` will override an `exclude`... If
you need to have exclusions to a set of `include` files, use the `!`
operator" —
[doc.rust-lang.org/cargo/reference/manifest.html](https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields).
This is **not** grim's model either — Cargo doesn't combine the two lists,
it picks one. Worth calling out explicitly in grim's docs, because "like
Cargo's include/exclude" is the wrong analogy despite the identical field
names.

**`.gitignore` `!` negation — deliberately NOT the model grim is adopting,**
confirmed by design: patterns are evaluated in file order, "the last
matching pattern decides the outcome," and negation has a documented trap —
"An optional prefix `!` ... negates the pattern; any matching file excluded
by a previous pattern will become included again. **It is not possible to
re-include a file if a parent directory of that file is excluded**" —
[git-scm.com/docs/gitignore](https://git-scm.com/docs/gitignore). Grim reuses
gitignore's **glob token syntax** (via globset, which explicitly targets
gitignore-compatible matching) but explicitly **not** gitignore's
ordered-negation semantics or its parent-directory trap — the docs must say
this in so many words, since "gitignore-style" is exactly the phrase likely
to get used loosely and mislead readers into expecting order-sensitivity.

**Verdaccio** is a fourth data point, closer to (b)/gitignore than to (a):
package-name glob rules (minimatch syntax) are evaluated **in declared
order** per package block, with `**` conventionally last as a catch-all —
not two independent include/exclude sets at all, but ordered rule-block
matching for **access control** (`access`/`publish`/`unpublish`), a
different axis entirely (see § 5).

**Harbor** replication/retention filters are a fifth shape: one pattern (or
comma-separated pattern list) plus a **mode toggle** — "matching" vs.
"excluding" — never independent include-list-and-exclude-list combined in
one rule (per Harbor's replication-rule docs, filter syntax: `*` matches
non-separator characters, `**` matches everything including `/`,
`{alt1,…}` matches any comma-separated alternative — the same glob dialect
family globset implements, for what it's worth).

**Bottom line for grim's docs:** the (a) shape is real, precedented, and
the least surprising choice for a "does this name pass the filter"
mental model — but it is one of **at least four** shapes in active use in
adjacent tooling, and the two most superficially similar-sounding
precedents in this exact ecosystem (Cargo's `include`/`exclude`, and
"gitignore-style" patterns generically) are **both** the wrong model. State
the precedence rule explicitly and give a worked include+exclude-overlap
example; do not rely on "works like Cargo" or "gitignore-style" as
shorthand, both will actively mislead.

## 5. Prior art for "browse filter, not access control"

I could not find a registry/index tool whose documentation contains an
explicit sentence to the effect of "this filter is not a security boundary."
The distinction exists in practice but is communicated **structurally**,
not with a warning label:

- **Sonatype Nexus Repository routing rules** — glob/regex ALLOW/BLOCK
  rules that limit which paths a proxy/group repository will even attempt
  to resolve upstream. Documented under "Repository Management," entirely
  separate from the dedicated "Access Control" doc section (Privileges,
  Roles, Content Selectors, IP Allow List) —
  [help.sonatype.com/routing-rules](https://help.sonatype.com/en/routing-rules.html).
  The docs do note a performance/DoS caveat about expensive regexes and
  point at "Repository Firewall" for actual policy enforcement, which
  reinforces that routing rules are a traffic-shaping mechanism, not a
  security control — but this is inferred from doc structure and adjacent
  wording, not a direct disclaimer.
- **Harbor** replication/tag-retention filters select *which artifacts a
  background job touches*; who can reach a project/repository at all is a
  wholly separate RBAC/robot-account system. Same structural separation,
  same lack of an explicit "not a security boundary" sentence in what I
  could retrieve.
- **Verdaccio** is the interesting counter-example: its glob-pattern rules
  are *actually* the access-control mechanism (`access`/`publish`), not a
  browse-only filter — so it's a useful negative data point (proof that
  glob-pattern-keyed rules *can* be a real security boundary in this space),
  which makes it more important, not less, that grim's docs state plainly
  that its `include`/`exclude` is *not* that.

**This looks like a genuine documentation gap in the ecosystem rather than
a solved-and-copyable pattern.** Recommendation: grim should be explicit and
up-front — state directly in the config-reference docs that `include`/
`exclude` governs the **browse/search rendering only**, that a fully
qualified reference to an excluded package still resolves via `grim add`/
`describe`/direct pull, and that this is a UX filter, not an allowlist.
Don't gesture at "like other registries do it" — the other registries don't
say this either, which is exactly the trap.

## 6. Sources

- [docs.rs/globset](https://docs.rs/globset/latest/globset/) — module docs, glob syntax, `GlobBuilder` methods and defaults (current)
- Empirical probe against `globset = "0.4"` → resolved **0.4.20** — primary evidence for all of § 1 (dated 2026-08-09, this session)
- [raw.githubusercontent.com/BurntSushi/ripgrep — globset/Cargo.toml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/crates/globset/Cargo.toml) — exact dependency list (current)
- [raw.githubusercontent.com/BurntSushi/bstr — Cargo.toml](https://raw.githubusercontent.com/BurntSushi/bstr/master/Cargo.toml) — bstr's own deps (current)
- `Cargo.lock` in this repo — checked directly for existing `aho-corasick`/`regex-automata`/`regex-syntax`/`log`/`memchr` versions (2026-08-09)
- [crates.io API — globset](https://crates.io/api/v1/crates/globset), [glob](https://crates.io/api/v1/crates/glob), [wax](https://crates.io/api/v1/crates/wax), [ignore](https://crates.io/api/v1/crates/ignore) — download counts, current version (fetched 2026-08-09)
- [github.com/rust-lang-nursery/glob issue #59](https://github.com/rust-lang-nursery/glob/issues/59) — ecosystem considering deprecation of `glob` in favor of `globset`
- [jfrog.com/help — What are Include/Exclude Patterns](https://jfrog.com/help/r/how-to-use-include-exclude-patterns/what-are-include/exclude-patterns) — current official doc, default values
- [docs.jfrog.com — Repositories Configuration in Artifactory YAML](https://docs.jfrog.com/installation/docs/repositories-configurations-in-artifactory-yaml) — current official doc, pattern syntax
- [jfrog.com/blog — Include and Exclude Patterns](https://jfrog.com/blog/include-and-exclude-patterns/) — ⚠ dated 2015-11-29, supplementary example only
- [man7.org — rsync(1)](https://man7.org/linux/man-pages/man1/rsync.1.html) — FILTER RULES structure (current)
- [0ink.net — rsync filter rules](https://0ink.net/posts/2023/2023-06-15-rsync-rules.html) — ⚠ dated 2023-06-15, quoted ordering behavior cross-checked against current man page structure
- [doc.rust-lang.org/cargo/reference/manifest.html](https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields) — Cargo's own include/exclude precedence (current)
- [git-scm.com/docs/gitignore](https://git-scm.com/docs/gitignore) — negation ordering and parent-directory trap (current, canonical)
- [verdaccio.org/docs/packages](https://verdaccio.org/docs/packages/) — ordered glob rule blocks as access control (current)
- [help.sonatype.com/en/routing-rules.html](https://help.sonatype.com/en/routing-rules.html) — routing rules vs. access control doc structure (current)
- Harbor replication-rule filter syntax — via search index snippet of goharbor.io docs (versioned docs 1.10–2.11.x); direct fetch 404'd on the URL tried, so treat the exact wording as secondhand and re-fetch a live goharbor.io/docs/2.x URL before quoting verbatim
