# ADR: Vendor configuration, shared-pool policy, and client selection

## Metadata

**Status:** Accepted (2026-07-26). D4 was resolved against its original
proposal — a generic fallback client, not an error; see [D4](#d4).
**Date:** 2026-07-26
**Deciders:** maintainer (architect proposal; wave-2 meta-plan `meta-plan_execute_vendor-coverage-wave2`)
**Beads Issue:** N/A
**Related PRD:** N/A
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` — no new dependency, no new tech; Rust + serde + the shipped anchor/prune machinery
**Domain Tags:** integration, api
**Supersedes:** N/A — extends
[`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md),
[`adr_render_layout_stability.md`](./adr_render_layout_stability.md),
[`adr_codex_vendor.md`](./adr_codex_vendor.md)

## Context

Grimoire materializes artifacts into ten client vendors behind the `Vendor`
trait (`src/install/vendor.rs`) and the closed `ClientTarget` identity enum
(`src/install/client_target.rs:184-195`). Four of those ten — Codex, Gemini,
Zed, Amp — already render skills into the **shared** cross-vendor pool
`$HOME/.agents/skills/<name>`; the other six render into their own native
skills directory.

Wave 2 of the vendor-coverage effort raises four coupled questions that must
not land as scattered commits, because each one constrains the others:

1. Should grim move *every* pool-capable client to the shared pool (N copies
   collapse to 1), or keep rendering natively?
2. If pooling stays optional, what config surface expresses "this vendor,
   pooled"? `[options]` already carries a `clients` **list**, so it cannot
   also be a per-vendor table.
3. Is moving a vendor's render layout a compatibility break on the road to
   1.0.0 (Principle 9)?
4. When neither `--client` nor `[options].clients` selects anything and
   detection finds nothing, what should happen? Today grim installs into
   **all ten** clients.

### Evidence base (verified 2026-07-26; do not re-derive)

| Fact | Consequence |
|---|---|
| Skill scanning is **additive** on every client checked — no client replaces its native dir with `.agents/skills` | Native-dir rendering is never invisible. There is no correctness argument for a pool move. |
| The agentskills.io spec is **silent on directory locations** (its own implementer guide says so) | `.agents/skills` is emergent convention, not a mandate; it is not under AAIF governance (that is AGENTS.md — a documented conflation). |
| The global pool path is **contested** — majority `$HOME/.agents/skills`, but Amp and Kimi CLI rank `~/.config/agents/skills` above it | A pool-first move would have to pick a path that a meaningful minority does not read first. |
| Documented non-adopters exist: Cline (native dirs only), LangChain Deep Agents (no filesystem convention by design) | The pool is not universal and cannot be assumed. |
| **No N→1 layout-collapse machinery exists.** The shipped migration precedent (`test/tests/test_global.py:308-360`) is strictly 1→1 | A pool-first move would need migration code that does not exist yet. |

## Decision Drivers

- **Principle 9, stabilization freeze.** Schema evolution is additive-only;
  every new config key, enum literal, and JSON field is permanent.
- **Honest declines over silent writes.** The `KindSupport` tri-state
  (`src/install/vendor.rs:65-81`) established the philosophy: when a vendor
  cannot host something, grim warns and records nothing rather than writing an
  inert file. A pool flag must behave the same way.
- **Never delete a file the user edited.** `docs/src/stability.md:137-140`
  makes this a stated promise for the layout-migration reaper, with no
  `--force` override.
- **Exclusive-write file ownership across worktrees.** Three downstream
  worktrees read this ADR as their spec; contracts must name files, fields,
  and exit codes precisely enough that no two worktrees touch the same file.

## Considered Options

The pool question (1) is the one with real alternatives; decisions 2–5 follow
from whichever is chosen.

### Option 1: Pool-first rendering

**Description:** every pool-capable vendor resolves `skills_root` to
`$HOME/.agents/skills`; native skill dirs are migrated away and reaped.

| Pros | Cons |
|------|------|
| One copy on disk instead of N | Solves a disk-space problem nobody reported — scanning is additive, so nothing is currently broken |
| One path to explain in docs | The pool path is contested (Amp, Kimi CLI prefer `~/.config/agents/skills`) — grim would pick a loser for some users |
| Matches where four vendors already are | Requires N→1 collapse migration that does not exist; the shipped precedent is 1→1 |
| — | A vendor with a non-empty `skill_fields()` registry would leak its fields into a file three other clients read (`src/install/render.rs:849-877`) |

### Option 2: Per-vendor opt-in, default off (chosen)

**Description:** native rendering stays the default; a user may opt an
individual, verified-pool-capable vendor into the shared pool through config.

| Pros | Cons |
|------|------|
| Zero behavior change for every existing install | Two render layouts to support instead of one |
| The user who wants one copy can have it, explicitly | A flip is a layout move and must carry migration |
| Capability gate keeps grim from writing where nothing reads | New config surface is permanent (Principle 9) |

### Option 3: Do nothing

**Description:** leave the four pool vendors as-is, add no config surface.

| Pros | Cons |
|------|------|
| Smallest diff | Leaves no answer for a user with six clients and six copies of the same skill |
| No permanent config surface added | The per-vendor table is needed anyway for future per-vendor knobs (e.g. a `root` override) |

## Decision Outcome

**Chosen option: Option 2**, expressed as five coupled decisions plus two
sub-decisions.

### D1 — Keep native-dir rendering. No pool-first move. {#d1}

Grim continues to render skills into each vendor's native skills directory.
The four vendors already on the shared pool stay there; no vendor is moved
onto or off the pool by default, in either direction.

**Rationale:** every leg of the pool-first argument fails against the evidence
base above. Scanning is additive, so native rendering is never invisible —
there is no correctness defect to fix. The spec that would mandate a location
does not mandate one. The path itself is contested. Documented non-adopters
exist. And the migration machinery a collapse would need has never been
written; the only shipped layout migration is 1→1.

That leaves exactly one benefit — fewer bytes on disk — which is not worth
spending a permanent layout change on.

**Owner principle, recorded 2026-07-26: prefer a vendor-specific directory over
the shared pool wherever one exists.** The pool is the fallback of last resort,
not the default. This is the general form of D1, and it is why [D2](#d2)'s
opt-in defaults off, why [D4](#d4)'s generic client only appears when no vendor
is detected at all, and why a vendor that declares its own skill fields is
disqualified from the pool entirely.

### D2 — `shared_skills`: per-vendor, opt-in, default off, capability-gated {#d2}

A new per-vendor config table:

```toml
[options]
clients = ["claude", "cursor"]

[options.vendors.cursor]
shared_skills = true
```

```rust
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub vendors: BTreeMap<String, VendorOptions>,

#[derive(…, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VendorOptions {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared_skills: bool,
}
```

**Why a nested table and not a flat key.** `[options].clients` is already a
`Vec<String>` (`src/config/declaration.rs:188`). TOML cannot make `clients`
both a list and a table, so per-vendor settings need their own key. `vendors`
is a `BTreeMap` for deterministic serialization order.

**Why this is additive.** `ConfigOptions` carries
`#[serde(deny_unknown_fields)]` (`src/config/declaration.rs:164`), so the key
must exist in the struct to be accepted — but `#[serde(default)]` plus
`skip_serializing_if` means an old config parses unchanged and a config that
never sets it never grows the key. This is the `show_deprecated` precedent in
the same struct.

**The capability gate.** A new `Vendor::pool_capable() -> bool` states whether
the client genuinely reads `.agents/skills`. Setting `shared_skills = true` on
a vendor that returns `false` is an **error, exit 65** — never a silent write
to a directory nothing reads. This is the `KindSupport::Declined` philosophy
applied to a config key rather than an artifact kind.

**Exit codes — three distinct failures, three distinct codes.** `[options].clients`
is the precedent and it is a *two-code* precedent, not one: `check_clients`
(`src/config/project_config.rs:409`) is a single shared validator whose own doc
comment (`:406-408`) records the split — set-time (`grim config set`) maps to
**65**, load-time (`validate_clients`, `:442`) maps to **78** — so the accepted
set can never drift between the two paths. `InstallErrorKind::UnsupportedClient`
likewise classifies to `ExitCode::ConfigError` = 78 (`src/error.rs:356`, locked
by the table test at `:634-638`).

For `[options.vendors]` the vendor name is part of the **key**, not a value,
which adds a third code:

| Failure | Surface | Code |
|---|---|---|
| `grim config set options.vendors.foo.shared_skills true` where `foo` is not a client | CLI key parse — `config_usage` at `src/command/config.rs:214-217` | **64** (unknown key) |
| A bad *value* on a valid vendor key | Set-time validation, `clients_set_error` precedent (`src/command/config.rs:551-557`) | **65** |
| A hand-written `grimoire.toml` names an unknown vendor in `[options.vendors.<name>]` | Load-time validation, `validate_clients` precedent | **78** |
| A known vendor that is not `pool_capable()` is set to `shared_skills = true` | Well-formed config asking for something the vendor cannot do | **65** |

78 means "your config file names something that does not exist"; 65 means
"your config is well-formed but semantically unsatisfiable"; 64 means "you
typed a key that does not exist". All three have precedent in the codebase, and
the set-time/load-time pair must route through one shared validator.

**Precedence.** `shared_skills` has no CLI flag and no `[options]`-level
equivalent, so the chain is short and must be stated as such rather than as
the muddled four-link chain in the W-B sub-plan: **the active scope's
`[options.vendors.<name>].shared_skills` > built-in default (`false`)**.
There is no cross-scope merge: `scope_resolution.rs:6-11` states each command
operates on exactly one scope, "never merged". A project config's
`[options.vendors]` therefore does not merge with the global one — the
resolved scope's table is used whole. W-B must not invent a per-key merge.

**Scope of `VendorOptions`: exactly one field.** A per-vendor `root` override
(which would answer the five env vars `subsystem-file-structure.md` records as
deliberately unhonored) is a plausible future field but is **not in scope**.
Leave the struct shaped so adding it is additive; add nothing speculative
(YAGNI).

### D3 — A layout move is not a compatibility break {#d3}

Flipping `shared_skills` moves where a vendor's skills live. Under the 1.0
stability contract this is explicitly permitted: `docs/src/stability.md:95-102`
excludes vendor render layout from the freeze and names the shared
`$HOME/.agents/skills` pool by name. The supported discovery channel is
`grim status --format json`'s `outputs: [{client, path}]` array
(`docs/src/stability.md:116-125`), not a hardcoded path.

Principle 9 is satisfied by **reusing the three shipped mechanisms**, not by a
version bump:

1. `output_at_current_layout` (`src/install/installer.rs:942-957`) — compares
   the recorded anchor+relative against what the current layout produces; a
   mismatch falls through the integrity gate and forces re-materialization.
2. `reap_moved_outputs` (`src/install/installer.rs:978-1042`) — deletes the
   orphaned old path behind five ordered guards (entry outputs skipped, still-
   produced outputs skipped, unresolvable anchors skipped, hash-mismatch
   preserved, resolved-identity alias protection).
3. A shipped upgrade fixture (`test/tests/test_global.py:308-360`) proving the
   full round trip: re-materialize, reap, re-anchor, `status` reads
   `installed`, `uninstall` cleans up.

**The honest gap — narrower than first stated.** An earlier draft of this ADR
claimed no shipped code covers the N→1 direction. Audited against source
2026-07-26, that is wrong:

- **N→1 convergence within an install pass is shipped and tested.** The four
  pool vendors already collapse onto one destination via the same-pass dedup at
  `installer.rs:650`, guarded by `shared_by_surviving_sibling`
  (`prune.rs:649-685`) and locked by `prune.rs:906/922/938/957`.
- **Both flip directions work.** Off→on: the flipping client's recorded anchor
  mismatches, the install re-materializes at the pool path, and
  `reap_moved_outputs` deletes the old native output while skipping the
  sibling's pool output (guard 2 matches it). On→off: guard 2
  (`installer.rs:985-987`) never reaps a path a *new* output still claims, so a
  pool directory a surviving sibling occupies is preserved structurally.

What is genuinely uncovered is **migrating into an already-occupied pool
path**. The untracked-clobber gate keys on **client, not path**
(`installer.rs:528-533`), so a flipping client counts as tracked, skips the
gate, and `installer.rs:659-661` runs `remove_path(&dest)` on the sibling's live
pool directory before rewriting it. That is safe only while every pool vendor
emits byte-identical bytes. The mitigation is [D2](#d2)'s capability rule, not
new migration code — see the W-E contract, where it is a requirement rather
than a recommendation. No shipped *test* covers this case (all four current
pool vendors were pool-native from day one), so W-E adds one.

### D4 — Zero detected clients fall back to a generic pool client {#d4}

> **ACCEPTED 2026-07-26.** This supersedes the originally proposed
> "zero detected → exit 78". The owner chose a generic fallback client instead;
> the 78 survives only in the narrow case below. D1, D2, D3, and D5 are
> unchanged.

**Decision:** when detection finds no client, grim targets a synthetic generic
client named **`agents`**, whose skills root is `<ws>/.agents/skills` (project)
and `$HOME/.agents/skills` (global).

- **Skills only.** `kind_support` returns `Declined` for `Rule`, `Agent`, and
  `Mcp` — none has a vendor-neutral surface. Warn, skip, record zero outputs,
  exactly as Codex does for rules.
- **Empty `skill_fields()`.** It is a pool writer, so [D2](#d2)'s capability
  rule binds it too; it routes through `render_universal_skill_doc`.
- **The surviving 78.** If the fallback is active and nothing in the artifact
  set is installable — only rules, agents, and/or MCP — exit **78** naming
  `--client`. That is the one case where grim genuinely cannot act.
- **Not persisted.** Recomputed each run, consistent with [D5](#d5).
- **Permanent name.** `agents` appears in `--client`, `[options].clients`, and
  `outputs[].client` — all frozen surfaces (Principle 9).

**Why this fixes the defect without recreating it.** The defect is that the
all-clients fallback *manufactures its own detection signal*: writing eleven
vendor directories is exactly what makes the next run "detect" ten clients.
The pool does not do that, because `.agents/skills` is already treated as a
**weak cross-vendor marker that nothing detects on** — Codex and Gemini
deliberately key detection on their own product directories instead
(`vendor_gemini.rs:100-105`). So the fallback writes a directory whose presence
changes no future resolution.

**Why not the error.** An error is a correct diagnosis of an ambiguous
situation but a poor answer to it: the user asked grim to install something,
and there is a defensible place to put a skill that costs nothing to write and
that several clients already read. Erroring would have been the first rung; the
generic client is the rung that actually holds. The narrow 78 remains for the
case where no defensible place exists.

The rest of this section records the original analysis, which still explains
*why* the old fallback had to go and where the new logic must live.

Today `detect_clients` (`src/install/target.rs:122-132`) falls back to
`ClientTarget::ALL` when detection finds nothing, defended by the doc comment
at `:116-121` as "never targets zero clients or prefers one".

**The argument that comment misses: the fallback is not idempotent with
respect to its own input.** A bare workspace installs into all ten clients,
creating `.claude/`, `.opencode/`, `.github/instructions/`, `.codex/`,
`.cursor/`, `.kiro/`, `.junie/`, `.gemini/`, `.zed/`, `.amp/` and
`.agents/skills/` — which is precisely what makes the *next* run detect all
ten. The fallback writes itself into permanent config-by-side-effect, and the
result is unrecoverable: afterwards nobody, including grim, can distinguish "I
use these clients" from "grim created these directories once." Blast radius
grows with every client added — it was 3 clients when this was written, it is
10 now, and W-G adds more.

The residual 78 uses `ExitCode::ConfigError` (`src/cli/exit_code.rs:46`), with a
message naming `--client` and listing the known client names — reusing
`ClientTarget::VALUE_NAMES` the way `InstallErrorKind::UnsupportedClient`
already does (`src/install/install_error.rs:134-138`), so the message cannot
drift. 78 is also what `UnsupportedClient` classifies to, keeping every "your
client selection is wrong" failure on one code.

**Where the fallback and the residual error live — this is load-bearing.**
Neither may live inside
`detect_clients`, which has read-only consumers that must keep working:
`src/command/status.rs:149` and `src/command/search.rs:252,276` use the
detected set to reconcile recorded outputs (so a client removed since install
does not lie about `missing` files), and `src/tui/app.rs` uses it in six
badge-derivation sites. Those consumers keep today's exact behavior via an
explicit `_or_all` wrapper — a mechanical call-site change with no observable
effect.

Both belong in `InstallTarget::parse` (`src/install/target.rs:64-93`),
the single entry point every mutating command funnels through: `install.rs:78`
and `:282`, `add.rs:533`, `update.rs:122`. `InstallTarget::new` stays
infallible so the ~40 unit-test call sites that pass explicit clients are
untouched.

**Two carve-outs:**

- **`grim tui` must not hard-exit** in the residual-78 case.
  `src/command/tui.rs:325-327` already falls back on a `parse` error; the TUI
  surfaces the condition in-app at `src/tui/app.rs:2339,2472,2564` rather than
  exiting.
- **`grim context` must not error.** It is the diagnostic command for exactly
  this situation ("effective client set"), so its `InstallTarget::parse` call at
  `src/command/context.rs:42` must report the **resolved** set — `["agents"]`
  under the fallback. This is a JSON *value* change, not a shape change.

**Ownership seam.** The vendor surface and the selection rule are split:
**W-A** ships `vendor_agents.rs`, the `ClientTarget::Agents` arm, the
`(Agents, Skill)` anchor row, the `docs/src/clients.md` row (the parity test at
`client_target.rs:611` forces it into the same commit), and the
`render.rs:874` pool-count generalization — the generic client is the fifth pool
writer, so it breaks that counter first. **W-C** ships only *when* the fallback
engages, the residual 78, `context.rs`, and `init.rs` seeding, and its task 1
therefore waits for W-A to merge.

### D5 — Do not auto-persist the detected set {#d5}

The detected set is recomputed each run and never written back to config.
Persisting it would produce config churn and teammate-diff noise every time a
developer opens a different editor. Instead:

- `grim init` seeds `[options].clients` from detection — a config write at the
  exact moment the user asked for a config file. If detection is empty at
  `init` time, write nothing and emit a note pointing at `--client` /
  `options.clients`.
- Nothing else auto-persists.

**The hole this leaves, and its fix.** `reap_dropped_clients` is gated at
`src/command/update.rs:182-192` on `[options].clients` being non-empty. The
gate is correct as written — the comment at `:165-181` explains why it reads
the raw config *before* `InstallTarget::parse` (parse collapses an empty vec
into live detection, destroying the explicit-vs-detected distinction, and
reaping against live detection would delete a still-wanted client's output the
moment a marker dir drifts). But the consequence is that **under autodetect
nothing is ever reaped** (`docs/src/stability.md:145-147`), and
`clients_missing` / `clients_extra` stay `[]` on every `status` row by design —
documented at `src/api/status_report.rs:100-109`, `docs/src/stability.md:52`
and `:61`, and `docs/src/commands.md:445-446`. An autodetect user who uninstalls Cursor
accumulates orphaned files forever and `grim status` says nothing.

**Decision: report the drift, do not widen deletion.** A recorded output for a
client that is no longer detected surfaces in `status` as `clients_extra`.
Reporting is additive and safe. Actually *reaping* under autodetect would make
the desired set track live detection — uninstalling a client's application
would then delete grim's files — and is explicitly **not** adopted.

> **REVERSED 2026-07-26 — the reporting half of this decision is withdrawn.**
> The "do not widen deletion" half stands and is strengthened.
>
> W-C implemented the reporting fix as specified, two independent opus reviewers
> reproduced it broken against a built binary, and it was reverted. Nothing
> shipped; `status` behaviour is unchanged and `clients_extra` still stays `[]`
> under autodetect.
>
> **Root cause: `detect_clients` is not an inverse of "grim installed here", in
> either direction.** The decision above assumed it was. Five vendors render
> skills *outside* their own detect marker — verified directly against
> `src/install/vendor_*.rs`:
>
> | vendor | skills root (project) | detect marker (project) |
> |---|---|---|
> | copilot | `.github/skills` | `.github/copilot-instructions.md`, `.github/instructions/`, `.vscode/mcp.json` |
> | codex | `.agents/skills` | `.codex` |
> | gemini | `.agents/skills` | `.gemini` |
> | zed | `.agents/skills` | `.zed` |
> | amp | `.agents/skills` | `.amp` |
>
> So a healthy `grim lock && grim install && grim status` on a bare workspace
> reported five phantom orphans — `clients_extra: ["amp","codex","copilot",
> "gemini","zed"]` on a row installed seconds earlier — with no non-destructive
> way to clear them, on the default onboarding path, and through the MCP surface
> too (`src/mcp/server.rs:98` serializes the same report).
>
> **D4 does not rescue it.** Removing the `ClientTarget::ALL` fallback kills that
> repro, but a second one survives it and so does every explicit `--client`
> selection of a pool vendor.
>
> The oracle is also blind in the case the feature exists for: with detection
> empty the desired set is everything, so `recorded − active` is ∅ — uninstall
> *some* clients and it reports, uninstall *all* and it is silent. And `claude`
> and `opencode` `detect` by reading files grim itself writes
> (`<ws>/.mcp.json`, `$HOME/.claude.json`, `<ws>/opencode.json`), so grim's own
> leftovers keep those clients detected and their genuine orphans never flag.
>
> Where the set difference *is* sound it is near-vacuous: `claude`, `cursor`,
> `opencode`, `kiro`, `junie` root their output inside their own marker dir
> (`copilot` too, at global scope), so deleting the marker deletes grim's files
> and there is no orphan left to report.
>
> **Consequence for reaping: the earlier "riskier" framing was too kind.**
> Reaping would be driven by the same unsound oracle, so those five phantom
> orphans become a phantom *deletion* set — `grim update --force` on a healthy
> workspace would delete live, correct output for five vendors. Not adopted,
> and now on stronger grounds than D5 alone.
>
> **The hole remains open.** Closing it needs a **path-level** probe — "this
> recorded output exists on disk and no surviving client wants it" — respecting
> pool sharing, since one `.agents/skills` tree backs four vendors and
> `prune.rs::shared_by_surviving_sibling` already encodes that refcount.
> Reporting-only stays the right posture; the oracle is what has to change.
> Preconditions for any future attempt are recorded in
> `.claude/state/handover/wc-client-detection-task2.md`.

**Correction to D5 as stated in the meta-plan.** "`install` prints the resolved
set once" is under-specified: the default tracing filter is `warn`
(`src/main.rs:278`), so an `info`-level line is invisible by default. The
supported surface for "which clients will grim write to" is **`grim context`**,
which already reports the effective client set. An `info`-level line on install
is worth adding for `--log-level info` users; a new always-present `clients`
field on the install report is the alternative, but it touches
`src/api/install_report.rs`, which no wave-2 worktree owns — leave it as a
follow-up, not a requirement.

### Sub-decision A7 — `kept_modified` on a `shared_skills` flip {#a7}

**The interaction.** `reap_moved_outputs` guard 4
(`src/install/installer.rs:993-999`) preserves an orphaned old output whose
on-disk hash no longer matches the record — the user edited it. There is no
`--force` override, and `docs/src/stability.md:139-140` states that as a
promise. `reap_dropped_clients` applies the same rule at
`src/install/prune.rs:531-540` (there `--force` *does* delete).

So: flip `shared_skills` on a workspace holding a hand-edited skill, and grim
preserves the old native file **and** writes the new pooled one. Because
scanning is additive — the same fact that makes D1 correct — the client then
discovers the same skill name twice, with different content, and which one wins
is client-defined and unpredictable. Worse, the preserved file drops out of the
record (`outputs` is replaced by the new set), so it is invisible to `status`
from that moment on and the warning is one-shot.

**The trigger is `grim update`, not `grim install`.** A hand-edited recorded
output trips the integrity gate first: `installer.rs:877-896` walks every
recorded output and returns `InstallOutcome::Refused` when any hash drifted and
`!force`. The duplicate is therefore reachable only via `grim update`, which
passes `force = true` unconditionally (`update.rs:141`, rationale at
`:118-121`), or `install --force`. Any test of this behavior must use that
path.

**Option A7-a — warn, never delete.** Emit a `warn`-level message at flip time
naming both absolute paths and the client that will now see both, and leave
both files in place.

| Pros | Cons |
|---|---|
| `stability.md:139-140` stays true verbatim; D3's legitimacy rests on that section | The duplicate is real until the user acts |
| No path by which grim deletes a hand-edited file | The warning is one-shot; the preserved file is untracked afterwards |
| One `rm` recovers; data loss does not | `status` cannot see the duplicate later |

**Option A7-b — reap with `--force`.** Extend the layout-migration reaper with
a `--force` override so `grim update --force` deletes the modified old copy,
matching what `reap_dropped_clients` already does.

| Pros | Cons |
|---|---|
| One command resolves the duplicate | Contradicts a documented, user-protective promise in the very section that makes "a layout move is not a compat break" legitimate |
| Consistent with the dropped-client reaper | `--force` is already used for unrelated reasons (re-materializing tracked members); a user typing it for one of those silently loses a hand-edited skill in an unrelated client directory |
| — | Widening deletion is the one direction that is not recoverable |

**Recommendation: A7-a (warn), and do not add a `--force` deletion path.**

The reasoning is asymmetry of harm. The duplicate requires two deliberate user
actions to occur (an opt-in config flip *and* a local edit), it is visible —
both files exist, same name — and it costs one `rm` to resolve. Deleting a
hand-edited file costs the user work that cannot be recovered. Buying
convenience on the recoverable side by weakening the sentence that makes D3
defensible is a bad trade at 1.0.

A7-a's genuine weakness — the warning is one-shot and the preserved file leaves
the record — is real and should not be papered over. It is fixable later,
additively and cheaply: `status` can stat the *other* skills root for each
(artifact, pool-capable client) pair and report a stale sibling. That touches
`src/api/status_report.rs` and `src/command/status.rs`, which W-E does not own,
so it is recorded here as a **named follow-up**, not W-E scope.

**Minimum bar for W-E either way:** the warning is mandatory and must name both
absolute paths and the client; silence is not acceptable. An acceptance test in
`test/tests/test_shared_skills.py` must assert both files exist and the warning
fires.

### Sub-decision A8 — The refcount boundary is a documented limitation {#a8}

`shared_by_surviving_sibling` (`src/install/prune.rs:649-685`) decides whether
a shared-pool directory is still referenced by inspecting **only the record's
own `outputs`**. There is no filesystem fallback and no cross-record scan. Its
contract is precise about what the caller must pass (the whole record's outputs
plus the complete drop set, `prune.rs:620-634`) and the regression test at
`prune.rs:1547-1585` locks the relocated-ancestor case.

The boundary: a **tampered or hand-edited `state.json`** that omits a sibling's
output lets grim delete a pool directory another client still scans. This is
accepted as a documented limitation, not a defect to engineer against — the
state file is grim-owned and inside the user's own trust boundary, and a
filesystem-wide refcount scan would add a cross-record dependency to a hot
delete path. **Document it in W-H (`docs/src/stability.md` known limitations);
do not build for it.**

### Consequences

**Positive:**
- Zero behavior change for every existing install (D1, D2 default off).
- The user who wants one copy on disk can have it, explicitly and per vendor.
- The client-selection fallback stops writing itself into permanent state (D4).
- Autodetect users gain drift visibility they have never had (D5).
- Every mechanism D3 relies on is already shipped and tested.

**Negative:**
- Two render layouts per pool-capable vendor to support and document.
- A permanent new config surface (`[options.vendors]`) that can never be removed.
- A permanent client name (`agents`, D4) that is not a real product — it will
  appear in every client enumeration and every compatibility matrix forever.
- A bare-checkout `grim install` writes `.agents/skills` where it previously
  wrote eleven vendor directories: still a behavior change, just a much smaller
  one, and exit 0 is preserved for the common case.
- The A7 duplicate is a real, if narrow, user-visible wart until the follow-up
  lands.

**Risks:**
- *A pool-capable vendor with a non-empty `skill_fields()` registry corrupts a
  sibling's recorded hash.* Two paths, both verified: cross-pass, the flipping
  client skips the client-keyed clobber gate (`installer.rs:528-533`) and
  `remove_path`s a live sibling directory (`:659-661`); same-pass,
  `installer.rs:650` dedups by destination *before* rendering, so the second
  vendor silently inherits the first's bytes and hash and its own fields never
  render. Mitigation: the disqualification rule in [D2](#d2), stated as a
  requirement in the W-E contract — not a guard in the installer.
- *The generic client's pool write is mistaken for a detection signal by a
  future vendor.* Mitigation: `.agents/skills` is already established as a weak
  marker nothing detects on (`vendor_gemini.rs:100-105`); any new vendor must
  follow that precedent, which W-G's brief already requires.
- *The residual 78 (D4) surprises a script whose artifact set is rules-only.*
  Mitigation: the message names `--client`; the case is narrow and previously
  produced ten inert directories instead.

## Builder Contracts

Three downstream worktrees name this ADR as their spec. These paragraphs are
the contract; where they disagree with a sub-plan, this ADR wins.

### W-B — `feat/per-vendor-config` (config surface only)

Add `pub vendors: BTreeMap<String, VendorOptions>` to `ConfigOptions`
(`src/config/declaration.rs`) with `#[serde(default, skip_serializing_if =
"BTreeMap::is_empty")]`, and `VendorOptions { pub shared_skills: bool }`
carrying `#[serde(deny_unknown_fields)]` and exactly that one field — no
speculative `root`. Register the dotted key
`options.vendors.<name>.shared_skills` in `src/command/config_keys.rs` so
`grim config get|set|unset|list` addresses it — but **not as a `ConfigKey`
arm**: `ConfigKey::ALL` is a fixed `[ConfigKey; 7]` of *static* keys
(`src/command/config_keys.rs:65`) whose completeness is asserted bidirectionally
by `config_options_completeness_matches_config_key_all` (`:343`), and a
per-vendor key is dynamic; follow the shipped dynamic-key pattern instead
(`ParsedKey::RegistryAlias`, `src/command/config.rs:209-211`). Write
`title`/`description` per `.claude/rules/subsystem-config-keys.md` (the
`description` must remain a whitespace-normalized **prefix** of the
`declaration.rs` doc comment, checked by
`config_key_metadata_matches_published_schema`). Validate vendor names against
`ClientTarget::ALL` — never a hand-maintained list — through **one shared
validator serving both paths**, exactly as `check_clients` /
`validate_clients` do (`src/config/project_config.rs:406-442`): set-time **65**,
load-time **78**, plus **64** for an unknown key through `grim config set`
(`src/command/config.rs:214`). Leave the two `options.clients`
"falling back to all clients" strings (`declaration.rs:181-186`,
`config_keys.rs:96-98`) untouched — D4 makes them W-C's this wave.
`ConfigOptions::resolved` (`src/config/resolved.rs:71-76`)
destructures exhaustively and will fail to compile until `vendors` is bound —
bind it `: _` like `clients`, because an empty table is meaningful, not an unset
value awaiting a default. Ship **no consumer**: `src/install/**` is forbidden in
this worktree. Report to W-E the exact registered key string and the precedence
sentence verbatim.

### W-C — `fix/client-detection` (selection semantics)

**Selection semantics only; split across two waves.** The generic client's
vendor surface is W-A's (task A6) — do not re-create it. **Task 2 runs in wave
1** (no W-A dependency); **task 1 runs in wave 2**, alongside W-D on a disjoint
file set, once `ClientTarget::Agents` exists. Task 1: strip the
`ClientTarget::ALL` fallback from `detect_clients`
(`src/install/target.rs:127-131`) so it returns the raw detected set, add an
`_or_all` wrapper for the read-only consumers (`status.rs:149`,
`search.rs:252,276`, the six `src/tui/app.rs` badge sites) so their behavior is
byte-identical to today, and in `InstallTarget::parse` (`target.rs:64-93`)
substitute `ClientTarget::Agents` when flag, config, and detection are all
empty. Raise `ExitCode::ConfigError` = **78** only when the fallback is active
*and* the artifact set contains nothing installable, with a message naming
`--client` and the client list derived from `ClientTarget::VALUE_NAMES`.
`InstallTarget::new` stays infallible. Two carve-outs are mandatory: `grim tui` surfaces the residual
78 in-app at `src/tui/app.rs:2339,2472,2564` rather than exiting, and
`src/command/context.rs:42` reports the **resolved** set (`["agents"]` under the
fallback). `grim init` seeds `[options].clients` from detection, writing nothing
when detection is empty. Task 2: make autodetect drift **visible** — a recorded
output for an undetected client surfaces as `clients_extra` in `status`; do
**not** enable reaping under autodetect, and do not touch the
`update.rs:182-192` gate's raw-config read. Keep `prune.rs:1547-1585` green and
update the `[]`-under-autodetect claim at `docs/src/stability.md:52`/`:61` and
`docs/src/commands.md:445-446` in the same commit. **Forbidden — all W-A's:**
`vendor_agents.rs`, `client_target.rs`, `path_anchor.rs`, `render.rs`,
`docs/src/clients.md`.

### W-E — `feat/shared-skills-opt-in` (the first consumer)

Add `Vendor::pool_capable() -> bool`, defaulting to **false**, populated only
from verified evidence (verified readers as of 2026-07-26: Codex, Gemini, Zed,
Amp, Cursor, GitHub Copilot, OpenCode; verified non-reader: Claude Code;
unevidenced ⇒ false: Kiro, Junie). Absent evidence defaults to not-capable — a
later flip to capable is additive, the reverse is breaking. `shared_skills =
true` on a non-capable vendor exits **65** naming the vendor and why. A second
rule is a **requirement, not a recommendation**: a vendor with a non-empty
`skill_fields()` registry is not `pool_capable()` at all. The consequence of
weakening it is silent corruption — the untracked-clobber gate keys on client,
not path (`installer.rs:528-533`), so a flipping client skips it and
`installer.rs:659-661` deletes and rewrites a sibling's live pool directory;
that is safe only while every pool vendor emits byte-identical bytes
(`render.rs:879-919`). When enabled, the vendor's `skills_root` resolves to the
pool. **Do not edit `src/install/render.rs`** — W-D generalizes the hard-coded
`assert_eq!(checked, 4)` (`render.rs:874`) to derive from `ClientTarget::ALL` in
wave 2, because W-E, W-F, and W-G each break it and W-F is concurrent; W-E
inherits the generalized invariant. Migration reuses `output_at_current_layout`
(`installer.rs:942-957`) and `reap_moved_outputs` (`installer.rs:978-1042`) —
invent nothing; both flip directions are already covered (see [D3](#d3)), and
the one uncovered case is the occupied-pool-path write that the capability rule
above mitigates. Add the missing test for it. Implement A7-a: warn, never delete
a hand-edited file, no `--force` override on the layout-migration reaper; the
warning names both absolute paths and the client, and an acceptance test in
`test/tests/test_shared_skills.py` asserts both files exist and the warning
fires — **written against `grim update` or `install --force`, never plain
`install`**, which returns `Refused` at the integrity gate first
(`installer.rs:877-896`). Do not touch `src/install/path_anchor.rs`.

## Implementation Plan

1. [x] **D4 resolved** 2026-07-26 — generic fallback client, not an error.
2. [ ] W-A: env-resolution fixes **+ the generic `agents` vendor** (Wave 1) —
   the critical path.
3. [ ] W-B: config surface, no consumer (Wave 1).
4. [ ] W-C: task 2 in Wave 1; task 1 in Wave 2 after W-A merges.
4. [ ] W-E: `pool_capable`, gate, render switch, migration, A7-a (Wave 3, after
   W-B and W-D).
5. [ ] W-H: document the A8 refcount limitation and the two render layouts
   (Wave 5).
6. [ ] Follow-up (unscheduled): stale-sibling-root probe in `status` to make an
   A7-preserved file durably visible.

## Validation

- [ ] Old `grimoire.toml` without `[options.vendors]` parses unchanged
      (additive guarantee, D2).
- [ ] Round trip: parse → serialize → parse leaves an empty table absent.
- [ ] Exit codes asserted at all three D2 surfaces (64 / 78 / 65).
- [ ] Flip off→on and on→off both migrate, re-anchor, and round-trip through
      `status` / `uninstall`, mirroring `test/tests/test_global.py:308-360`.
- [ ] Two vendors both opted in ⇒ one file, two `ClientOutput` records with
      equal targets; dropping one keeps the file
      (`shared_by_surviving_sibling`), dropping both removes it.
- [ ] Hand-edited old path on flip ⇒ both files present, warning emitted (A7-a).
- [ ] D4: bare workspace + a skill ⇒ exit 0, output under `.agents/skills`,
      `outputs[].client == "agents"`; bare workspace + only rules/agents/MCP ⇒
      **78** naming `--client`; two consecutive bare-workspace runs both resolve
      to `agents` alone (the pool must not manufacture a detection signal —
      this is the regression test for the original defect); `grim context`
      reports `["agents"]`; `status`, `search` unaffected in shape.
- [ ] `prune.rs:1547-1585` stays green.

## Links

- [`adr_vendor_wave_expansion.md`](./adr_vendor_wave_expansion.md) — shared-pool
  refcount semantics (§3) this ADR bounds in [A8](#a8)
- [`adr_render_layout_stability.md`](./adr_render_layout_stability.md) — the
  layout-move contract [D3](#d3) reuses
- [`adr_codex_vendor.md`](./adr_codex_vendor.md) — the honest-decline
  philosophy [D2](#d2)'s capability gate extends
- [`adr_install_state_portability.md`](./adr_install_state_portability.md) —
  `PathAnchor` / `outputs` model every migration claim depends on
- `docs/src/stability.md` — §Unstable (render layout), §The compatibility
  promise (reaper preservation rule)

---

## Changelog

| Date | Author | Change |
|------|--------|--------|
| 2026-07-26 | architect | Initial draft — D1–D5, sub-decisions A7/A8, builder contracts for W-B/W-C/W-E |
| 2026-07-26 | architect | **Ownership ruling.** The generic client splits at the vendor/selection seam: W-A ships `vendor_agents.rs`, the enum arm, the anchor row, the docs row, and the `render.rs:874` generalization (it is the first worktree to break that counter); W-C keeps selection only and splits across waves 1 and 2. W-D's `render.rs` carve-out struck — it inherits |
| 2026-07-26 | architect | **Owner decisions.** Status → Accepted. **D4 replaced**: zero detected clients now fall back to a synthetic skills-only `agents` client writing the shared pool, with exit 78 surviving only when nothing in the set is installable; rationale, the non-poisoning argument (`vendor_gemini.rs:100-105`), and the four out-of-ownership files recorded. D1 gains the owner principle "prefer vendor-specific directories over the pool". Consequences and Risks rewritten around the new D4 and the second (same-pass, `installer.rs:650`) corruption path. Not changed: D1, D2, D3, D5, A7, A8 |
| 2026-07-26 | architect | Factual corrections from the sub-plan audit (no decision changed): D2 exit codes now record the set-time/load-time 65/78 split through one shared validator plus 64 for an unknown key; D3's "no shipped code covers N→1" retracted — both flip directions are covered, the residual gap is the occupied-pool-path write; D5 cites `stability.md:52,:61` (not `:145-147`); A7 records that the duplicate is reachable only via `grim update` / `install --force`; W-B contract switched to the `ParsedKey::RegistryAlias` dynamic-key pattern; W-E contract promotes the `skill_fields()` rule to a requirement and hands `render.rs` to W-D |
