# WP-F stub report — the `Vendor` hook seam across all 18 vendors

**Worktree:** `.agents/worktrees/wp-f` · **Phase:** Stub · **Not committed.**

**Files touched (4, all inside the declared set):** `src/install/vendor.rs`,
`src/install/vendor_claude.rs`, `src/install/vendor_codex.rs`, `src/install/vendor_copilot.rs`.
The other 15 `vendor_*.rs` files are **untouched** — the decline is the default.

**Gates, executed:**

| Gate | Result |
|---|---|
| `cargo check --all-targets` | clean — **zero warnings** |
| `cargo clippy --locked --all-targets -- -D warnings` | pass |
| `cargo test --bin grim` | **2685 passed, 0 failed** |
| `cargo fmt` | applied |

---

## 1. The four method signatures, and why each has that shape

All four are **defaulted trait methods** on `Vendor` (`src/install/vendor.rs`).

### 1.1 `fn hook_surface(&self) -> Option<HookSurface> { None }`

The whole point of Decision A. `kind_support` defaults to `Native` and every vendor override
closes its `match` with `_ => KindSupport::Native`, so an `ArtifactKind::Hook` arm there would
make **all 18 vendors silently claim native hook support** — Warp and Zed included, which have no
hook mechanism of any kind — with no compile error and nothing greppable. Opt-in inverts that: a
forgotten vendor fails safe.

**Scope-blind, deliberately.** No scope parameter and no scope field in `HookSurface`. The A1
project-scope gate rides on the shipped `kind_surface(kind, scope)` seam — see §3.

### 1.2 `fn hook_event_name(&self, event: CanonicalEvent) -> Option<&'static str>`

**Derived, not overridden:** `self.hook_surface().map(|_| event.as_str())`, so every hook-capable
client gets the canonical **PascalCase** spelling and no vendor overrides it. That is WP-B's hard
requirement 1 turned into a structural property rather than a convention: Copilot's camelCase
dialect — under which `matcher = "Bash"` never fires and `matcher = "*"` is *skipped as an invalid
regex* — is now unreachable without a deliberate override, which is the correct amount of friction
for it. The Copilot evidence lives on `CopilotVendor::hook_surface`'s doc, where a reader of that
file will actually meet it.

Returns `Option` (not `&'static str`) so the 15 surfaceless clients answer "no name", and the doc
states explicitly that **naming is not support** — hosting is `RESPONSE_PROJECTION`'s answer.

### 1.3 `fn hook_tier_support(&self, tier: HookTier, event: CanonicalEvent) -> KindSupport`

Returns the **shipped** `KindSupport` tri-state rather than a new enum: `Declined` is exactly the
"tier the client cannot honour" case, and `Degraded` is exactly the ADR's `⊘` "dropped with one
warning". A new parallel enum would have been a second vocabulary for a decision the codebase
already spells.

The stub implements the fail-safe half and stubs the table half — see §4 for why that split, and
§2 for how it stays a query.

### 1.4 `fn hook_registration(&self, entry: &HookEntry, event: CanonicalEvent, launcher: &Path, root: HookRoot<'_>) -> Result<HookRegistration, HookDecline>`

The **single** `HookEntry` → `HookRegistration` assembly site (C-005, C-018b, C-025), defaulted
with a shared body and **overridden by no vendor** — verified: `grep -l "fn hook_registration"
src/install/vendor_*.rs` returns nothing. The three v1 clients differ only in `--client` and the
event spelling, both read off `self`, so one body covers all three and "single assembly site" is a
property of the code rather than a rule someone must remember.

**The parameter split is the C-018b trust boundary made visible.** One publisher-controlled input
(`entry`) and three grim-chosen ones (`event`, `launcher`, `root`). The command string is built
from grim literals plus `launcher`, `self.name()`, `hook_event_name` and `root`; nothing from
`entry` is interpolated into it — `matcher` reaches the structured `HookRegistration::matcher`,
`timeout` reaches `timeout`, and `id`/`policy`/vendor tables reach the dispatch table or nothing.
That makes C-018b's test decidable: same manifest with and without shell metacharacters ⇒
byte-identical command.

Two new types support it, both in `vendor.rs`:

- **`HookRoot<'a> { Global, Workspace(&Path) }`** — the `--root` argument as a closed type instead
  of a `&str`, because Decision P's whole content is that this value is grim-chosen and never
  client-derived. A string parameter would accept `$PWD`.
- **`HookDecline`** — a closed refusal enum with `reason()` + `Display`. Carried in the `Err` arm:
  a decline is warn-and-skip (I3), never an `Error`, never an exit code. `Option` (the `mcp_entry`
  precedent) would have lost the reason, and the reason is precisely what makes a silent guardrail
  visible in `grim status` (S-013). Refusal order is documented, outermost cause first:
  `NoSurface` → `SurfaceUnimplemented` → `EventUnsupported` → `TierUnsupported` →
  **`MutatorOnShellCommandTool`** (added in the re-stub, § 7.2) →
  `MatcherEmpty`/`MatcherNotLossless`.

**No `HookSurface` variant matches into a panic.** `hook_registration` matches all three arms;
`CodegenModule` returns `Err(HookDecline::SurfaceUnimplemented)` — declined plus one warning at the
caller — so the reachable-panic defect WP-A's marker arms were corrected for is not re-created in
new code.

### 1.5 Supporting items (same file)

- **`MatcherForm` + `classify_matcher(Option<&str>) -> MatcherForm`** — C-025's dialect table as
  one classifier: `All` (absent or whole-string `*`), `ExactOrAlternation`, `Empty`,
  `NotTranslatable`. ⛔ **The "the three lossless forms agree" claim here is RETRACTED — see § 7.3
  (F-3).** Identity is the only portable translation and it is **not** lossless: claude/codex are
  tail-open (prefix match), copilot PascalCase is case-insensitive. Both are recorded as residuals on
  `classify_matcher`, and neither is detectable by a client-independent classifier.
- **`posix_single_quote`** (private, **implemented** not stubbed) — the quoting *is* the C-018b/
  guard argument, and WP-B executed the failure it prevents: unquoted expansion + a space in the
  path ⇒ `sh`/`bash` word-split and **a planted executable at the split prefix ran instead of the
  launcher**. A body-less version would have left that constraint unexpressed.
- `All` translates to `matcher: None` (field omitted), not `"*"`: omission is the one form WP-B
  observed firing in **all four** dialect columns, while `"*"` fails one of them.

---

## 2. How `hook_tier_support` stays a query and never a copy (C-021)

It stores **no data**. The verdict is computed from `projection_for(self.name(), event)` — WP-A's
single lookup into `RESPONSE_PROJECTION` — by a three-line rule spelled out in the doc comment so
Specify generates tests from the rule rather than from a table:

1. no surface, or no row for `(client, event)` ⇒ `Declined`;
2. the tier's **required** field absent ⇒ `Declined` (`gatekeeper` → non-empty `verdict`;
   `mutator` → `mutation`; `observer` → nothing, so an observer is never declined on a hosted
   event);
3. otherwise a field the tier *may* use absent ⇒ `Degraded`; else `Native`. (Every tier may emit
   context, so an absent `context` degrades all three.)

Three consequences worth stating, because they are the drift-resistance argument:

- **No vendor file contains a per-tier verdict.** There is nothing to fall out of sync with.
- **Copilot's `mutator` answer follows from the table, not from a special case** — which is exactly
  why the stale row in §4/B1 must be fixed in the table rather than worked around here. A
  `if self.name() == "copilot"` branch would have been the second copy C-021 exists to prevent, and
  it would have passed review by looking like new code.
- Native-only moments are **not** reachable through a second lookup — see B5.

---

## 3. How all 18 vendors resolve

`hook_event_name` and `hook_registration` are derived from `hook_surface`, so the first column
determines the rest. Verified by construction (`grep -l` shows overrides in exactly 3 files) plus
the passing `kind_surface` pin.

| # | Client | `hook_surface()` | `hook_event_name` | `hook_tier_support` | `hook_registration` | `kind_surface(Hook, Project)` | Why |
|---|---|---|---|---|---|---|---|
| 1 | **claude** | `Some(SpliceConfig)` | `Some("PreToolUse"…)` | table query | assembled | **`true`** | `settings.json` / gitignored `settings.local.json`; both scopes |
| 2 | opencode | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | JS-plugin-only surface; `CodegenModule`-shaped, no v1 template |
| 3 | **copilot** | `Some(OwnFile)` | `Some(…)` | table query | assembled | **`false`** | `~/.copilot/hooks/grim.json` dir glob; project file is tracked (A1) |
| 4 | **codex** | `Some(OwnFile)` | `Some(…)` | table query | assembled | **`false`** | `$CODEX_HOME/hooks.json`, one fixed path ⇒ own-or-nothing; project file is tracked (A1) |
| 5 | cursor | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | splice `.cursor/hooks.json` — phase 3 |
| 6 | kiro | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | own-file `.kiro/hooks/*.json` — phase 3 |
| 7 | junie | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | splice `.junie/config.json`, **EAP** — phase 3 |
| 8 | gemini | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | splice `.gemini/settings.json` — phase 3 |
| 9 | **zed** | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | **no hook mechanism exists** |
| 10 | amp | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | JS plugin — `CodegenModule`-shaped, no v1 template |
| 11 | agents | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | synthetic zero-detection fallback; no product to register into |
| 12 | antigravity | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | splice `hooks.json` — phase 3 |
| 13 | cline | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | own-file, filename == event — phase 3 |
| 14 | droid | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | splice `.factory/settings.json` — phase 3 |
| 15 | goose | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | own-dir plugin hooks — phase 3 |
| 16 | **warp** | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | **no hook mechanism exists** |
| 17 | openclaw | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | JS + `HOOK.md` own-dir — phase 3 |
| 18 | kilo | `None` | `None` | `Declined` | `Err(NoSurface)` | `true`¹ | JS plugin (OpenCode runtime) — phase 3 |

¹ `kind_surface` is **not** the hook capability gate — it returns its `true` default for these 15
because `hook_surface() == None` already declines them before scope is ever asked. Documented at
both the trait method and the test.

**Count: 3 capable, 15 declining** — 12 deferred phase-3 clients with real hook mechanisms, plus
warp, zed and the synthetic `agents`. **The brief says "fourteen must decline"; the correct number
is fifteen.** Most likely the synthetic `agents` client was not counted; flagging it because a
pinned-set test written from "fourteen" would be wrong by one row.

`kind_surface`'s pinned `SCOPE_GAPS` set gained two rows (`codex`/`copilot` × `Hook` ×
`Project`) and the existing test loop gained `ArtifactKind::Hook`, so both hook scope gaps are now
pinned in the mechanism that shipped for Junie-rules-at-global. A flip in either direction is
silent otherwise: made `true`, grim writes an armable registration into a tracked file; made
`false` at global too, hooks stop installing on that client entirely.

---

## 4. What I could not stub, and what it needs

### B1 — ⛔ `RESPONSE_PROJECTION`'s `copilot · PreToolUse` row contradicts WP-B requirement 3

`src/oci/hook.rs:733-743` (WP-A's file, **outside my declared set**) still carries the pre-execution
answer to Open Question 2:

```rust
mutation: None,                    // ⇒ hook_tier_support(Mutator, PreToolUse) == Declined
forbidden: &["updatedInput"],      // ⇒ render-time refusal of the field WP-B proved WORKS
```

WP-B settled it by execution: PascalCase `PreToolUse` + `hookSpecificOutput.updatedInput` **applies
the mutation** (`MUT_hso_updatedInput` observed in the tool result). So the row is wrong twice — it
declines the tier *and* forbids the field.

Because `hook_tier_support` is a query (C-021), the seam produces the wrong verdict for copilot
mutator until the row is fixed, and **the fix must land in the table, not here.** Two edits in that
one row:

```rust
mutation: Some("hookSpecificOutput.updatedInput"),
forbidden: &[],
```

I did **not** special-case copilot in `hook_tier_support`, and Specify must not either: that branch
would be the second copy of the projection data C-021 exists to prevent. **Owner: whoever holds
`src/oci/hook.rs` (WP-A / lead).** Until then, a Specify test asserting "copilot mutator is Native"
will fail — correctly, and it should be written that way rather than to the stale row.

### B2 — ⛔ C-018's matcher allowlist forbids the `|` that C-025 calls lossless

`MATCHER_ALLOWED = "A-Za-z0-9_*?./-"` and `matcher_char_allowed` (`src/oci/hook.rs:80-96`) admit no
`|`, so the **`A|B` alternation form is unauthorable at `grim build`** — while §6.3's translation
table lists it as one of the three lossless forms and WP-B verified it fires on all three clients.
One of the two contracts must move. Recommendation: **add `|` to both** (one character, additive,
`|` is inert in a shell single-quoted string and never reaches one anyway under C-018b). My
classifier accepts alternation as C-025 specifies, so that branch is currently unreachable — a
Specify test for it will fail until the allowlist moves. **Owner: `src/oci/hook.rs`.**

### B3 — `.` passes C-018 but is a regex metacharacter (documented narrowing, not blocked)

`.` is in `MATCHER_ALLOWED`, and claude/codex matcher fields are regexes, so an "exact name"
containing `.` would silently match **more** tools than it names. `classify_matcher` therefore
returns `NotTranslatable` for it ⇒ `Declined`. The alternative (escape `.` → `\.` per client) makes
the translation client-dependent, since copilot's PascalCase dialect is literal-name matching and
would take `\.` literally — more machinery, more risk, zero v1 benefit: no tool name on any of the
three clients contains a `.`. Costs nothing today, fails safe. Needs one line in the published
format doc (**WP-M/WP-L**), because it narrows what a published matcher may contain below what
`grim build` accepts.

### B4 — the empty matcher needs a build-time refusal

`matcher = ""` passes C-018 (charset vacuously, length under the cap) but Copilot **rejects and
skips** it while Claude treats it as match-all. No translation is both faithful and non-skipped, so
the seam returns `HookDecline::MatcherEmpty`. That is a backstop, not the right place: it should be
`HookManifest::validate` rule 3 (**WP-A / Specify**), which would make the variant unreachable.
Recorded on the variant's own doc so the duplication is deliberate.

### B5 — native-only moments (`<vendor>.event`) have no path through this seam

`hook_tier_support` and `hook_registration` both key on `CanonicalEvent`, so an entry whose only
moment is a `<vendor>.event` override cannot be registered by this seam — it would be silently
un-registered rather than declined. I did **not** invent a second lookup: the plan's sanctioned
route is to **widen `ProjectionRow.event` to a native-event-aware key**, never a parallel table
(C-021). Two acceptable answers, and **the choice is owed before Specify**: (a) widen the key in
`src/oci/hook.rs`, or (b) decide v1 does not register native-only moments and make that an explicit
`HookDecline` variant plus a `grim build` warning. Either is additive; leaving it unstated is the
one option that ships a silent gap. **Owner: lead / WP-K.**

### B6 — `command_windows` for claude is a v1 coincidence, flagged for WP-I

Claude Code has exactly one `command` string and no `commandWindows`/`powershell` field (WP-B row
4), so the shared body derives `command_windows: None` from `SpliceConfig` and `Some(..)` from
`OwnFile`. That correlation is a v1 fact, not a law — it holds because claude is the only
`SpliceConfig` client today. Documented in the method; **re-check when a fourth client lands.**
Adding a fifth trait method for it would have exceeded the declared surface.

### B7 — watchlist rows I could not write (`.claude/rules/vendor-capability-watchlist.md` is outside my file set)

Each is recorded in the relevant vendor's doc comment; all five want a dated watchlist row:

1. **claude — `settings.local.json` gitignored *by the client*: UNVERIFIED** (WP-B row S4; the probe
   file was hand-written). Invariant **I1** leans on it for claude·project, the only repo-resident
   registration grim ships. Highest-value item on this list.
2. **codex — a `fish`/`nushell` `$SHELL` cannot execute the guard at all.** Codex runs hooks through
   `$SHELL -lc`, and `L='…';` is not fish syntax, so the hook silently never fires for a supported
   developer configuration. No shell selector exists in the registration.
3. **codex/copilot — Windows runtime forms UNVERIFIED** (`commandWindows` and `powershell` both
   *load* and were schema-accepted on Linux; no Windows host was available).
4. **copilot — transcript display mismatch:** with a mutation applied, Copilot's own transcript
   shows the **original** command while executing the rewritten one. Accepted disclosed residual;
   it is what mutator control 5 (S-016) exists to compensate for.
5. **codex — a correct registration is not an armed hook.** Hooks require human approval in the
   interactive `/hooks` TUI, there is no scripted verb, and an unapproved hook is skipped
   **silently**. Already owned by WP-H/C-017; noted here because `hook_surface` is where a reader
   will ask.

---

## 5. The ownership marker (lead constraint, WP-D finding N-1)

Two frozen constants in `src/install/vendor.rs`:

```rust
pub const HOOK_MARKER_KEY:   &str = "com.grimoire.managed";
pub const HOOK_MARKER_VALUE: &str = "hook-dispatcher";
```

**The value is a grim constant** — not the artifact name, not the scope, not the workspace path, not
`$GRIM_HOME`. The reason is the one the lead names: the `owner` predicate has to match a
registration whose artifact has **already left install state**, so an artifact-derived value cannot
form it and the registration would stay armed forever in a user-owned file. `hook-dispatcher` also
says what it is — one dispatcher, not one entry per hook (Decision H) — which is the correct reading
of the element it marks.

A string rather than `true`, so the predicate stays meaningful if grim ever manages a second kind of
element in the same config; **unversioned**, so it can never need to change. Both strings are
**frozen under Principle 9**: they live in a file grim does not own, and changing either makes every
already-written registration invisible to the `owner` predicate — unreapable, with no reaper that
could sweep the old spelling. Artifact identity or a version, if ever needed, goes in a different
member that is neither an identity key nor an `owner` field — though under Decision L it should not
be needed at all: nothing in a registration is recorded, one dispatcher serves every hook, and the
dispatch table is what maps `(event, matcher)` to payloads.

### 5.1 ⛔ RETRACTED 2026-08-17 — both refinements below were wrong; see § 7 (F-1)

**The marker goes on the HANDLER ELEMENT with `identity_keys = [HOOK_MARKER_KEY]` alone.** The
group-level placement and the two-key identity in this subsection cannot be driven by WP-D's merged
primitive in either direction, and the "identity ≠ ownership" asymmetry I built dissolves. The two
frozen constants are unchanged. Kept in place, struck rather than deleted, because § 7 is the record
of *why* — and because a later reader who finds the group-level argument attractive needs to meet its
refutation, not its absence.

### ~~5.1 Two refinements to the framing, both load-bearing~~ (retracted)

**(a) The marker goes on the array element the splice addresses — which for claude is the
matcher GROUP, not the handler object.** Claude's shape is
`hooks.<Event>[] = {matcher, hooks:[{type, command, timeout?}]}`. grim owns a whole group per
`(event, matcher)`; the handler object stays exactly the client's own shape, so grim never
interleaves a member into the object Claude validates hardest, and never mutates an array element
the user authored (Claude runs every matching group, so the semantics are identical). This also
shrinks the tolerance question in §5.2 — the marker sits one level above the handler.

**(b) Identity and ownership are different predicates and must not be collapsed.**

- `identity_keys = [HOOK_MARKER_KEY, "matcher"]`. The event is already in the JSON pointer, but
  **several** grim groups can share one event with different matchers (Decision H is per
  `(client, event, scope, matcher)`), so the marker alone does not identify an element. `matcher` is
  safe here exactly where the struck `["type","command"]` was not: it is publisher-authored data,
  charset-validated by C-018, and **not environment-derived**.
- **The `owner` predicate is the marker ALONE** — never marker-plus-matcher. A group whose matcher
  changed in a new artifact version, or whose artifact is gone, must still be enumerable. Only the
  ownership predicate has to survive the artifact; identity does not.

**`["type","command"]` is disqualified and I concur** — the command embeds the launcher path and
`--root <abs workspace>`, so a workspace move or a `$GRIM_HOME` change breaks identity and orphans
the element. It is the same defect class as the executed-path finding: ownership must never be
environment-derived.

### 5.2 (1) Does each client tolerate an unknown member? — the question is smaller than it looks

**Only claude matters, and it is UNVERIFIED.** WP-B's evidence settles codex and nothing else:

| Client | Surface | Marker needed? | Unknown-member evidence |
|---|---|---|---|
| **claude** | `SpliceConfig` | **yes** | **UNVERIFIED.** WP-B's hooks executed from a hand-written `settings.local.json`, but no probe carried an unrecognized member. |
| codex | `OwnFile` | **no** | Settled by execution: unknown **handler** field *silently accepted, hook ran*; unknown **event name** silently ignored; unknown **top-level** field ⇒ `deny_unknown_fields`, **every hook in the file dropped** (`research_hooks_launcher_verification.md` § 2.2). Moot, but it says a top-level marker there would be catastrophic. |
| copilot | `OwnFile` | **no** | **UNVERIFIED**, and it would be the risky one: copilot validates per field with skip-on-error (`hooks.sessionStart[0].exec: Expected string — hook will be skipped`) and is **fail-closed** on `preToolUse`. Moot. |

`OwnFile` clients need no marker at all — grim owns the whole file, so ownership is the **path** and
reaping is regenerating or removing it. That is the whole reason copilot's fail-closed strictness
never becomes a risk here: grim puts no marker in its file. The tempting symmetry ("stamp it
everywhere") is the wrong instinct, and it is recorded as such on the constant.

**What settles claude — one run on WP-B's reproducible harness (§ 10):** write
`"com.grimoire.managed": "hook-dispatcher"` into a `hooks.PreToolUse[]` **group** object in
`.claude/settings.local.json` beside `matcher`/`hooks`, force a tool call, and observe (i) the hook
still fires and (ii) whether Claude logs a settings warning. A warning without a skip is
acceptable-but-ugly and should be reported; a skip means the marker cannot live there.

**Fallback ladder if claude rejects it**, in order, so nobody has to re-derive it:

1. Marker on the **handler** object instead of the group (codex tolerates that level; claude
   unverified either way).
2. If both are rejected, ownership falls back to a **structural** predicate — a handler whose
   `command` begins with grim's launcher-directory prefix. This is a **last resort with a disclosed
   residual**, not a plan: it is environment-derived, so a `$GRIM_HOME` move orphans every existing
   registration, which is the very failure the constant-marker rule exists to prevent.

Adding the claude probe to the watchlist alongside item B7.1 (the `settings.local.json` gitignore
question) is worth doing — both are one interactive Claude Code run in a scratch repo, and I1 leans
on the second one.

---

## 6. Nothing else was touched (stub phase)

No changes to `src/oci/hook.rs`, the installer, `candidate_anchors`, the docs, or the catalog. No
commit. The three requirements from WP-B's executed verdict are encoded as follows: **(1)**
PascalCase structurally, via a derived `hook_event_name` no vendor overrides; **(2)** interior `*`
⇒ `MatcherForm::NotTranslatable` ⇒ `HookDecline::MatcherNotLossless` on all three clients; **(3)**
copilot `mutator` supported — encoded as a table query that will return `Native` the moment B1's
one-row fix lands, and deliberately **not** as a per-vendor special case.

---

## 7. Re-stub (2026-08-17) — response to the post-stub review's 3 Blocks

**Gates re-run after every change below:** `cargo check --all-targets` **0 warnings** ·
`cargo clippy --locked --all-targets -- -D warnings` **pass** · `cargo test --bin grim` **2685
passed, 0 failed** · `cargo fmt --check` clean. Same 4 files, still nothing outside the declared set,
still not committed.

**The review is right on all three, and it did not need to persuade me on F-1:** I re-read the
**merged** `src/install/json_splice.rs` (from `hex/hooks-artifact-kind` — it is **not** in this
worktree, which is why I designed against an inferred shape rather than the real one) and WP-D's own
`NestedHandlerPath::identity_keys` doc prescribes, verbatim, *"one stable grim-owned marker member
stamped on every **element** grim writes, serving as **both** the identity key here and the `owner`
predicate"* and *"A constant marker is unambiguous as an identity key because there is at most one
grim-owned element per group."* My § 5.1 argued against a contract I had not read.

### 7.1 F-1 — marker moved to the handler element

`HOOK_MARKER_KEY`'s doc is rewritten (`src/install/vendor.rs`):

| | Retracted (§ 5.1) | Re-stubbed |
|---|---|---|
| Marker sits on | the matcher **group** | the **handler element** `{type, command, timeout?, com.grimoire.managed}` |
| `identity_keys` | `[HOOK_MARKER_KEY, "matcher"]` | **`[HOOK_MARKER_KEY]`** alone |
| `owner` | marker alone | **`[(HOOK_MARKER_KEY, HOOK_MARKER_VALUE)]`** — the same one member |
| Identity vs ownership | deliberately asymmetric | **agree by construction**, which is what WP-D says the pair is for |
| Fallback ladder | group → handler → structural | **handler → group**, and only if the claude probe shows the handler object rejects an unknown member |

Each of the review's four mechanisms is now recorded *in the doc*, not just in the report: the
`InvalidData` contract fires on the **element**; `owned_nested_handlers` matches `owner` against
**elements** (a group-level marker owns nothing → D-1 re-opened one level up); `group_value` already
selects the group *before* identity is consulted, so the second key was a category error; and there is
no group-level write to call. The "Claude validates the handler hardest" benefit is labelled
**asserted, not evidenced** — both levels are unverified on claude, and codex, the only executed
evidence, tolerates an unknown *handler* field. `["type","command"]` is recorded as disqualified.
Grim still owns the whole group when it creates one. **Both frozen strings unchanged.**

### 7.2 F-2 — DECISION: the Decision K seam lands **here**, option (a)

Three additions in `src/install/vendor.rs`, no fifth trait method:

1. **`HookDecline::MutatorOnShellCommandTool`** with a `reason()` line, documented as refused per
   `(tool, matcher)` — and *why* it cannot live in `hook_tier_support`: that signature is tool-blind,
   and the permitted-field table "gates field names, not contents", so a rewritten
   `{"command": "..."}` is a well-formed `updatedInput` nothing upstream catches. CVE-2023-22809's
   shape, named in the variant doc.
2. **`SHELL_COMMAND_TOOLS`** — the per-client roster, keyed on `Vendor::name()`, following
   `POOL_CAPABLE_VENDORS`' shape. All three entries are `Bash` **today**, and the doc says why that is
   not a shared constant: codex's tool is `exec_command` on the wire and is **renamed to `Bash` in the
   hook payload**, so the roster holds *the name the matcher is compared against*, which is per client
   by construction. A client absent from the table contributes nothing — correct for the 15.
3. **`matcher_may_select_shell_command_tool(client, matcher)`** — the predicate, stubbed, with its
   full rule table in the doc so Specify generates cases from it. Conservative in one direction on
   purpose: `All`, `Empty` and `NotTranslatable` all answer **true**; `ExactOrAlternation` answers true
   iff some alternative is a **case-insensitive prefix** of a roster tool. Both relaxations are forced
   by F-3's executed residuals, not by caution — a false `true` costs one legible decline, a false
   `false` ships the rewrite path.

**Refusal order** now `NoSurface → SurfaceUnimplemented → EventUnsupported → TierUnsupported →
MutatorOnShellCommandTool → MatcherEmpty / MatcherNotLossless`, ahead of the two matcher refusals as
directed. The doc states that the position changes no verdict (a bad matcher on a mutator declines
either way) — only which reason the author is told, and that this arm must never be reordered behind a
check that could later be relaxed.

**And the honest half of option (b), recorded even though I chose (a):** `hook_tier_support(Mutator,
PreToolUse)` still answers `Native` on claude and copilot, because the event-level capability question
*is* `Native` — the registration is what declines. A new **"Not the arming authority for `mutator`"**
section on `hook_tier_support` states that a consumer reporting arming, or filling a compat-matrix
cell, from that method alone would show a mutator as available where nothing is registered — an S-013
silent-guardrail report. **WP-L and Specify must read the verdict off `hook_registration`.**

With the lead's `RESPONSE_PROJECTION` fix (my B1) plus this seam, `mutator` + `matcher = "Bash"` on
claude and copilot now resolves **Declined**, and the reason names Decision K.

### 7.3 F-3 — the lossless claim is narrowed to what is true

`classify_matcher`'s doc no longer claims the three forms "agree" in result. It now says **identity is
the only portable translation and it is not lossless**, and records both residuals with their
consequences:

1. **Prefix breadth** — claude/codex are start-anchored but tail-**open** (`Ba*` fires, `^Bash$`
   fires), so an exact name is a prefix match there and a literal match on copilot; `Bash` is a real
   prefix of `BashOutput`. Fail-safe for observer/gatekeeper, **not** for mutator — which is exactly
   why the Decision K predicate is prefix-aware.
2. **Copilot PascalCase is case-INsensitive, claude is case-sensitive** — `matcher = "bash"` installs,
   fires on copilot, is silently inert on claude/codex. Canonical PascalCase tool names are therefore
   an **authoring requirement owed to WP-M's format doc**, not something this seam can enforce.

Both are marked undetectable by a client-independent classifier (grim holds no tool roster).
`^(?:NAME)$` anchoring is explicitly forbidden in the doc with the evidence — `^Bash$` fires on claude
and not on codex or copilot-PascalCase, so anchoring is *less* portable than identity. **Specify pins
the residuals; it does not hunt a per-client branch.**

### 7.4 Warns and Suggests

| # | Applied |
|---|---|
| **F-4** | One paragraph on `hook_surface`: the `Hook` arm replaces the `kind_support` **conjunct only** and must keep `&& kind_surface(kind, scope)`; names the `Mcp`/`Bundle` arms as the misleading precedent, I1/T3 as the consequence, WP-J2 as the owner, and states that this module's test pins `kind_surface` *directly* and so cannot catch the omission — **Specify's pinned-set test must run through `client_supports_kind` at both scopes.** |
| **F-8** | Freeze argument corrected: `owned_nested_handlers` takes an `owner` **slice**, so a dual-value predicate is a perfectly ordinary additive migration here. The doc now argues *"freezing avoids taking on that dual-predicate reaper obligation"* rather than the false *"no reaper could sweep the old spelling"*. |
| **F-10** | `vendor_copilot.rs` no longer restates `hookSpecificOutput.updatedInput`; it points at the `mutation` column of copilot's `PreToolUse` row in `RESPONSE_PROJECTION` and says why (C-021; a name copied into prose goes stale silently). Also notes Decision K's per-tool refusal there. |
| **F-9** | **Not applied — it is Specify's, and stated as an instruction:** one loop asserting `hook_event_name(e) == Some(e.as_str())` for all 18 clients × 4 events, plus a `grep`-free assertion that no vendor overrides `hook_registration`. Without it, "PascalCase is structural" and "one assembly site" are verified by grep only. |
| **F-5 / F-7** | Ownership re-parked below. |

### 7.5 Re-parked owners (F-5) and one promoted gate (F-7)

| Item | Was | **Now** |
|---|---|---|
| B1 (copilot row) | lead | ✅ **fixed by the lead** on the feature branch (`hook.rs:754-755`) |
| B2 (`\|` in `MATCHER_ALLOWED`) | lead | ✅ **fixed by the lead** (`hook.rs:80`, `:102`) — C-018 and C-025 now agree |
| B3 (`.` narrowing → format doc) | WP-M/WP-L | **WP-M only** — WP-L's file set is `client_target.rs` + `docs/src/clients.md`, not the format doc |
| B4 (empty matcher → `validate` rule 3) | WP-A / Specify | **Dead owner. The lead patches `src/oci/hook.rs`**, the same route B1/B2 took — WP-A is merged |
| B5 (native-only `<vendor>.event`) | lead / WP-K | **Lead only.** WP-K owns `src/command/hook.rs`, so it *cannot* widen `ProjectionRow.event`. Still the consequential one: such an entry has no path through `hook_registration`, so a caller must skip it with **no `HookDecline` and nothing in `grim status`** — the silent gap S-013 exists to prevent |
| B6 (`command_windows` correlation) | WP-I | ✔ unchanged |
| B7 (5 watchlist rows) | "outside my file set" | **WP-M owns `vendor-capability-watchlist.md`**; its obligation list currently names only B7.2 (`$SHELL`), so **four rows are unassigned** |
| **B7.1** (claude gitignores `settings.local.json`) | watchlist row | **⛔ PROMOTED TO A GATE.** It is the *sole* premise for A1's claude-only project scope, `vendor_claude.rs` states it as fact, and it is UNVERIFIED. If false, the one repo-resident registration grim ships violates **I1** and A1 collapses to global-only for all three clients. **Blocking verification before WP-I/WP-J2 arm project scope** — one interactive run in a scratch repo, the same run that answers the § 5.2 marker-tolerance probe |

### 7.6 Owed to the lead (plan edits, outside my file set)

- **F-6** — `plan_hooks_artifact_kind.md:1248` still reads *"copilot `mutator` ⇒ `Declined`"*, which
  contradicts requirement 3 at :1176 and the row the lead just fixed. **Implement reads the Implement
  bullet**, so B1's fix is undone by the plan until that line changes.
- **F-1's plan half** — the adopted refinements (a)/(b) at plan:1202-1215 carry the retracted
  group-level design, and `["type","command"]` appears there as an example; both need the correction
  above.
