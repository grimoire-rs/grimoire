# WP-F post-stub review — `reviewer:spec` + `architect`, one pass

**Target:** `.agents/worktrees/wp-f` working tree (4 files, 643 insertions, uncommitted)
**Method:** constraints re-derived from `adr_hooks_support.md` (Amendments 2026-08-16 first),
`plan_hooks_artifact_kind.md` § WP-F, `research_hooks_launcher_verification.md` §§ 3, 6.3, and the
**merged** `src/install/json_splice.rs` / `src/install/installer.rs` contracts — then compared with
the author's report. The report's claims are treated as claims.

## Verdict: **STOP AND RE-STUB** — scoped to three items, all inside the declared file set

| Severity | Count |
|---|---|
| **Block** | 3 |
| **Warn** | 4 |
| **Suggest** | 3 |

The seam's *shape* is right: Decision A's opt-in inversion is correctly built, C-021 holds with zero
second copies, no `HookSurface` variant panics, `HookRoot` closes the `--root` type, and the
18-vendor resolution is correct row-for-row (my independent enumeration below agrees, including the
author's off-by-one correction to the ADR). What must be re-stubbed is three contracts the stub
*documents* wrongly — and one of them is the very defect that sent WP-D back for a re-stub.

**Gates not re-run.** No `target/` exists in this worktree and the volume is at 97 % (5.8 GB free), so
a cold `cargo check --all-targets` was not attempted. The author's gate table is therefore
unverified by me. Note independently that `hook_tier_support`, `classify_matcher` and
`hook_registration`'s tail are `unimplemented!()`, and `projection_for` is a WP-A stub, so
"2685 passed" exercises none of the new seam.

---

## Block

### F-1 — ⛔ The marker on the matcher GROUP is incompatible with WP-D's **merged** primitive, in both directions

`src/install/vendor.rs:109-145` (report § 5.1, adopted into the plan at lines 1202-1215) decides:
marker on the **group** object, `identity_keys = [HOOK_MARKER_KEY, "matcher"]`, `owner` = the marker
alone. Re-derived against `src/install/json_splice.rs` as merged, that cannot be driven:

1. **`upsert_nested_handler` refuses every call.** Its documented `InvalidData` contract
   (`json_splice.rs:451-454`) fires when *"`handler` itself lacks one of `path.identity_keys`"* —
   and `handler` is the **element**. Under refinement (a) the handler object "stays exactly the
   client's own `{type, command, timeout?}` shape", so it carries neither
   `com.grimoire.managed` nor `matcher`. Both identity keys are unsatisfiable on the object the
   primitive tests.
2. **`owned_nested_handlers` enumerates nothing.** It matches `owner` against **elements**
   (`json_splice.rs:520-552`, *"Every element under `path` that grim owns"*). A group-level marker is
   invisible to it, so the reap driver owns nothing — which is **exactly the unreapable-registration
   hole (D-1) the constant marker was chosen to close**, re-opened one level up.
3. **`"matcher"` in `identity_keys` is a category error against this primitive.**
   `NestedHandlerPath` already carries `group_value` — the value at `group_key`, i.e. the matcher —
   as a *separate* field used to select the group **before** identity is consulted. So the report's
   argument for the second key ("several grim groups can share one event with different matchers, so
   the marker alone does not identify an element") is answered by the primitive's own design: identity
   is only ever resolved *within one already-selected group*. WP-D says so explicitly
   (`json_splice.rs:413-415`): *"A constant marker is unambiguous as an identity key because there is
   at most one grim-owned element per group."*
4. **No group-level write or group-level ownership read exists.** `NestedGroupPath` was split out
   *solely* so `owned_nested_handlers` can address every group under one member
   (`json_splice.rs:350-353`). There is no `upsert_nested_group`, no `remove_nested_group`, no
   `owned_nested_groups`. Adding one means editing `json_splice.rs` — a **merged** WP-D file with no
   live owner, which is the B1/B2 route again.

WP-D's intent is unambiguous and self-consistent: `owned_nested_handlers` returns each owned element
*paired with its group's `group_key` value* precisely so the reaper can read the matcher off the
group and feed it back as `group_value` to `remove_nested_handler`. That loop is complete — **and it
only closes with the marker on the element.**

The stated benefit of the group placement ("grim never interleaves a member into the object Claude
validates hardest") is **asserted, not evidenced**: both levels are UNVERIFIED on claude (report
§ 5.2 concedes this), while codex — the one client with executed evidence — tolerates an unknown
**handler** field and fails catastrophically only at the *top* level, and copilot takes no marker at
all. So the placement trades a concrete, merged-contract incompatibility for a speculative benefit.

**Re-stub:** marker on the **handler element**; `identity_keys = [HOOK_MARKER_KEY]` alone;
`owner = [(HOOK_MARKER_KEY, HOOK_MARKER_VALUE)]`. Identity and ownership then agree by construction,
which is what WP-D says the pair is *for* (`json_splice.rs:396-402`), and the asymmetry the report
builds in § 5.1(b) dissolves. Keep grim owning the whole group when it *creates* one — that is
unaffected. Correct the plan's adopted refinements (a)/(b) in the same pass, and invert the fallback
ladder: escalate to the group level **only if** the claude probe shows the handler object rejects an
unknown member. The two frozen string constants themselves need no change.

### F-2 — ⛔ Decision K has no seam, no variant, and no escalation — `hook_tier_support` is tool-blind by signature

The plan's own WP-F Implement bullet (line 1249) assigns it: *"`mutator` ⇒ `Declined` for
shell-command-string tools (Decision K)"*. ADR Decision K (lines 811-830) is explicit that the
permitted-field table *"gates field names, not contents"* and is therefore **insufficient**, that the
decline is per **tool shape**, that it is *"enforced in the same render-time table … pinned by a
test"*, and that this is one of three controls the research *"would not ship the tier without"* —
the `sudo` CVE-2023-22809 shape.

`fn hook_tier_support(&self, tier: HookTier, event: CanonicalEvent) -> KindSupport`
(`vendor.rs:618`) takes **no tool and no matcher**, so Decision K is unexpressible through it. The
rule-3 doc comment (`vendor.rs:591-616`) does not mention it. `HookDecline` (`vendor.rs:220-252`) has
no variant for it and the documented refusal order (`vendor.rs:678-685`) has no arm for it. The
report's B1–B7 list does not name it. `hook_registration` is the only matcher-aware entry point and
its refusal ladder stops at `MatcherEmpty`/`MatcherNotLossless`.

Consequence if implemented as stubbed: with the copilot row now fixed, `mutator` at `PreToolUse`
resolves **`Native` on claude and copilot for `matcher = "Bash"`** — grim ships the string-rewrite
path the ADR refused to ship. "Each vendor's equivalent [of Bash]" is per-client data, so its natural
home *is* this seam.

**Re-stub:** either (a) add the seam — a `HookDecline` variant plus the per-vendor
shell-command-string tool roster, with the refusal ordered before `MatcherEmpty`; or (b) record it as
an explicit B-item with a *live* owner and a named alternative home (WP-K's runtime projector is the
only other candidate, and choosing it means `hook_tier_support` reports `Native` for a control that
no-ops at runtime — an S-013 silent-guardrail case that must then be stated). Not deciding is the one
option that ships the CVE shape.

### F-3 — ⛔ `classify_matcher`'s "the three lossless forms agree" is contradicted by the source it cites

`vendor.rs:307-315`: *"**Client-independent by result, not by assumption.** WP-B ran the same matcher
matrix against all three v1 clients and the three lossless forms agree, so v1 needs no per-client
branch."* `research_hooks_launcher_verification.md` § 3.2 says otherwise, in two ways
`classify_matcher` cannot detect:

1. **claude and codex are start-anchored but tail-OPEN.** § 3.2: *"`Ba*` fires (matches at position 0)
   but `as` does not … `^Bash$` fires, so the end is not forced."* So an exact name is a **prefix**
   match there and a **literal** match on copilot PascalCase. `Bash` is a real prefix of Claude
   Code's real `BashOutput` tool — one manifest, two different tool sets. Fail-safe for
   `observer`/`gatekeeper` (fires more), **not** for `mutator`, which then rewrites the input of a
   tool the author never named.
2. **copilot PascalCase is case-INsensitive; claude is case-sensitive.** § 3.2: *"literal names match
   case-insensitively (`bash` matches `Bash`)"*; § 3.1 row `"bash"`: claude *"does not fire"*. So
   `matcher = "bash"` installs, reports installed, fires on copilot, and is **silently inert on
   claude and codex** — the same silent-guardrail class WP-B requirement 1 exists to prevent, reached
   through casing instead of dialect.

Neither is detectable by a client-independent classifier (grim has no tool roster), and § 6.3's
"translates 1:1" table glosses both — so this is not the author inventing a claim, but it *is* a
verified-without-evidence statement on the contract C-025's "never approximated" guarantee rests on,
and the plan's Specify line (1254-1256) demands *"per-client tests including **anchoring**"* against
a seam that has no per-client translation step at all.

**Re-stub:** narrow the claim to what is true — *identity is the only portable translation, and it is
not lossless* — and record the two residuals on `classify_matcher` (prefix breadth on claude/codex;
copilot's case-insensitivity, hence canonical PascalCase tool names are an authoring requirement for
WP-M). Then tell Specify to pin the residuals rather than hunt a per-client branch. Do **not** try
`^(?:NAME)$` anchoring: § 3.1 shows `^Bash$` fires on claude and does **not** fire on codex or
copilot-PascalCase, so anchoring is less portable than identity.

---

## Warn

### F-4 — The `client_supports_kind` composition is the real gate, and nothing holds it

Decision A's wording — *"`client_supports_kind` special-cases `Hook` to `hook_surface().is_some()`
and **never consults `kind_support` for it**"* — says nothing about `kind_surface`. The shipped
function (`installer.rs:1100-1123`) is a `match kind` whose `Mcp` and `Bundle` arms **return without
consulting `kind_surface`**; only the catch-all arm composes the two. An implementer adding
`ArtifactKind::Hook => vendor.hook_surface().is_some()` beside them — the literal reading of the ADR
— drops the scope gate and arms **codex and copilot at project scope**, into tracked repository
files: invariant **I1**, attacker **T3**.

The test added in this diff pins `kind_surface` *directly*, so it would not fail. Two facts make this
concrete rather than hypothetical:

- Today on this branch `client_supports_kind(zed, Hook, …) == true` — zed's `kind_support` closes on
  a `_ => Native` wildcard and `kind_surface` defaults `true`. **The opt-in seam alone does not
  invert the default**; only the not-yet-written `installer.rs` arm does. (Latent, not live:
  `installer.rs:2446` / `:2466` refuse `ArtifactKind::Hook` outright today.)
- The owner exists (WP-J2 owns `installer.rs`) but the *ordering hazard* is recorded nowhere — not in
  the ADR, not in the plan, not in the report's B-list, only obliquely in a `dead_code` reason.

**Carry forward:** one sentence on `Vendor::hook_surface` stating that the `Hook` arm replaces the
`kind_support` conjunct **only** and must keep `&& kind_surface(kind, scope)`; and Specify's
pinned-set test must run through `client_supports_kind` at **both** scopes, not through
`hook_surface`, or it proves nothing about installation.

### F-5 — Three of the five parked items (B3–B7) carry dead or wrong owners

| Item | Recorded owner | Re-derived owner |
|---|---|---|
| B3 (`.` narrowing → published format doc) | "WP-M/WP-L" | **WP-M** only. WP-L's file set is `client_target.rs` (tests) + `docs/src/clients.md` — not the format doc. |
| B4 (empty matcher → `HookManifest::validate` rule 3) | "WP-A / Specify" | **Dead.** `HookManifest::validate` is in `src/oci/hook.rs`; WP-A is **merged**. Same route B1/B2 took: lead patches the file. |
| B5 (native-only `<vendor>.event` moments) | "lead / WP-K" | **Lead** is right; **WP-K is wrong** — its file set is `src/command/hook.rs` + submodules, not `src/oci/hook.rs`, so it cannot widen `ProjectionRow.event`. |
| B6 (`command_windows` correlation) | WP-I | ✔ correct — WP-I owns `vendor_{claude,codex,copilot}.rs`. |
| B7 (5 watchlist rows) | "outside my file set" | **WP-M owns `.claude/rules/vendor-capability-watchlist.md`.** Its obligation list currently names only *WP-B's `$SHELL` entry* (= B7.2); the other **four** rows are unassigned. |

B5 is the consequential one: an entry whose only moment is a `<vendor>.event` override has no path
through `hook_registration` (which keys on `CanonicalEvent`), so a caller must skip it — with **no
`HookDecline`, hence nothing in `grim status`**. That is a silent gap, which is what S-013 exists to
prevent. The escalation is right; half its ownership is not.

### F-6 — The plan's own Implement bullet still declines copilot `mutator`

`plan_hooks_artifact_kind.md:1248`: *"per-tier verdicts from C-004 (copilot `mutator` ⇒ `Declined`,
Open Question 2)"* — stale, contradicting requirement 3 at line 1176 and the `RESPONSE_PROJECTION`
row the lead just fixed. Implement reads the Implement bullet. Correct the line, or B1's fix is
undone by the plan.

### F-7 — B7.1 is a gate, not a watchlist row

That Claude Code itself gitignores `.claude/settings.local.json` is **UNVERIFIED** (WP-B § 9 — the
probe file was hand-written), and it is the *sole* premise for A1's claude-only project scope
(`vendor_claude.rs:196-199` states it as fact). If it is false, the one repo-resident registration
grim ships violates **I1** and A1 collapses to global-only for all three clients. The author names it
"highest-value item on this list" and then files it as a dated row. It belongs as a **blocking
verification before WP-I/WP-J2 arm project scope** — one interactive run in a scratch repo.

---

## Suggest

- **F-8 — the Principle 9 freeze argument is overstated.** *"no reaper that could sweep the old
  spelling"* (`vendor.rs:141-145`, plan:1200) is not true: `owned_nested_handlers` takes an `owner`
  slice, so a dual-value predicate is exactly the additive migration this repo uses elsewhere
  (legacy `open-code-*` tags, `reap_relocated_roots`, the reaped legacy `catalog.json`). The freeze
  is the right call — argue it as *"freezing avoids the dual-predicate reaper obligation"*, not as
  *"no migration exists"*, or the next reviewer discounts a correct conclusion for a wrong reason.
- **F-9 — nothing pins "no vendor overrides `hook_event_name`."** The PascalCase property is claimed
  structural but verified by `grep`. One loop in Specify's pinned-set test — `hook_event_name(e) ==
  Some(e.as_str())` for all 18 clients × 4 events — makes it structural for real. Same for
  `hook_registration`, which the report says no vendor overrides.
- **F-10 — one doc-level copy of a table value.** `vendor_copilot.rs:98` restates the literal
  `hookSpecificOutput.updatedInput`. Not a C-021 violation (it is prose, not data), but it goes stale
  silently; point at the `RESPONSE_PROJECTION` row instead — AGENTS.md's own rule is to fix the
  source, not a restatement of it.

---

## What I verified and found sound

- **Decision A's inversion is correctly built.** `hook_surface()` defaults `None` (`vendor.rs:555`);
  exactly three vendors override; a forgotten vendor declines with zero lines. `hook_event_name`,
  `hook_tier_support` and `hook_registration` all derive their fail-safe half from it, so the
  "forgotten vendor fails safe" property holds *within the seam* (see F-4 for outside it).
- **C-021 holds — no second copy, no special case.** Grepped the whole diff and `src/`: zero
  occurrences of any projection field name, verdict/tier table, `match self.name()`, or per-vendor
  override table outside `src/oci/hook.rs`. The author's refusal to special-case copilot is correct
  and load-bearing.
- **The lead's two fixes are exactly what the seam needed, and added no third copy.** With
  `mutation: Some("hookSpecificOutput.updatedInput")` (`hook.rs:754`), rule 2 finds the required
  field present and rule 3 finds `context` present, so copilot·`Mutator`·`PreToolUse` computes
  `Native` — from the table, with no branch. `forbidden: &[]` (`hook.rs:755`) is required or the
  render-time refusal would reject the field WP-B proved applies; it now matches claude's
  `PreToolUse` row exactly. `MATCHER_ALLOWED` gaining `|` (`hook.rs:80`, `:102`) makes C-018 and
  C-025 agree in the additive direction.
- **No panic on any `HookSurface` variant.** `hook_registration` (`vendor.rs:695-720`) matches all
  three arms and returns `Err(SurfaceUnimplemented)` for `CodegenModule`; `hook_tier_support` reaches
  it only via `projection_for` → `None` → `Declined`. WP-A's convention is honoured. (Separately:
  `client_target.rs:312`/`:364` and `path_anchor.rs:721` `unreachable!()` on `ArtifactKind::Hook`
  become reachable once WP-J2 lands the install branch — WP-A's arms, guarded today by
  `installer.rs:2446`/`:2466`. Not this WP's, worth one line in WP-J2's brief.)
- **`HookSurface` stays scope-blind**, and the A1 gate rides the shipped `kind_surface`/`SCOPE_GAPS`
  seam with both rows pinned and `Hook` added to the existing loop. Correct mechanism, correct
  precedent (Junie-rules-at-global). The gap is the composition, F-4, not the choice.
- **`HookRoot<'a>`** closes `--root` as a two-variant type instead of a `&str` that would accept
  `$PWD` — the right expression of Decision P. **`posix_single_quote` implemented rather than
  stubbed** is the right call: the quoting *is* the C-018b argument and WP-B executed the failure it
  prevents.
- **`Vendor` stays dyn-compatible** with all four additions (no generics, no `Self` in return
  position) — required, since `ClientTarget::vendor()` returns `&'static dyn Vendor`.
- **Principle 9:** everything here is additive — four defaulted trait methods, new pub types, two new
  `SCOPE_GAPS` test rows, a brand-new kind whose scope gate changes no shipped behaviour. One item to
  state rather than fix: an artifact carrying `A|B` published by a newer grim fails an older grim's
  `grim build`. Forward-compat only, same class as adding an enum literal — permitted, and it is the
  lead's change.

## Independent 18-vendor resolution vs the author's

Re-derived from `research_hooks_vendor_survey.md`'s master matrix (rows 38-53 = the 17 surveyed
clients) plus the synthetic `agents`, against `ClientTarget::ALL` (`client_target.rs:251`, **18**).

**My resolution is identical to the author's, row for row: 3 capable / 15 declining.**

| Surface | Clients | Agrees with author |
|---|---|---|
| `Some(SpliceConfig)` | claude | ✔ |
| `Some(OwnFile)` | copilot, codex | ✔ |
| `None` — no hook mechanism at all | **warp, zed** | ✔ (the only two of 17 surveyed) |
| `None` — synthetic, no product behind it | agents | ✔ |
| `None` — splice-shaped, phase 3 | cursor, gemini, droid, antigravity, junie | ✔ (matches the survey's splice list exactly) |
| `None` — own-file/own-dir, phase 3 | kiro, goose, cline | ✔ |
| `None` — JS/TS plugin, `CodegenModule`-shaped, no v1 template | opencode, kilo, amp, openclaw | ✔ |

**The author's off-by-one correction to the ADR is right.** ADR Decision A says *"all 17 vendors"* /
*"Fourteen must decline"*; `ClientTarget::ALL` has 18 and the missing one is the synthetic `agents`,
added after the ADR was written. The correct figures are **18 clients, 3 capable, 15 declining**, and
a pinned-set test written from "fourteen" would be wrong by one row. The trait doc's careful
"the only two of 17 **surveyed** clients" phrasing is consistent with both counts and should be kept.

One imprecision, not a defect: amp is grouped as "JS plugin" but the survey records it as *partial* —
a JS plugin **plus** a legacy shell `delegate` and a splice route into `amp.permissions`. The answer
is `None` either way; the "why" cell is thinner than the evidence.

---

## Required before Specify

1. **F-1** — re-stub the marker onto the handler element; `identity_keys = [HOOK_MARKER_KEY]`;
   correct the plan's refinements (a)/(b); invert the fallback ladder. *(`src/install/vendor.rs` +
   plan)*
2. **F-2** — decide Decision K: seam here, or an explicit B-item with a live owner and the S-013
   consequence stated. *(`src/install/vendor.rs` + plan)*
3. **F-3** — narrow `classify_matcher`'s lossless claim and record the two residuals.
   *(`src/install/vendor.rs`)*
4. **F-4** — one doc sentence on `hook_surface`; Specify's pinned-set test goes through
   `client_supports_kind` at both scopes.
5. **F-6** — fix plan line 1248 before Implement reads it.
6. **F-5 / F-7** — re-park B3/B4/B5 on live owners; add B7's four unassigned rows to WP-M; promote
   B7.1 to a gate on WP-I/WP-J2.
