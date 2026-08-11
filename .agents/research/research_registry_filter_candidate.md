# Research — browse-filter match candidate and the `registry set` verb

**Provenance.** Produced 2026-08-11 as the SOTA / known-pitfall axis of the
high-tier `/hex-review` of `feat/registry-set-verb`, and persisted here as the
technology and domain research artifacts for the follow-up `/hex-plan high`
run (`architect` + decompose of the fix loop). Scope was the branch diff:
`browse_candidate`, the `registry set` verb, the pinned globset version, and
the shared `write_config` writer.

**Superseded in part by owner decision, 2026-08-11.** Finding 2 below reports
that grim's pattern grammar has no in-grammar way to disambiguate two registry
hosts inside one index. The owner has since decided the **dual-candidate rule**
(each pattern matched against both `repository` and `{registry}/{repository}`,
admitting on either), which *is* an in-grammar answer — a host-qualified
pattern selects one host, a bare pattern selects all. Read finding 2 as the
problem statement and the ecosystem survey behind that decision, not as an
open gap. Its "Remediation" paragraph is obsolete; the rest stands and is the
best available evidence for how comparable tools solve it.

Findings 1, 3, 4 and 5 are unaffected.

---

## 1. [High] The anchoring model is the polar opposite of gitignore's and Cargo's own default — docs name the surprise but not its cause

**Surface:** `src/config/registry_filter.rs` (D4), `docs/src/configuration.md:420-434`, `src/command/config.rs` (`registry_add_help`/`registry_set` doc comments).

**The gap:** grim's wildcard-free auto-expansion anchors a bare pattern at
the candidate's **first segment** — `hex` means `hex{,/**}`, and matching
`acme/arcana/hex` needs `**/hex` written explicitly. This is not the
gitignore-family default:

- **gitignore itself**: "If there is a separator at the beginning or middle
  (or both) of the pattern, then the pattern is relative to the directory
  level... Otherwise the pattern may match at any level" — a bare pattern
  like `hex` (no slash) is **unanchored by default**, equivalent to `**/hex`.
  Only a **leading** `/` anchors it to the root
  ([git-scm.com/docs/gitignore](https://git-scm.com/docs/gitignore), fetched
  2026-08-11).
- **Cargo's own `include`/`exclude`** — the tool this ADR cites two sections
  earlier in the same doc page for the *precedence* divergence — uses the
  identical rule: "A pattern like `foo` matches any file or directory with
  the name foo anywhere in the package... equivalent to `**/foo`. A pattern
  with a leading slash like `/foo` matches... only in the root"
  ([doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields](https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields),
  fetched 2026-08-11).

grim's rule is the exact inverse of both: bare = anchored-at-root; `**/`
prefix = anywhere. Cargo is the single closest sibling tool a Rust-CLI user
already has muscle memory for, and `docs/src/configuration.md:352-353`
already invokes it by name for the *precedence* model ("unlike Cargo's
`include`/`exclude` fields, which are mutually exclusive") — a reader who
generalizes that one stated divergence to "otherwise it behaves like Cargo"
lands on the wrong anchoring, on the more consequential axis, with no
correction in sight.

**What the docs already do, and what they're missing:** `configuration.md:420-424`
already says "This is the shape that surprises people, because a bare name
reads like one" — the author independently discovered the surprise. What's
missing is the *cause*: it never states that this is the mirror image of
gitignore's and Cargo's own bare-pattern default, which is exactly the fact
that would let a reader translate their existing intuition instead of
re-deriving grim's rule from the worked table. The nearby `[gitignore]`
citation (`:406`) is for the `*`/`**` segment-crossing rule only, not
anchoring — a reader who follows that link to confirm grim's `*` semantics
will find gitignore's docs stating the opposite anchoring convention one
paragraph later, with nothing in grim's page telling them that's expected.

**Is this the surprising choice, or the conventional one?** Genuinely mixed
— not a case to relitigate, a fact to add. Nexus routing rules use full
regex with no implicit default at all (admin writes `^`/`$` explicitly —
[help.sonatype.com/en/routing-rules.html](https://help.sonatype.com/en/routing-rules.html),
fetched 2026-08-11); Artifactory's Ant-style patterns (the model ADR D2
*does* borrow, for precedence) are structurally closer to grim's own choice
— Ant-style segment patterns have no implicit "prepend `**/`" rule for a
bare name, unlike gitignore — though neither JFrog doc page fetched states
this explicitly enough to cite as primary-source-verified
([jfrog.com/help — Include/Exclude Patterns](https://jfrog.com/help/r/how-to-use-include-exclude-patterns/what-are-include/exclude-patterns),
fetched 2026-08-11, inconclusive on this specific point). So D2's own cited
precedent is anchoring-consistent with grim; it's gitignore and Cargo — the
two tools a Rust developer is most likely to reach for as a mental model —
that diverge.

**Remediation:** one added sentence in `docs/src/configuration.md` after
the existing "shape that surprises people" paragraph: *"This is the reverse
of gitignore's and Cargo's own `include`/`exclude` default, where a bare
`foo` matches at any depth (`**/foo`) and only a leading `/` anchors it to
the root — invert that instinct here."* The same addition to
`registry_add_help`'s doc comment would be too long for `--help` — the docs
page is the right target. Doc-only; no code or semantics change; nothing
here reopens D4's frozen dialect decision.

---

## 2. [Warn] Host-stripping across a multi-host index has three known ecosystem answers

> **Superseded as a gap by the dual-candidate decision (2026-08-11)** — see
> the provenance note at the top. The survey below is the evidence base for
> that decision; its remediation paragraph is obsolete.

**Surface:** `src/config/registry_filter.rs` `browse_candidate` (f790273 commit body, "accepted cost" paragraph); ADR D3.

**The problem, named as an accepted cost by the commit itself:** "an index
whose rows span two registry hosts cannot tell them apart, since
`ghcr.io/acme/tools` and `quay.io/acme/tools` are both `acme/tools`." This is
not a hypothetical edge case in the wider ecosystem — every comparable
multi-source tool checked hit this exact problem and picked one of three
concrete mechanisms, none of which existed in grim's pattern grammar:

- **Go modules never strip the host at all.** The module path *is* the
  repository root path, host included — `golang.org/x/net`,
  `github.com/user/repo` — specifically so two proxies can never collide on
  a short name ([go.dev/ref/mod#module-path](https://go.dev/ref/mod#module-path),
  fetched 2026-08-11).
- **Homebrew taps use an explicit qualifier.** `brew install
  username/repository/formula` disambiguates a formula that collides with
  `homebrew/core`'s own name — the qualifier is a first-class part of the
  CLI grammar, not a config-only escape hatch
  ([docs.brew.sh/Taps](https://docs.brew.sh/Taps), fetched 2026-08-11).
- **Artifactory virtual repositories use an admin-configured resolution
  order**, not silent merge — "you can set the order in which repositories
  ... are searched and resolved by ... ordering them ... within the
  corresponding section of the Configure Repositories page"
  ([docs.jfrog.com/virtual-repositories](https://docs.jfrog.com/artifactory/docs/virtual-repositories),
  fetched 2026-08-11).

grim's accepted-cost paragraph was honest about the limitation and correctly
scoped it (single-source filters never contend with each other — only an
*index* spanning multiple hosts is affected). What was missing was that the
pattern grammar had no `host:`-style qualifier at all, so a user hitting this
collision had **no in-grammar way to disambiguate**, only "split into two
index entries" — which the ADR elsewhere argues *against* as the exact
infrastructure-fragmentation cost this feature exists to avoid. Given
`product-context.md`'s "Discovery: The Index" section names the index as the
primary multi-registry surface (not a corner case), and a self-hosted index
aggregating GHCR plus a private registry is a stated adoption path, this
collision is reachable in the product's own intended use.

**Note for the architect.** The dual-candidate rule is a **fourth** shape,
distinct from all three above: rather than forcing the host into every pattern
(Go), adding a qualifier sigil (Homebrew), or resolving by configured order
(Artifactory), it matches each pattern against both the host-stripped and the
fully-qualified form and admits on either. Its closest relative is Homebrew's
— a qualified name is accepted where an unqualified one also works — without
Homebrew's explicit `user/repo/formula` grammar. Worth stating in the ADR as
the alternative-analysis anchor, since no surveyed tool does exactly this and
the trade-off (an unqualified pattern is host-agnostic by construction) is the
thing to argue.

---

## 3. globset: no version drift, one old bug (irrelevant), one alleged bug investigated and disproven

**Surface:** `Cargo.lock` (`globset = "0.4.20"`), ADR "Matching-engine risk notes".

- **No version drift.** `Cargo.lock` pins 0.4.20; crates.io's API confirms
  0.4.20 is still the latest release, published 2026-08-04
  ([crates.io/api/v1/crates/globset](https://crates.io/api/v1/crates/globset),
  fetched 2026-08-11). Nothing to bump.
- **[BurntSushi/ripgrep#1079](https://github.com/BurntSushi/ripgrep/issues/1079)**
  — "the order of patterns added to the builder matters and it shouldn't":
  a real correctness bug (`GlobSetBuilder` order-dependence caused a match
  to silently fail), but filed against **0.4.2** in 2018 and closed as
  fixed in 2019 — seven years stale, long resolved before 0.4.20. Not a
  live risk; noted only because it is exactly the class of "silent
  non-match" the ADR is already careful about elsewhere.
- **[BurntSushi/ripgrep#3018](https://github.com/BurntSushi/ripgrep/issues/3018)**
  — chased down because it looked directly load-bearing: the report claimed a
  3+-item flat brace alternation (`{foo,bar,baz}`) silently drops the *first*
  alternative, which would undermine the exact multi-pattern workaround ADR
  D12 names for `grimoire.toml` hand-editing (`acme/{platform,tools}/**`).
  **Investigated the full thread — not a real bug.** The reporter's own
  reproduction did not compile; BurntSushi fixed the compile error and ran it,
  and all three assertions passed. Closed as invalid. **A confirmation, not a
  gap** — grim's `{a,b,c}` multi-pattern escape hatch is sound as documented.
- No CVE reaches globset (confirms the ADR's existing claim; checked that no
  newer advisory exists as of 2026-08-11).
- Third-party crate `glob-set` (distinct from `globset`) claims ~8x faster
  `GlobSetBuilder::build()` by skipping regex compilation — 46µs vs 5.5µs for
  an 8-pattern set ([lib.rs/crates/glob-set](https://lib.rs/crates/glob-set),
  fetched 2026-08-11). Not actionable — compilation happens once per registry
  per config load, not per request. Noted as an "if this ever becomes a hot
  path" data point.

**Relevance to the dual-candidate rule:** matching one pattern set against two
candidate strings is two `GlobSet::is_match` calls, not two compiled sets — the
compile-time cost model above is unchanged.

---

## 4. `registry set`'s patch semantics: kubectl is the precedented shape, not git config — and Cargo doesn't have this verb at all

**Surface:** `src/command/config.rs` `RegistryCommand::Set`, ADR D12 (no
comparable-CLI citation anywhere in the section).

Verified across five comparable CLIs:

| Tool | Verb | Shape |
|---|---|---|
| `git config` | `--add` / `--replace-all` / `--unset-all` | Solves a **different** problem: multiple values under **one flat key**. Default `git config <key> <value>` "replaces at most one line" and *refuses* on an ambiguous multi-valued key without `--replace-all` ([git-scm.com/docs/git-config](https://git-scm.com/docs/git-config), fetched 2026-08-11). |
| `kubectl config set-context` / `set-cluster` / `set-credentials` | patch-merge | **The structurally matching precedent**: "specifying a name that already exists will merge new fields on top of existing values for those fields" ([kubernetes.io](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_config/kubectl_config_set-context), fetched 2026-08-11) — name an existing ID-keyed entry, only the flags you pass change, everything else untouched. Exactly `registry set`'s contract. |
| `cargo config` | — | **No `set` verb exists at all.** `cargo config get` shipped; `set`/`edit` are open, wanted, unimplemented roadmap items ([rust-lang/cargo#9936](https://github.com/rust-lang/cargo/issues/9936), [#9301](https://github.com/rust-lang/cargo/issues/9301), fetched 2026-08-11). |
| `gh config set` | scalar-only | `gh config set editor vim` — single flat key, no structured multi-field entries. |
| `pip config set` | scalar-only, file-level lists | `pip config set command.option value`; list-like options are appended as multi-line values inside one config entry, not via an `--add`-style flag ([pip.pypa.io](https://pip.pypa.io/en/stable/cli/pip_config/), fetched 2026-08-11). |

**Position, with evidence:** `registry set`'s whole-entry patch semantics is
the **kubectl-precedented** shape, not the git-config one — git config's
vocabulary solves a narrower, different problem (append/replace-all under one
key) that does not map onto "edit one field of a structured record." Cargo —
the nearest sibling tool by ecosystem, cited elsewhere in this same diff's docs
— has no comparable verb to borrow from at all; grim is ahead of Cargo's own
roadmap here, not behind convention.

**The one place git config's vocabulary was directly on-point and got
declined:** D12's amendment chose whole-list-replace-only for `grim config set
registry.<alias>.include` rather than `--add`/`--unset` incremental semantics
for that one field — precisely the "multiple values under one key" problem git
config solves. The ADR gives a real, sufficient reason (one field should not
carry two capabilities) but never names git config as the road not taken. A
citation gap, not a functional one.

**Bearing on the `--clear-include` / `--clear-exclude` decision (2026-08-11):**
kubectl's patch-merge precedent does not itself supply a clear primitive — the
kubectl answer to "empty this field" is to edit the kubeconfig or delete and
recreate the entry, i.e. the same two-command route grim had. A distinct clear
flag is therefore an extension beyond the precedent rather than a borrowing
from it, and worth arguing on its own terms in the ADR (atomicity for a GUI
consumer applying a whole desired end-state in one call).

---

## 5. [Warn] `registry set` rewrites an existing hand-authored entry through a non-comment-preserving writer — and grim already owns the fix elsewhere in-tree

**Surface:** `src/command/add.rs:881` `write_config` (per ADR D13, reused by
`registry set`); contrast with `src/install/toml_splice.rs` (per
`subsystem-file-structure.md`'s MCP install-layout section).

ADR D13 already documents that `write_config` hand-writes `[[registries]]`
with `writeln!`, not `Serialize` — a data-loss risk for fields the emitter does
not know about, mitigated by a round-trip *field-equality* tripwire test. That
tripwire proves the **fields** round-trip; it says nothing about **comments**,
because a `writeln!` emitter has no representation of them.

**Scope correction (review synthesis, 2026-08-11).** This research framed
`registry set` as "the first verb in this subsystem that rewrites an existing
entry". That is not accurate — `grim config set registry.<alias>.<field>`
already edits an existing entry through the same `write_config` path, and a
hand-verified round-trip on this branch found the loss identical to every other
`config set`: entry position, both locators, `[options].clients`,
`default_registry` and both declaration tables all survive; only the leading
comment and `[options]` key order move. So the property is **pre-existing and
out of this diff's scope**, not newly introduced by `registry set`.

What survives, and is worth keeping: the fix already exists in-tree. `toml_edit`
is the crate built for exactly this — "the goal is to preserve formatting and
comments — ensuring only the user-requested change is made — which is why
`toml_edit` underlies cargo-edit" ([lib.rs/crates/toml_edit](https://lib.rs/crates/toml_edit),
fetched 2026-08-11) — and grim's own MCP install layer already runs a
"span-preserving splice... every byte outside the managed member — key order,
formatting, comments — survives" via `toml_edit` (`src/install/toml_splice.rs`).
The comment-preserving tool is not a speculative dependency to evaluate; it is
one grim already ships and already trusts for a structurally identical problem.

**Remediation (deferred, not this diff):** track "port the registries writer
onto `toml_edit`" as a follow-up alongside ADR D5's params-struct collapse —
both are Two-Hats-deferred refactors of the same shipped seam.

---

## Sources

- [git-scm.com/docs/gitignore](https://git-scm.com/docs/gitignore) — fetched 2026-08-11 — bare-pattern-matches-any-depth vs leading-slash-anchors-to-root, primary source
- [doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields](https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields) — fetched 2026-08-11 — Cargo's identical bare-pattern-any-depth rule, primary source
- [jfrog.com/help — What are Include/Exclude Patterns](https://jfrog.com/help/r/how-to-use-include-exclude-patterns/what-are-include/exclude-patterns) — fetched 2026-08-11 — Ant-style pattern examples; anchoring default not explicitly stated (inconclusive, flagged as such)
- [help.sonatype.com/en/routing-rules.html](https://help.sonatype.com/en/routing-rules.html) — fetched 2026-08-11 — Nexus routing rules are full regex, explicit anchors, no implicit default
- [goharbor.io — replication rule filter syntax](https://goharbor.io/docs/1.10/administration/configuring-replication/create-replication-rules/) — fetched 2026-08-11 (via search index) — `*`/`**` segment-crossing rule matches grim's; default anchoring not conclusively established
- [go.dev/ref/mod#module-path](https://go.dev/ref/mod#module-path) — fetched 2026-08-11 — Go module paths always include the host, primary source
- [docs.brew.sh/Taps](https://docs.brew.sh/Taps) — fetched 2026-08-11 — `user/repo/formula` qualified-name disambiguation, primary source
- [docs.jfrog.com/artifactory/docs/virtual-repositories](https://docs.jfrog.com/artifactory/docs/virtual-repositories) — fetched 2026-08-11 — admin-configured resolution order for aggregated repos, primary source
- [crates.io/api/v1/crates/globset](https://crates.io/api/v1/crates/globset) — fetched 2026-08-11 — 0.4.20 is current latest (2026-08-04), primary source
- [github.com/BurntSushi/ripgrep/issues/1079](https://github.com/BurntSushi/ripgrep/issues/1079) — fetched 2026-08-11 — order-dependent match bug, filed 0.4.2 (2018), closed fixed 2019, primary source
- [github.com/BurntSushi/ripgrep/issues/3018](https://github.com/BurntSushi/ripgrep/issues/3018) — fetched 2026-08-11 — full thread read; alleged 3-item-brace bug disproven by maintainer's own repro, closed invalid, primary source
- [lib.rs/crates/glob-set](https://lib.rs/crates/glob-set) — fetched 2026-08-11 — competing crate's build-time benchmark claim, not actionable here
- [git-scm.com/docs/git-config](https://git-scm.com/docs/git-config) — fetched 2026-08-11 — `--add`/`--replace-all`/`--unset-all` semantics, primary source
- [kubernetes.io — kubectl config set-context](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_config/kubectl_config_set-context) — fetched 2026-08-11 — patch-merge semantics on a named entry, the structurally matching precedent for `registry set`
- [github.com/rust-lang/cargo/issues/9936](https://github.com/rust-lang/cargo/issues/9936), [#9301](https://github.com/rust-lang/cargo/issues/9301) — fetched 2026-08-11 — `cargo config set`/`edit` unimplemented, tracked roadmap items, primary source
- [pip.pypa.io/en/stable/cli/pip_config](https://pip.pypa.io/en/stable/cli/pip_config/) — fetched 2026-08-11 — scalar `set`, file-level multi-line list convention, primary source
- [lib.rs/crates/toml_edit](https://lib.rs/crates/toml_edit) — fetched 2026-08-11 — comment/formatting preservation as `toml_edit`'s stated design purpose
- `.claude/rules/subsystem-file-structure.md` (in-repo) — grim's MCP install layer already uses a `toml_edit`-based comment-preserving splice (`src/install/toml_splice.rs`)
