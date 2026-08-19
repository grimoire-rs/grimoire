# Research: Hook distribution via a `grim hook` trampoline

## Metadata

**Date:** 2026-08-14
**Domain:** packaging | integration | security
**Triggered by:** maintainer feature request — "support hooks", with a proposed
design: render each client's *native* hook registration, but point it at a
`grim hook` subcommand that normalizes the vendor payload into one generic
interface and then invokes the user's real command (shell / python / npm / …).
**Expires:** 2027-02-14 (vendor hook surfaces move fast — re-verify with
[`research_hooks_vendor_survey.md`](./research_hooks_vendor_survey.md))
**Companion artifact:** [`research_hooks_vendor_survey.md`](./research_hooks_vendor_survey.md)
— the 17-client survey. This file is the *grim-side* analysis.

## Direct Answer

The trampoline is the right shape, and it is a strictly better design than the
one already recorded in [`adr_hooks_support.md`](../adr/adr_hooks_support.md)
(Proposed, 2026-06-03). That ADR translates the *registration* per client but
leaves each hook script facing N payload dialects and N response schemas. The
trampoline moves normalization from "every hook author, N times" to "grim,
once" — and it buys four things the ADR cannot:

1. **One managed config entry per (client, event)** instead of one per hook —
   which collapses the hardest engineering problem (owning an element inside a
   user's config array, idempotently and reversibly) into a shape grim
   *already ships* for OpenCode's `instructions` glob.
2. **grim owns matching, ordering and decision aggregation** — so per-vendor
   matcher dialects (JS regex vs glob vs category-events) stop being a
   translation problem.
3. **A policy enforcement point.** Because every hook runs through grim, an
   unapproved or untrusted hook can be refused *at fire time*, not just at
   install time. A config entry alone can never be revoked; a trampolined one
   can.
4. **It sidesteps the executable-bit defect** recorded below — grim launches
   the payload itself, so the payload never needs to be an executable file.

The dominant open risk is the mirror image of (3): the trampoline makes grim a
**runtime dependency of the agent loop**, on the hot path of every tool call,
in a process the user did not launch. That risk is manageable but must be
designed for explicitly (see "Fail-open or fail-closed" below) — it is the one
thing that can turn "hooks stopped working" into "my agent refuses to run any
command".

## Findings in the current codebase

Verified by reading source on 2026-08-14 at `03e59b0`. These are the facts a
hook kind has to build on.

### F1 — The executable bit does **not** survive the registry round trip

| Site | Behaviour |
|---|---|
| `src/skill/skill_package.rs:439` | packer hardcodes `header.set_mode(0o644)` for every entry |
| `src/install/materializer.rs` (`DefaultMaterializer`) | unpacks with `std::fs::write` — tar modes are never applied |
| `src/install/client_target.rs:565-588` (`atomic_copy`) | *does* preserve source permissions, deliberately, with a test |

So the preserved-exec-bit test
(`materialize_skill_preserves_exec_bits_and_leaves_no_temp_files`) passes
because it materializes from a **local directory** whose script was `0o755`.
An artifact that travelled through an OCI registry arrives `0o644` at the
store, and `atomic_copy` then faithfully preserves `0o644`. **A distributed
script is not executable.** `adr_hooks_support.md` § Context item 1 is
therefore correct, and its consequence is larger than stated: the fix is not
just "chmod in the materializer", it is "the packer erases the bit first".

**Why the trampoline matters here:** if grim invokes the payload (`sh -c …`,
`python3 …`, or an argv array), no client ever execs the file, so no
permission semantics need to change in the store or the packer. The exec-bit
problem becomes optional rather than blocking.

### F2 — The "reversible foreign-config registration" engine already shipped

`adr_hooks_support.md` treats reversible config registration as the new,
unproven machinery to be built. It has since been built and shipped — for a
different consumer — and its own source comment names hooks as the origin of
the pattern (`src/install/opencode_config.rs:13`):

> "added when the first OpenCode rule installs, removed when the last one
> uninstalls (the reversible config-registration pattern from the hooks ADR)"

What exists today:

| Piece | Location | Relevance to hooks |
|---|---|---|
| `Vendor::sync_config(state, workspace, scope)` | `src/install/vendor.rs:377` | state-derived convergence seam, called after every install/update/uninstall; doc comment literally says "(hooks ADR pattern)" |
| `opencode_config::sync_for_state` | `src/install/opencode_config.rs` | working reference: one managed array element, added on first, removed on last |
| `json_splice::upsert_member` / `remove_member` | `src/install/json_splice.rs:57,131` | span-preserving object-member splice; **two-level pointer only** (`split_pointer` rejects deeper) |
| `json_splice::upsert_array_element` / `remove_array_element` | `:184,230` | **array element splice already exists** — but only for a *string* element at a *root* key |
| `toml_splice` | `src/install/toml_splice.rs` | the Codex/TOML equivalent, on `toml_edit` |
| `ClientOutput.entry: Option<String>` | `src/install/install_state.rs:84-94` | entry-typed output: semantic (not byte) integrity on the pointed value; uninstall removes the entry, never the file |
| `ClientOutput.adopted` | `:95-111` | "the user already had exactly this" → leave it alone on uninstall |
| prune refcount (`shared_by_surviving_sibling`) | `src/install/prune.rs` | N artifacts → 1 physical destination, released only by the last |

**Gap to close:** hook registrations are *objects nested two or three levels
deep inside arrays* (`hooks.PreToolUse[i].hooks[j]` for Claude), where today
the splice engine handles a string element at a root key or an object member
one level down. This is an extension of a proven engine, not a new one.

### F3 — The dispatcher entry makes the gap in F2 mostly disappear

If grim registers **one entry per (client, event)** whose command is the
trampoline, and fans out to N hooks internally, then:

- the managed unit is one array element per event, whose *identity* is its own
  command string — exactly `upsert_array_element`'s existing contract;
- installing hook #2 touches **no** vendor config at all;
- `sync_config` converges the whole set from install state, so uninstall/prune
  reversibility is the OpenCode pattern verbatim;
- the entry is registered with the **widest** matcher (match-all), so no
  per-vendor matcher dialect ever has to be translated — grim matches
  internally with one semantics.

Cost: grim is invoked on every event of a registered kind even when nothing
matches, so the no-match path must be genuinely cheap (see F5).

### F4 — Forward compatibility: emit hook fields only when non-empty

`GrimoireLock` and `InstallState` are `deny_unknown_fields`. The codebase is
explicit about what that costs (`install_state.rs:106-111`): an
*always-emitted* new field makes every new state file unreadable by an older
grim — "a breaking change, not an additive one". The precedent to copy is
`adr_agent_artifact_kind.md`: the declaration hash emits `"agents"` **only
when non-empty**, and the lock stays V1 with an optional section.

So: `[hooks]` in config, optional `[[hook]]` in the lock, hook fields in state
— all `#[serde(default)]` **and** skipped when empty. A user with no hooks
keeps byte-identical files, and an older grim can still read them.

Second-order trap: if hook *registrations* widen `ClientOutput.entry` to a
three-level pointer, an older grim reading that state file parses the field
fine (it is a `String`) but `split_pointer` returns `None` → it cannot
uninstall the entry. That is a behaviour break for old binaries, not a schema
break. Worth an explicit decision rather than a discovery.

### F5 — Latency budget: the trampoline is on the hot path

`pre-tool` fires on **every** tool call. The trampoline adds one process spawn
ahead of the payload's own.

**Corrected 2026-08-14 (axis `trampoline-hot-path-cost`, re-verified against
`src/context.rs:167-183` and `src/config/project_config.rs:140,149`).** This
finding originally said grim's startup "builds a `Context` (config walk-up,
lock parse, state parse) — far too much for this path". That conflated two
different things and the practical consequence is different:

- `Context::new()` is **already cheap** — pure struct construction from env
  reads plus `OnceLock::new()`, no I/O at all. Nothing needs fixing there.
- The real cost lives in **per-command scope resolution**: `walk_up_for_config`
  (`project_config.rs:149`, the only walk-up in the tree) plus the lock and
  install-state parses layered on it. `grim hook run` must simply never call
  that path.
- The shipped precedent is `grim schema` / `grim completions`
  (`app.rs:131,135`) — and it needs stating precisely, because a first reading
  overstates it: `Context::new` runs **unconditionally** for every command at
  `app.rs:27`, before the dispatch match, so **no command skips it**. That is
  harmless (it is the cheap, no-I/O constructor above). What those two commands
  actually do is never *receive* `&ctx` — their arms take `&args` only — so
  nothing in them can reach scope resolution. `grim hook run` should follow
  exactly that: a dispatch arm that does not take `&ctx`. It would be the
  **first** command whose hot path depends on that property rather than merely
  happening to have it, so the property needs a test, not a convention.

Requirement therefore stands but with a sharper target: read one **pre-compiled
dispatch file** regenerated wholesale by the existing `sync_config` convergence
pass on every install/update/uninstall (never patched incrementally), do an
in-memory exact-key lookup on `(client, event)`, exit — no lock, catalog,
registry, network, tokio runtime, or per-invocation regex compilation. Matching
must use precompiled exact/glob forms: regex *compilation* alone measures from
microseconds to ~44 ms for Unicode-heavy patterns, which would dwarf everything
else on the no-match path.

**Platform reality (do not generalize from one host).** Process-launch floors
diverge by roughly an order of magnitude Linux → macOS and more than twenty
times Linux → Windows (`CreateProcess`, and sensitive to Defender/AV). **WSL2 is
a third platform, not a Linux proxy** — simple `fork` measures ~2–5 ms there
versus ~30 µs native Linux. Specific to this repo: both checkouts
(`/mnt/wsl/share/dev/…` on `/dev/sdd` and `/home/mherwig/dev/…` on `/dev/sde`)
are genuine **ext4 block devices**, so the separate 9P cross-filesystem tax
(~6 ms/stat on `/mnt/c/…`) does **not** apply here and must not be conflated
with spawn cost — but every number measured on this host is still a WSL2
number, not a native-Linux one.

**No budget number is invented here, deliberately.** The comparable tools
(lefthook, starship, direnv, mise) publish none — they benchmark against what
they replace, not an absolute figure. The strongest prior art is `mise`
eliminating `asdf`'s shim indirection (~120 ms → ~5–10 ms per command) by
pushing resolution to install time: the same move this design makes. The
implementation task must measure **two** numbers (no-match fast path vs
match-and-dispatch), report **p50 and p99** rather than a mean (spawn cost is
bimodal and AV-sensitive on Windows), state cold vs warm cache explicitly, and
carry WSL2-native as its own row. Tooling: `hyperfine` (`--warmup`, and
`-N`/`--shell=none` for sub-5 ms commands) plus `strace -c` / `perf stat` to
attribute cost rather than trust an estimate; the Windows-equivalent
methodology is `NOT DOCUMENTED` in anything surveyed and needs its own
verification rather than an assumed transfer.

A resident daemon stays the wrong first answer (new lifecycle + new security
surface, and YAGNI), and the precedent is cautionary: `eslint_d`'s 700 ms →
160 ms win is entirely about eliminating **Node** startup, which does not
transfer to a compiled Rust binary, and `pnpm` shipped a daemon
(`pnpm server`) and then removed it in pnpm 11. Daemon remains the escape
hatch if measurement demands one.

### F6 — `grim hook run` is already covered by an existing CLI exemption

`subsystem-cli-api.md` § "Commands That Exec a Child Process": a command whose
job is to replace/spawn a child process is exempt from the `Printable` /
report-module path. `grim mcp` and `grim tui` are the existing
`Printable`-exempt precedents. So the trampoline subcommand needs no report
module — it speaks the vendor's stdout protocol instead. Convention already
exists; nothing new to invent.

### F7 — The maintainer's own later ADR already settles "kind, not metadata field"

`adr_structured_vendor_metadata.md` (Proposed, 2026-07-17) adds
`FieldType::Json` for object-valued vendor metadata and explicitly refuses to
put `hooks` on its allowlist:

> "`hooks` and `mcpServers`/`mcp-servers` already have better-fitting homes (a
> dedicated artifact kind and the MCP artifact kind, respectively)"

Same statement in three shipped places: `vendor_claude.rs:28-29`,
`docs/src/vendor-metadata.md:186`, `docs/src/agents.md:196-199`. So "hooks =
dedicated `ArtifactKind`" is settled context, not an open question.

### F8 — Claude also exposes hooks *inside* agent frontmatter

`docs/src/vendor-metadata.md:228` records `hooks` as an object-valued **agent**
frontmatter field grim cannot currently project, with the documented
workaround "edit the installed file". That is a *second* registration surface
(agent-scoped hooks — active only while that subagent runs) and a genuinely
different capability from settings-level hooks. Out of scope for v1; purely
additive later, and worth not designing it out.

### F9 — Blast radius of a new kind

**Superseded by F10 below — the "~12 sites" figure is folklore.** Real cost:
~25–30 exhaustive `match` arms across ~20 files, **plus** ~20 struct-literal
sites no enum count reveals, **plus** 17 vendor files that are *not*
compiler-forced.

| Surface | Change |
|---|---|
| `ArtifactKind` | new variant + `subdir()` → `"hooks"`; see F10 for the real edit set |
| `docs/src/clients.md` | new column in the compat matrix — **parity-tested**, a test reads the table |
| `catalog/` | CLI + docs change ⇒ mandatory drift review of `grim-usage` / `grim-authoring` / `ai-config-authoring`; `task catalog:verify` gates it |
| `grim schema --kind` | a hook descriptor schema joins `config`/`publish`/`lock`/`mcp` |
| `KindSupport` | per-*kind* today; hooks need per-*hook* capability resolution (see D3) |

### F10 — The real edit set for a new `ArtifactKind` (Discover, 2026-08-14)

From the `architecture-explorer` pass, spot-verified by the orchestrator. The
prior ADR's "~12 sites, compiler-guided" is wrong in both the count and the
*kind* of cost:

| Cost | Size | Compiler-forced? |
|---|---|---|
| Exhaustive `match kind { … }` arms with no wildcard | **~25–30** across ~20 production files — `artifact_kind.rs` itself (`subdir`/`artifact_type`/`config_media_type`/`is_dir_artifact`/`Display`), `client_target.rs`, `command/{remove,add,install,update,publish,build}.rs`, `resolve/resolver.rs`, `mcp/render.rs`, `fetch.rs`, `installer.rs`, `tui/app.rs` (×5), `skill/local_pack.rs`, `lock/effective_set.rs` | yes |
| **`GrimoireLock` and `DesiredSet` carry one field per kind, not a map** | **13** `GrimoireLock { … }` + **7** `DesiredSet { … }` explicit struct-literal sites | yes — but **invisible to any enum-match count**, which is why the prior estimate missed it |
| `path_anchor.rs::candidate_anchors` — its own per-`(client, kind)` match | likely the single largest file to touch | yes |
| **`Vendor::kind_support` — one arm per vendor file** | **17** `vendor_*.rs` files | **NO** — see the trap below |

### F11 — The `kind_support` default is backwards for hooks (correctness trap)

`Vendor::kind_support` defaults to `KindSupport::Native` (`vendor.rs:196-198`).
So the moment `ArtifactKind::Hook` exists, **all 17 vendors silently claim
native hook support** — including Warp and Zed, which have no hook surface at
all. Nothing fails to compile; a missed vendor file is a silent false claim,
not an error.

For a v1 shipping **3** clients that is exactly inverted: 14 of 17 must be
`Declined`. Two ways out, and the ADR must pick one explicitly:

- **14 explicit `Declined` arms** plus a test pinning the exact supporting set
  (the `SCOPE_GAPS` / `POOL_CAPABLE_VENDORS` pattern already used in
  `vendor.rs`'s tests for precisely this class of silent-drift risk); or
- **invert the default for this kind** — a hook-specific predicate defaulting
  to `Declined`, so support is opt-in per vendor and a forgotten vendor fails
  safe.

The second is the safer shape and has no precedent in the trait yet; the first
matches existing convention. Either way the *test* is the load-bearing part.

### F12 — Splice-engine gap is wider than F2 stated

Re-verified: `json_splice::upsert_array_element` / `remove_array_element`
operate on **string elements at a root key only** — they call
`json_string(element)` and compare by value equality. **`toml_splice` has no
array-element function at all.**

Consequences for the two splice-shaped v1 targets:

- **Claude** (`hooks.PreToolUse[].hooks[]` — objects nested two levels inside
  arrays) needs **genuinely new splice code**, not a parameter change.
- **Codex**, if registered by splicing `[hooks]` in `config.toml`, would need a
  brand-new **TOML array-of-objects** primitive as well. That is a second new
  engine for one client — and it **strengthens the case against surface 2 in
  D7a**, independently of the trust-identity argument.

### F13 — Two contract surfaces need a stated semantic, not a column

- **`[options.experimental]` does not exist anywhere today.** D10's table is
  brand new, not an extension of a shipped one; nearest precedents are
  `VendorOptions` / `TuiOptions` (`config/declaration.rs:111-176`).
- **The `docs/src/clients.md` parity test hardcodes exactly four kind
  columns** (`client_target.rs:749-786`): it iterates `[Skill, Rule, Agent]` by
  index and special-cases `cells[3]` as a *boolean* MCP column derived from
  `mcp_config_path().is_some()`. A `Hook` column therefore cannot be added
  mechanically — one `kind_support(Hook)` verdict per client cannot express
  per-hook-**tier** fidelity (a client may host an `observer` natively and be
  unable to host a `mutator` at all). **The ADR must define what the Hook cell
  means** — the recommendation is *the best tier the client can host*
  (`✓` = mutator-capable, `◐` = observer/gatekeeper only, `✗` = no surface) —
  and the test grows a fifth column against that definition.

## Design analysis

### The two-tier authoring model already exists — reuse it verbatim

`src/install/vendor.rs:16-19` states the owner decision: a capability common
to several vendors is authored **once as a canonical top-level field** and
projected per vendor; a capability unique to one vendor is authored as
`<vendor>.<field>` in `metadata`. Hooks should follow this exactly rather than
invent a taxonomy:

- `event:` — a **small** canonical event, only for concepts that are
  semantically identical in 4+ clients (candidates: `session-start`,
  `session-end`, `prompt-submit`, `pre-tool`, `post-tool`, `stop`,
  `pre-compact`).
- `<vendor>.event` — a native event name, installing for that vendor only
  (`cursor.beforeShellExecution`, `claude.PreCompact`, …).

Resisting a 30-event canonical superset is the main discipline here. A
canonical event that exists in one client is not portable — it is a native
event wearing a portable costume, and it silently lies to hook authors.

### Payload channel: stdin envelope, not env-var JSON

The request floated "a JSON file passed via env var, or env vars with
structured data, plus the real vendor payload". Recommendation: **stdin
carries one canonical envelope that itself contains the raw vendor payload**;
env vars carry only a few flat scalars.

Why not the full payload in env:

- **Size.** `post-tool` payloads can embed whole file diffs or command output.
  The environment block is bounded (`ARG_MAX`-adjacent on Linux; Windows caps
  a single variable at ~32 KiB). This overflows in practice, and grim should
  not be in the business of capping hook payloads.
- **Exposure.** `/proc/<pid>/environ` is readable by the same user, env is
  inherited by every grandchild, and it lands in crash dumps and CI logs. Tool
  input can contain secrets.
- **The temp-file variant is worse than both.** A path in an env var adds
  cleanup, a TOCTOU window, and world-readable-tmp risk, and buys nothing over
  stdin — except in the one real case where the payload must be read twice or
  the payload program cannot read stdin. Support it as an opt-in
  (`payload = "file"`), never as the default.

Env vars worth setting (cheap for shell one-liners, no size risk):
`GRIM_HOOK_EVENT`, `GRIM_HOOK_CLIENT`, `GRIM_HOOK_NAME`, `GRIM_HOOK_TOOL`,
`GRIM_HOOK_CWD`, `GRIM_HOOK_TIER`, `GRIM_HOOK_SCHEMA` (envelope version), and
`GRIM_HOOK_DIR` — the hook artifact's own install directory, so a payload can
reach its bundled files. Claude's `$CLAUDE_PROJECT_DIR` is the precedent for
why that last one is not optional.

### The response contract is harder than the payload

Payloads are informational, so a superset is easy — extra keys are ignorable.
Responses are semantic and lossy in *both* directions, and one divergence is
sharp enough to be a design driver on its own:

> Claude fails **open** on an unexpected non-zero exit; Copilot's `preToolUse`
> fails **closed**. The same unchanged hook, on two clients, has opposite
> behaviour when it crashes.

That is precisely what a trampoline can fix and a translation layer cannot:
grim decides the failure policy and emits the vendor response that
*implements* it, instead of letting each vendor's default leak through.

Keep the canonical response **small and closed** — `decision`
(`allow|deny|ask|none`), `reason`, `context`, `user_message`, `stop` — and
project it. Anything richer is native passthrough for one declared vendor.

### Capability tiers × per-vendor capability = Native / Degraded / Declined

`KindSupport` (`Native | Degraded | Declined`) is already the blessed
vocabulary for "this vendor cannot fully express this". Hooks need the same
tri-state resolved **per hook**, not per kind, because what a hook needs
varies:

| Hook tier | Needs | On a client that cannot | Verdict |
|---|---|---|---|
| `observer` | fire-and-forget, ignore output | — | Native almost everywhere |
| `gatekeeper` | can deny / block | client has no blocking response | **Declined** + warn (installing it would silently not protect) |
| `mutator` | rewrites tool input | client cannot return modified input | **Declined**; and arguably out of v1 entirely |

`observer` is where nearly all of the near-term value lives (audit logs,
formatters, notifiers, metrics) and where the security story is easiest. A
`gatekeeper` that silently degrades to an observer is a *security* defect, not
a fidelity one — it must decline, not degrade.

### Fail-open or fail-closed — the risk that needs an owner's decision

The trampoline makes grim load-bearing at agent runtime. Failure modes:

- grim upgraded, moved, or uninstalled → registration points at nothing;
- version skew: config written by a newer grim, invoked by an older one;
- a grim panic becomes an agent-visible event — and on a fail-closed client,
  a *blocked tool call*.

Recommended posture: **the trampoline fails open by default** (any internal
error → allow + log), and only a hook that explicitly declares fail-closed,
at a tier that permits blocking, gets the other behaviour.

Cheap, concrete mitigation for the missing-binary case: register a **guarded
one-liner** rather than a bare command, e.g.
`sh -c 'command -v grim >/dev/null 2>&1 || exit 0; exec grim hook run …'`.
No extra file, no exec bit, and "grim is gone" degrades to "hooks silently
off" instead of "agent broken". (Windows/PowerShell needs its own spelling;
several vendors already carry a `powershell` variant field for this.)

Open question worth deciding rather than discovering: does the registration
reference `grim` **on `$PATH`** (breaks under a minimal agent env), an
**absolute path** to the running binary (breaks on upgrade/move, and grim's own
state is anchor-relative precisely to avoid baking absolute paths), or a
**generated launcher** under the vendor dir (an extra file, but grim generates
it locally so it can `chmod` freely and self-heal on re-install)?

### Security: this changes grim's risk class

Today grim ships **text an agent reads**. A hook ships **code that executes
automatically**, hundreds of times per session, at user privilege, delivered
by a registry. Consequences worth stating plainly:

- `grim install` currently arms nothing. It must not silently arm code
  execution. The strongest model in the field is Codex's: hash/digest-pinned
  approval per hook, re-prompted on change. `grim mcp --allow-writes` is the
  in-repo precedent for gating a dangerous capability at the invocation
  surface.
- CI needs a non-interactive path (`--allow-hooks` / `GRIM_ALLOW_HOOKS=1` /
  an approved-digest list in config).
- **Bundles can smuggle a hook.** A bundle expands at resolve time; a hook
  member must be visible at `add` time and in the approval prompt, never
  arrive as a side effect of `grim add some-bundle`.
- **Global scope is the dangerous one**, inverting the usual intuition: a
  global hook arms every project the user ever opens. Project-scope hooks from
  a cloned repo are the RCE-on-clone case that vendors answer with workspace
  trust.
- A `pre-tool` hook that can rewrite tool input is a prompt-injection
  amplifier (`cargo build` → `curl … | sh`). That is the `mutator` tier, and
  the safest v1 answer is "not yet".

### Sequencing: two clients, not one

`adr_hooks_support.md` sequences Claude-first, then generalize. With a
trampoline the better first cut is **the envelope + trampoline against two
clients simultaneously** (Claude plus one of Cursor / Codex). A portability
layer validated against a single vendor is not validated at all — and the
second vendor is cheap once the trampoline exists: an event-name map plus a
registration writer.

## Decisions (owner, 2026-08-14)

| # | Decision | Outcome |
|---|---|---|
| D1 | Dispatcher entry per (client, event) vs one entry per hook | **Dispatcher** — collapses the array-identity problem (no vendor but Antigravity has an identity field), gives grim ordering + aggregation, reuses `sync_for_state` |
| D2 | Payload channel | **stdin envelope** containing `raw`; env vars for flat scalars only; `payload = "file"` as opt-in |
| D3 | Canonical events only, or canonical + native passthrough | **Both.** Canonical = **Claude's schema and PascalCase names**, which the survey shows is the de facto standard, not a neutral invention. `<vendor>.event` for native-only events |
| D4 | v1 capability scope | **`observer` + `gatekeeper` + `mutator`** — owner decision, taken with the injection risk stated. Mutator ships with the mandatory controls in "Mutator tier" below |
| D5 | Security gate | **Per-hook digest-pinned approval** (Gemini's validated model), re-prompt on digest change, CI escape via `--allow-hooks` / `GRIM_ALLOW_HOOKS=1` / an approved-digest list in config |
| D6 | How the registration names the binary | **Resolved by the trust findings: a fixed-path, grim-owned launcher** (e.g. `$GRIM_HOME/bin/grim-hook`). See "The command string is a trust key" below — two clients hash the command string, so the string must be **byte-stable across grim upgrades**, which rules out both a versioned absolute path and anything that moves. A fixed launcher path is stable *and* absolute (so a minimal agent `$PATH` cannot break it) *and* grim-owned (so it can fail open when grim is absent, which exec-form argv cannot) |
| D7 | v1 client set | **claude + codex + copilot**, no codegen client. Copilot's **cloud-agent** surface is explicitly out of scope (no grim binary in the ephemeral sandbox). **The design must forward-accommodate the other 12** — see "Forward design" below |
| D7a | Which Codex surface | **RESOLVED: Surface 1 — grim owns `hooks.json`** (`$CODEX_HOME/hooks.json` global, `.codex/hooks.json` project), with Surface 3 (plugin) as the fallback. See "D7a resolution" below — the researcher on this axis recommended Surface 2, and its own source-verified findings overturn that. |
| ~~D7a~~ *(superseded, kept for the record)* | ~~Which Codex surface~~ | **Reopened — do not settle on the splice.** The first reading favoured splicing `[hooks]` in `config.toml` (the file grim already splices for Codex MCP, same `toml_splice` engine). The full Codex report then established that a hook's identity is a **derived positional key**, `source:path:event_snake:group_idx:handler_idx`, and that **trust is content-hash-gated per entry**. Splicing a shared table therefore makes grim's identity depend on entries grim does not own: a user adding a group above grim's shifts `group_idx`, changing the identity. A **grim-owned file** pins both `path` and the indices, which is strictly better for trust stability — but Codex's `hooks.json` is a fixed filename per scope, so owning it collides with a user's own. The three candidates: own `.codex/hooks.json` when absent and refuse otherwise (65); splice `[hooks]` in `config.toml` and accept identity churn on user edits; or register as a Codex **plugin** (`<plugin-root>/hooks/hooks.json`), which is grim-namespaced — viability unverified. **Needs one more targeted check before it is decided.** |
| D9 | Trampoline naming | **`grim hook run` — a subcommand of the one `grim` binary** (owner decision). No second binary, no `grim-hook` executable. The vendor registration therefore names `grim` and passes `hook run …` as arguments. Where a fixed absolute path is needed (D6), it is a small grim-*generated* shim script at a stable path, never a separate shipped binary |
| D10 | Feature gate | **This is the single most load-bearing control in the design, not a launch-phase convenience** — the ADR must say so in those terms. Evidence (`research_hooks_autoexec_supply_chain.md`): every ecosystem surveyed that shipped *silent execute by default* (npm, RubyGems, PyPI, Homebrew third-party taps, VS Code extensions, Cargo `build.rs`) has since eaten at least one named incident traceable to that default, and the fix that actually changed outcomes was **always a default flip** (pnpm 10, npm v12, Homebrew 6.0 Tap Trust) — never an attestation or logging layer added afterwards (npm provenance and 2FA did not stop the 2025–26 Shai-Hulud worm or the 2026 TanStack compromise). **Off by default, behind a named experimental flag** (owner decision): a boolean feature-flag table, recommended `[options.experimental]` with `hooks = false`, so it inherits the existing dotted-key CLI (`grim config set options.experimental.hooks true`) and the `[options.vendors.<name>]` precedent for a new nested option table. Gated off ⇒ hook artifacts resolve and lock normally but `grim install` **skips them with a warning** and `grim status` reports them gated — reusing the `Declined`-kind reporting path rather than inventing a new one. Note the forward-compat shape: a hand-written config using the new key needs the new grim (78 on an older one), which is exactly how `[options.vendors]` landed |
| D8 | Exec bit in the packer | **Open**, and now nearly moot: the trampoline means no client execs a registry-delivered file. Cline is the one exception in the wider roster (the hook *is* an executable whose filename is the event) — and grim generates that file locally, so it can `chmod` it freely |

## Unsupported events and tiers — the projection rule

"Claude-shaped" must mean **Claude's names and wire format, not Claude's breadth.** Claude has
30+ events; the canonical set is the handful that exist everywhere. Everything else is authored
as a native per-vendor event, exactly as `vendor.rs`'s owner decision already prescribes for
metadata: common capability → canonical top-level field; vendor-unique → `<vendor>.<field>`.

**Authoring surface — one rule, already shipped.** `event` is canonical; `<vendor>.event`
**overrides** it for that vendor, or stands alone (making the hook vendor-only). That is the
same override precedence already documented for agent fields — "a lifted `<vendor>.*` key
overrides its common field, silently, because the collision is the documented escape hatch"
(`docs/src/vendor-metadata.md` § agent-overrides). No new mechanism.

**Syntax: no quoting needed.** A dotted TOML key whose segments are bare (ASCII letters,
digits, `_`, `-`) needs no quotes, and it **nests into a real table** — verified with `tomllib`
2026-08-14:

```toml
[[hooks]]
id      = "shell-guard"
event   = "PreToolUse"
tier    = "gatekeeper"
argv    = ["sh", "${GRIM_HOOK_DIR}/guard.sh"]
cursor.event = "beforeShellExecution"     # parses to hooks[0].cursor.event
claude.timeout = 60                       # natively an integer, not a string
```

Three consequences:

1. `"gemini.event" = "…"` was wrong in the first sketch — that spelling was carried over from
   YAML frontmatter, where `cursor.readonly` is a *literal string key* inside a string-valued
   `metadata` map. In TOML the same *spelling* nests, so the author writes what they already
   know from skills/rules/agents, and grim gets real structure.
2. **Per-vendor overrides are natively typed.** `claude.timeout = 60` is an integer with no
   `FieldType::Json` workaround — the constraint `adr_structured_vendor_metadata.md` exists to
   work around does not apply to a TOML-authored kind.
3. **Vendor names become reserved keys** in a hook table. Acceptable and checkable: grim already
   maintains the closed client list (`ClientTarget::ALL`, `render::reserves_namespace`), so a
   hook using a client name for anything but a vendor override fails `grim build` with a clear
   message. The alternative — a `vendor.<client>.<field>` container, which has *closer* parity
   with the shipped `metadata:` container and no reserved words — is equally valid TOML and is
   the fallback if the reserved-word cost is judged too high.

**Chosen form (owner, 2026-08-14): single-line inline table** for a multi-key vendor override —
`cursor = { event = "…", fail = "closed" }` — with the repeated dotted prefix as the multi-line
form of the *same* structure:

```toml
cursor.event   = "beforeShellExecution"     # identical parse to the inline table above
cursor.fail    = "closed"
cursor.timeout = 20
```

**Do not bless multi-line inline tables, even though grim's own parser accepts them.**
Newlines inside `{ … }` and trailing commas are **TOML 1.1** features:

- grim's shipped parser implements them — `toml 1.1.4+spec-1.1.0` / `toml_edit 0.25.13+spec-1.1.0`,
  whose inline-table ABNF is the 1.1 rule
  `inline-table = inline-table-open [ inline-table-keyvals ] ws-comment-newline inline-table-close`
  (source-level evidence from the vendored crate, plus `set_trailing_comma`; **not** verified by
  an executed round-trip — compiling a probe was not permitted in this session);
- a stock **TOML 1.0** parser hard-rejects both. Verified with Python `tomllib` 2026-08-14:
  multi-line inline table → `Invalid initial character for a key part`; trailing comma → same.

`hook.toml` is a **published artifact format** authored by third parties and read by *their*
tooling — editor plugins, `taplo`, CI validators, Python/Go scripts over a catalog. Blessing
1.1-only syntax pushes a 1.1-parser requirement onto every one of those consumers, and the
failure mode is a hard parse error rather than a warning. Under the 1.0 stability freeze the
format is a frozen contract, so the documented dialect must be the **1.0-compatible subset**.

Posture: **liberal in what grim accepts, conservative in what it documents and emits.** grim may
keep parsing multi-line inline tables (its parser does anyway); the docs, examples, and anything
grim itself writes stay 1.0-valid.

Also available but not recommended: an explicit `[hooks.cursor]` sub-table header — valid, but
order-dependent (it attaches to whichever `[[hooks]]` came last) and visually detached from its
hook. **Verified gotcha:** a dotted key *and* an explicit header for the same table is a hard
error in both dialects (`Cannot declare ('hooks','cursor') twice`) — pick one per vendor per hook.

**The projection rule.** A canonical event may project onto a differently-named native event
**iff it fires at the same moment AND the native surface's power is ≥ what the hook's tier
requires.** Otherwise it declines. This admits the useful cases and forbids the dangerous one:

- *Allowed — renaming:* Gemini's `BeforeTool` **is** `PreToolUse`. Same moment, same power.
- *Allowed — narrowing:* Cursor's `beforeShellExecution` is `PreToolUse` restricted to shell.
  A hook declaring `event = "PreToolUse"`, `matcher = "Bash"` may project onto it — same moment,
  same blocking power, and the narrower surface is a *more precise* fit, not a lossy one.
- **Forbidden — substitution:** never map a hook onto a *different moment* because it is
  "similar". A `PreToolUse` guardrail relocated to `PostToolUse` would run after the damage.
  Silent moment-substitution is how a fidelity decision becomes a security hole.

**Three failure modes, three existing verdicts.** `KindSupport` resolved per hook × client:

| What is missing | Verdict | Behaviour |
|---|---|---|
| The event does not exist on that client at all (e.g. `SessionStart` on Antigravity, which has only 5 events) | **Declined** | warn at install, record zero outputs for that client, `grim status` shows it |
| The event exists but the client cannot honour the hook's **tier** (`gatekeeper` on a Goose observation-only event; `mutator` on kiro / goose / cline / antigravity) | **Declined** | warn at install — **never degrade a guardrail into a logger** |
| Event and tier are fine, but one **response field** has no equivalent (e.g. `context` on a client with no context injection) | **Degraded** | install, drop the field, warn once — fidelity loss without safety loss, the shipped meaning of `Degraded` (OpenCode rules dropping `paths`) |

**Anti-over-engineering commitments.** Deliberately small, because adding is additive and
removing is breaking (Principle 9):

- **4 canonical events** to start: `PreToolUse`, `PostToolUse`, `SessionStart`, `Stop`.
  `PreToolUse` and `Stop` are the only two present in all 15 hook-capable clients; the other two
  are present in nearly all. Every other event is native-only at v1.
- **One handler kind**: `command` / `argv`. Claude's `http` / `prompt` / `agent` / `mcp_tool`,
  Cursor's `prompt` and Kiro's `agent` are native-only and out of v1 — the LLM *is* the handler
  there, so there is no process for a trampoline to stand in for.
- **One matcher dialect** — grim's own, because the dispatcher matches internally and registers
  with the widest vendor matcher.
- **One closed response shape.**

## D7a resolution — Codex: grim owns `hooks.json`

Evidence: [`research_hooks_codex_surface.md`](./research_hooks_codex_surface.md),
source-verified against `openai/codex` `main` @ `8630bb3c` (2026-08-14).

**The decisive fact.** Codex's hook content hash (`hook_hash`) covers only
`(event_name, matcher, normalized handler)` and is **format-independent** — but
the persisted trust record is looked up by a **positional key**,
`path:event:group_index:handler_index`, with **no hash-based fallback**. So a
third party or the user inserting a hook group *above* grim's, in the same
event array of the same file, shifts `group_index`, orphans the trust record,
and grim's byte-identical hook **silently reverts to Untrusted** — which means
silently **not executing** (source-confirmed: skipped, never a hard error).

For a `gatekeeper` or `mutator` hook that is a **security failure, not an
inconvenience**: the guardrail stops guarding and nothing says so. And the
mitigation the axis proposed — "emit one matcher group per event, comment-flag
it as grim-owned" — cannot work, because the shift is caused by an edit grim
does not control.

Owning the **file** pins both `path` and the indices, so nothing external can
move grim's key. That is what decides it:

| Surface | Verdict |
|---|---|
| **1 — grim owns `hooks.json`** | **Chosen.** Positional key is stable because grim owns the path and is the only writer. No splice engine needed at all (`own-a-file`, the rule-materialization path). Collision with a pre-existing user file is handled by grim's **already-shipped** untracked-destination refusal (exit 65, `--force` to adopt) — not a special case. Crucially, `hooks.json` and inline `[hooks]` **union** rather than one winning, so grim owning `hooks.json` never deprives the user of a place for their own hooks: `config.toml` still works. |
| 2 — splice `[hooks]` in `config.toml` | **Rejected on two independent grounds.** (a) The positional-key instability above — a shared array is exactly the wrong home. (b) **F12**: `toml_splice` has *no* array-element function, so this needs a brand-new TOML array-of-objects primitive — a second new splice engine, for one client. |
| 3 — Codex plugin | **Fallback.** `.codex-plugin/plugin.json` is optional-fields-only (`{}` is valid, name from dirname) and gives grim a fully namespaced path *and* dictionary-keyed identity — but a bare directory drop is **not** auto-discovered: it needs `[marketplaces."grim"]` + `[plugins."<name>@grim"]` written into `config.toml` (both named-key tables, which the *existing* `toml_splice` does handle), plus a probable Codex-owned cache copy that is **unconfirmed**. Plugin hooks are still non-managed, so they clear the same trust gate — no friction saved. Adopt only if `hooks.json` collisions prove common in practice. |

**Hard prohibition — grim must never forge another tool's trust record.**
Codex reads hook trust state **only** from the User and SessionFlags config
layers (`hook_states_from_stack`), i.e. every approval for a hook of any scope
lives in the user's own `~/.codex/config.toml`. Open issue `openai/codex#21615`
shows third-party integrators self-writing
`[hooks.state."<key>"] trusted_hash = "…"` there as an unofficial workaround to
skip the prompt. **grim does not do this**, and the ADR must say so explicitly:
writing a trust record for grim's own code into a consent mechanism grim does
not own would silently defeat the exact control that makes Codex hooks safe,
and it would do so on the user's behalf without asking. grim's own
digest-pinned approval (D5) is grim's gate; Codex's `/hooks` review is Codex's,
and the user clears it. The same rule generalizes to Gemini's
`trusted_hooks.json`.

**Watchlist.** `openai/codex#32491` (headless `codex exec` ignoring persisted
trust) is still open, but its latest comment (2026-08-09) reports
non-reproduction on stable 0.147.0 — likely fixed between 0.144.1 and 0.147.0,
unconfirmed by a maintainer. Re-check before relying on headless behaviour.

**Strategic aside, feeding the plugin-mode forward-compat paragraph.** The same
plugin manifest is accepted at **`.codex-plugin/plugin.json`,
`.claude-plugin/plugin.json`, and `.cursor-plugin/plugin.json`** — deliberate
cross-vendor compatibility. Combined with `adr_render_layout_stability` §2
(plugin rendering as an accepted post-1.0 render mode) and Agent Plugins 1.0,
this makes the future plugin-render mode a plausible **single** carrier of hook
registrations for Claude, Cursor and Codex at once. That is a reason to keep the
v1 registration layer thin and behind one seam, not a reason to build plugin
mode now.

## D10a — The flag needs no new precedence rule; only the CI escape is global-only

**Owner reframe, 2026-08-14.** The panel's Block (quality B2) was that a cloned repo could ship
`hooks = true` plus a pre-approved digest list and arm a hook with no human. My three proposed
fixes all invented a cross-scope precedence rule (two-key AND, global-only, or prompt-only).

None is needed. Verified: **grim never merges config across scopes.**
`src/command/scope_resolution.rs:6` — *"Each of those commands operates on exactly one scope
(global or project)"* — and `grim config list` is documented as "never merged across scopes"
(`subsystem-cli-commands.md:22`). So `[options.experimental] hooks` should be read exactly like
every other `[options]` key: **from the scope the command operates in**, and nothing else. That
*is* "global or project", already, with no new semantics, no second knob, and no precedence
table to document.

**Where the security weight actually sits.** With the flag carrying no cross-scope magic, the
control that stops a cloned hook arming is the one already decided: **per-hook digest approval,
keyed on `(digest, scope root)`**. A cloned repo may enable the *feature*; every hook in it still
faces an approval prompt bound to that workspace, so nothing arms without a human.

**The one piece that must be global-only is the CI escape**, precisely because it is the thing
that bypasses the prompt: `--allow-hooks` / `GRIM_ALLOW_HOOKS=1` / an approved-digest list are
honoured from **global config or the invoking environment only**, never from a project
`grimoire.toml`. A repo cannot carry its own permission to skip approval. That is one sentence
and one test, instead of a precedence model.

**Design note for reuse.** `[options.experimental]` is intended as a **general facility** for
rolling a feature out to test before everyone can use it — not a hooks-specific knob. It should
therefore be specified as a plain boolean table under the existing `[options]` shape
(`VendorOptions` / `TuiOptions` precedent, `config/declaration.rs:111-176`), with the flag name
as the only hook-specific part.

## Compatibility: a hooks-unaware reader must not break

Owner requirement, 2026-08-14 — and it splits into three cases with different answers.

| Reader | What it meets | Behaviour required |
|---|---|---|
| **An older `grim`** (no `Hook` variant) reading a lock/config/state that carries hooks | `deny_unknown_fields` is on `LockedArtifact` (`src/lock/locked_artifact.rs:15,70`) and on the config option tables (`src/config/declaration.rs:118,166`), and both error paths note they "also fire on `deny_unknown_fields`" (`lock_error.rs:67`, `config_error.rs:62`) | **A hard error is unavoidable and correct** — you need the new grim to use the new feature. What matters is that it is a *clean, explanatory* error naming the version requirement, not a bare TOML parse failure. Needs a test asserting the message. |
| **A current `grim` with the flag off** meeting a declared hook | resolves and locks normally | Must **not** error: warn + skip + report `gated` in `grim status`, reusing the `Declined`-kind reporting path. This is the case that must be graceful, and it is the common one. |
| **A project with no hooks** | nothing | **Byte-identical** files. Guaranteed by the `adr_agent_artifact_kind.md` precedent: emit the hook sections only when non-empty. |

### `grimoire-vscode` — VERIFIED SAFE, and by deliberate design

Checked directly against `../grimoire-vscode` (checked out 2026-08-14). Both worries are already
answered, and not by luck — the extension documents the pattern:

- **An unknown `kind` degrades, never throws.** `webview/model.ts:100-102`:
  `normalizeKind(kind)` returns `(KINDS as string[]).includes(k) ? k : null`, and
  `kind: ArtifactKind | null` is the modelled type throughout (`webview/protocol.ts:73,306`).
  A `"hook"` row normalizes to `null` and renders as a kind-less row.
- **The open/closed split is intentional and written down.** `grim.ts:245-252`, verbatim:
  *"kept as an open `string` here, not a closed union: the JSON contract is frozen/additive, so a
  future grim may add a type this extension doesn't know yet. Narrowing + the 'unknown type
  degrades to a read-only row' rule live in webview/settings (buildSettingsVM), same split as
  `SearchItem.kind` (open here) / `ArtifactKind` (closed, webview)."*
- **New always-present fields are ignored.** No `zod` / `ajv` / `io-ts` / `valibot` in
  `package.json`; parsing is plain `JSON.parse`, so an added field cannot break it. This is what
  the additive-field policy (`subsystem-cli-api.md`) relies on, now confirmed on the consumer side.

**Residual, degraded-but-safe:** a hook artifact appears in the VS Code UI as a row with **no kind
icon**, and it will not match any kind filter, because `KINDS` (`model.ts:87`) drives both the
filter list and `KIND_ICONS` (`:92`). Nothing crashes; the row is simply unclassified. Closing it
is a three-line change in that repo — add `'hook'` to `ArtifactKind`, `KINDS`, and `KIND_ICONS` —
so this is a **coordinated-release note, not a blocker**: grim may ship hooks first and the
extension catches up, with the only symptom being an iconless row.

## D6a — The registration is machine-local, because it is materialized output

**Owner reframe, 2026-08-14, and it dissolves a whole defect class rather than mitigating it.**
The panel found the same Block from two directions — a project-scope Copilot registration commits
one developer's absolute `$GRIM_HOME` launcher path, and on a teammate's clone (spec B1) or in the
cloud agent reading the default branch (quality B1) Copilot's fail-closed `preToolUse` then denies
**every tool call**. My three proposed fixes all treated the committed file as a given: guard the
command, refuse project scope, or both.

The correct framing is that **the registration is materialized output, regenerable from
`grimoire.lock` by `grim install`** — the same property every other kind already has. The lock
travels; the rendered artifact does not have to. So write the registration to each client's
**machine-local** surface and nothing version-controlled ever carries a machine-specific path.

Verified per v1 client:

| Client | Machine-local registration surface | Evidence |
|---|---|---|
| **claude** | `.claude/settings.local.json` — a first-class hook scope ("Single project" precedence), and **Claude Code gitignores it itself** when it saves a setting there | `hooks_vendor_reports/claude.md:73`, and hooks are valid in every scope incl. `settings.local.json` (`:251`) |
| **copilot** | `.github/copilot/settings.local.json` — documented as **"meant to be gitignored"**, and hooks have been definable inline in `settings.local.json` since **1.0.8** (2026-03-18) | `hooks_vendor_reports/copilot.md:86`, `:59`, `:330` |
| **codex** | `$CODEX_HOME/hooks.json` at **user** scope — already outside the repo entirely, so the question does not arise | D7a resolution; no `.local` variant exists in Codex's config-layer list |

What this buys:

- **spec B1 and quality B1 are dissolved**, not mitigated. Nothing commits an absolute path, so no
  clone and no cloud run can inherit one. The cloud agent reads `.github/hooks/*.json` from the
  default branch — grim simply stops writing there.
- **The fail-open guard becomes optional rather than load-bearing**, so Claude's exec form (`args:
  []`, zero shell-quoting surface) can be kept on every client that offers it.
- **D6's byte-stability constraint is unaffected** — the command string still never changes across
  grim upgrades, so Codex's and Gemini's trust hashes still clear once.
- It restores symmetry with grim's existing model instead of adding a Copilot special case.

Two honest caveats, both small:

1. **"Meant to be gitignored" is not "is gitignored."** grim's shipped policy self-manages
   `.grimoire/.gitignore` (`*`) but **never touches the consumer's root `.gitignore`**
   (`subsystem-file-structure.md:297-301`). Claude ignores its own local file; Copilot's is only
   *documented* as local. So grim should **check whether the target is ignored and warn when it is
   not** — a cheap, honest control — rather than silently assuming the convention holds.
2. **The dispatch-table plant (security B-2) is a separate defect and still needs its own fix.**
   That one is about locating `<workspace>/.grimoire/hooks/dispatch.json` without a walk-up, not
   about the registration, so `--root <abs>` in the launcher argv is still required.

Consequence for the "project scope" semantic, worth stating in the ADR: a project-scope hook means
*this hook applies to this project*, while its **registration is machine-local**. Sharing a hook
with a team happens through `grimoire.toml` + `grimoire.lock` — the artifacts that are supposed to
travel — and each teammate's `grim install` renders their own registration. That is strictly better
than sharing a rendered file, and it is what the rest of grim already does.

## Panel findings to fold into the ADR (design content, not review bookkeeping)

### P1 — Ordering is only guaranteed *among grim's own hooks* (residual risk, state it)

Codex and Copilot **union** hook sources rather than one layer winning, so a user's own
hand-authored native hook can sit in the same per-event array as grim's dispatcher entry — and
**the vendor, not grim, decides the firing order between them**. The deterministic serial
pipeline (mutator control 3) covers only the hooks grim dispatches internally; it cannot order
grim's entry against a foreign one.

This is precisely the problem Kubernetes hit with independent mutating admission webhooks, and
their fix was `reinvocationPolicy: IfNeeded` — a webhook opts into being re-run when a later
admission step changes what it already decided on. **No vendor here offers an equivalent**, so
grim cannot fix it; it can only state it. Needs a one-paragraph residual-risk note in the ADR
(the shape decision E already uses for the equal-privilege approval problem) — not a redesign.

Consequence worth naming explicitly: a `gatekeeper` verdict can be correct at the moment grim
issues it and stale by the time the tool runs, if a foreign hook mutates the call afterwards.

### P2 — Reserve a declarative-policy payload as a future extension point

The strongest observation from the gap check: a **pure-decision** hook (allow/deny, no side
effects) does not need to be arbitrary code at all. Sondera demonstrates the shipped form —
Cedar policy evaluated by an engine, not a script — which **eliminates** the RCE surface rather
than containing it, and would make the `gatekeeper` tier dramatically cheaper to trust than the
nine-control regime `mutator` requires.

`hook.toml`'s `argv` / `command` pair leaves no room for this. Recommendation: reserve
`policy = "cedar" | "rego"` on paper now, exactly as `adr_render_layout_stability.md` §4 reserves
`[options] render = "files" | "plugin"` without parsing it — so a future contributor has an
anchor and the schema does not have to break to grow one. Cheap, additive, and it records that
the safest possible gatekeeper is declarative rather than executable.

### P3 — grim's own shipped skill says hooks are NOT a security boundary (doctrinal conflict)

Verified 2026-08-14. The first-party published skill `ai-config-authoring` already treats hooks
as a concept, in **six** places, and one of them contradicts how a `gatekeeper` tier would be
read:

| Site | Text |
|---|---|
| `catalog/skills/ai-config-authoring/references/choosing-types.md:40` | hooks are for "Invariants: format-on-save, lint gates, blocking writes to protected paths, audit logging" — and **not** for "Anything needing judgment; **a *hard* security boundary — use the client's permission system for that**" |
| `.../choosing-types.md:30` | "Deterministic, event-fired …; **never model-invoked**" |
| `.../SKILL.md:32`, `.../references/guardrails.md:21` | `\| Hook \| Zero context cost — prefer it for anything mechanical \|` |
| `.../SKILL.md:42,49` | routes "mechanical, must happen 100% of the time, no judgment" and "logic a machine can run" to Hook |

**This is not stale text to update — it is guidance that is right, and the ADR should adopt it
rather than overwrite it.** A grim-distributed hook is a **soft** gate, and this design's own
findings say why: it fails open by default (D6/control set), the vendor — not grim — decides
firing order against a foreign hook (P1), a `gatekeeper` verdict can go stale before the tool
runs (P1), and every v1 client's trust gate can silently disarm it (D7a, Gemini fingerprints).
So the ADR must state plainly that hooks are **defence-in-depth, not a security boundary**, and
that the client's own permission system remains the boundary. Marketing `gatekeeper` as a
security control without that sentence would put a published first-party artifact and the ADR in
direct conflict.

### P4 — Documentation and catalog punch list (the ADR under-scoped this)

Confirmed against the real files:

- **`docs/src/artifacts.md:17` is `## The five kinds {#kinds}`** — a *linked anchor*, so it is
  both wrong ("five" → six) and load-bearing for cross-references. The ADR never names this file.
- **`catalog/README.md:109` names six trigger pages**, not the two the ADR implies:
  `docs/src/{artifacts,clients,publishing,vendor-metadata,commands,package-index}.md`, plus
  `src/command/**` and `src/mcp/**`. Drift targets are `grim-usage` and `grim-authoring` always,
  **plus `ai-config-authoring` specifically for `clients.md` and `vendor-metadata.md`** — both of
  which a Hook kind touches. So **all three** shipped skills fire, and `task catalog:verify`
  gates it in CI. An implementer trusting the ADR's count under-scopes a mandatory review.
- **`docs/src/vendor-metadata.md:186`** — "`hooks` … a separate ADR governs that surface" — is
  the forward pointer this very ADR is meant to close. Currently cited only decoratively.
- **`docs/src/configuration.md`** documents `[options.tui]` and `[options.vendors]` but would
  have no `[options.experimental]` section — leaving the design's single most load-bearing
  control undocumented.
- **`catalog/skills/grim-authoring/SKILL.md:19`** is `## The Five Kinds`, referenced by
  `references/bootstrap-existing-repo.md:17` as a `[five-kinds]` link anchor; **`grim-usage`**
  carries "five artifact kinds" in its description and body.
- Also unnamed: `docs/src/concepts.md` (kind/client taxonomy prose), `docs/src/json-interface.md`
  (no report shape for `grim hook list`).

**Independent corroboration worth recording:** this reviewer re-verified every `file:line`
citation in the ADR against `HEAD` and found **no** false claims about shipped behaviour — which
also independently confirms this run's three upstream corrections (`json_splice` array ops are
string-only, `toml_splice` has no array primitive at all, `Context::new` is I/O-free and
`schema`/`completions` take `&args` while `tui`/`mcp` take `&ctx`).

### Verified clean by the same pass

ACP (no hook/middleware shape), MCP as of 2026-07-28 (official extensions Tasks, MCP Apps, EMA —
none hook-shaped), Agent Plugins 1.0 (still excludes hooks; no companion spec proposal found).
So there is genuinely no external schema the canonical envelope should be a superset of.

## The command string is a trust key — and the dispatcher is what makes that survivable

Two of the three v1 clients gate a hook on the **content of its command**, not on its identity:

- **Codex** — `HookTrustStatus: managed | untrusted | trusted | modified`, reviewed in a `/hooks`
  TUI, keyed to a content hash. *Editing invalidates trust.* Escape hatch
  `--dangerously-bypass-hook-trust`, whose startup banner reads verbatim: *"…is enabled. Enabled
  hooks may run without review for this invocation."*
- **Gemini** — fingerprint `name:command` (`getHookKey()`), stored in `trusted_hooks.json`,
  re-prompts on change.

This is the sharpest practical argument for **D1 (dispatcher)** that the survey produced, and it
was not visible before the vendor reports landed:

| Design | What the client sees when you install hook #2, or update hook #1 | Result |
|---|---|---|
| One config entry **per hook** | a new/changed command string | **a human trust prompt every time**, and a headless CI stall |
| **One dispatcher entry per event** | nothing — `grim hook run --client codex --event PreToolUse` is unchanged | trust is cleared **once**, then hooks are added, updated and removed with no further prompts |

The corollary is D6: because the string is hashed, it must be byte-stable across grim upgrades.
A versioned or relocatable absolute path would silently re-trigger the trust gate on every
`grim` update, on two clients, one of which (Codex) then refuses to run the hook until a human
clears it. Hence a **fixed-path grim-owned launcher**.

## Mutator tier — mandatory controls

`mutator` rewrites the tool call before it runs. It is the most capable tier and the one real
injection amplifier in the design (a compromised `PreToolUse` mutator turns `cargo build` into
`curl … | sh`). It ships in v1 by owner decision; these controls are what make that defensible,
and none of them is optional.

**1. Capability gate — Decline, never degrade.** A `mutator` hook installs only where the client
can actually express an input rewrite. Verified per client:

| Can rewrite tool input | Field | Cannot (⇒ `Declined` for `mutator`) |
|---|---|---|
| claude | `updatedInput` | **antigravity** — no input-rewrite field (`permissionOverrides` is permissions, not input) |
| cursor | `updated_input` | **kiro** — no structured JSON response at all |
| copilot | **CONTESTED — do not rely on this row.** Corrected 2026-08-14: the primary report documents `preToolUse` returning **`modifiedArgs`** (`hooks_vendor_reports/copilot.md:262`), not `updatedInput`. This row originally read `updatedInput`, inferred from an *issue title* in that report's Sources table (`github/copilot-cli#2013`, "`updatedInput` ignored", closed 2026-04-10) — a closed issue about a field being ignored does not establish the live field name, and the documented response shape disagrees. The VS Code surface documents no input-rewrite field at all, and the field name may vary with config casing. **Treat Copilot `mutator` as `Declined` in v1 pending live-CLI verification** (ADR open question 1). | **goose** — block-or-nothing, binary |
| gemini | tool-arg override in `hookSpecificOutput` / `tailToolCallRequest` | **cline** — VS Code extension's `PreToolUse` explicitly **cannot** rewrite input (the SDK/CLI surface has `overrideInput`, but that is not the surface grim targets) |
| droid | `updatedInput` | warp, zed — no hook surface |
| junie | `updatedInput` | |
| codex | `updatedInput` — **but event-restricted, see below** | |
| opencode, kilo, amp, openclaw | mutate the `output` object in place (JS surfaces) | |

**2. The Codex trap — why the per-event response table is not optional.** Codex accepts
`updatedInput` on one event shape and, on another, documents it as *"Reserved for a future
input-rewrite capability. … fail closed if present."* So emitting the field on the wrong Codex
event **blocks the tool call**. A single pass-through projector would ship that bug on day one.
The response projector must be a per-`(vendor, event)` table with a closed set of permitted
fields, and emitting an unpermitted field must be a build-time/render-time error, not a runtime
surprise.

**3. Deterministic mutation pipeline — a capability no vendor has.** Claude runs hooks in
parallel and resolves competing `updatedInput` as last-process-to-exit-wins; most other clients
leave ordering `NOT DOCUMENTED`. Because the dispatcher owns fan-out, grim can do better and
must: run mutators **serially in declaration order**, threading each one's output into the next
as input, and emit exactly **one** final `updatedInput` to the client. A race becomes a
pipeline, and the result is reproducible.

**4. Audit — REVISED 2026-08-14, my original wording was the weakest control in the set.**
Evidence: [`research_hooks_autoexec_supply_chain.md`](./research_hooks_autoexec_supply_chain.md).
"Mandatory before/after audit of every mutation, full stop" is necessary but was being asked to
do work logging cannot do, and as written it *creates* a surface: audit logging has **zero**
documented before/after evidence of reducing incidents anywhere in that survey — every mature
system (Kubernetes, CloudTrail, `auditd`) treats it as forensics, not prevention — and logging a
mutated tool input, which may carry a secret, invites secret capture plus log injection
(CWE-117) and unbounded growth (CWE-400) from a hostile mutator's own output.

Corrected control: **audit defaults to a redacted metadata view** (hook id, event, tier, digest,
which fields changed, sizes, a decision verdict) and **gates full before/after body capture
behind a stricter, separately-enabled mode** — the Kubernetes
`Metadata`/`Request`/`RequestResponse` level pattern and CloudTrail's field truncation, both
designed exactly this way. Sanitize control characters on the way in; bound the record size.
Audit remains mandatory; *unredacted* audit does not.

**7. Re-verify the digest at execution time, not only at install time.** The repeated real-world
failure across unrelated systems is **an approval keyed on the wrong identity, checked at the
wrong time**: GitHub Actions tag mutation (CVE-2025-30066), Bun's name-based trust bypass
(CVE-2026-24910), Cursor's create-vs-edit approval gap (CVE-2025-54135 / CVE-2025-54130) and its
case-normalization bypass (CVE-2025-59944). A digest is structurally immune to the mutable-tag
and name-collision variants **only if** grim (a) hashes the artifact that will actually run —
not a bundle wrapper or a declared version — and (b) re-checks it against the file on disk
immediately before `exec`, closing the TOCTOU window between "approved" and "ran".

**Interaction with the hot-path budget (F5), and its resolution.** Hashing on every event would
blow the latency budget. It does not have to: the **no-match path never executes anything**, so
it needs no hash at all. The digest re-check belongs on the *matched* path only, immediately
before spawning that payload — which is already the expensive branch. State this explicitly in
the ADR so an implementer does not "optimize" the check onto the wrong side of the branch.

**8. A mutator's output must be re-parsed by the same parser that will execute it.** This is the
general lesson of `sudo` CVE-2023-22809: a trusted layer that inspects or rewrites a command
line and hands the result to a *different* parser than the one that finally executes it reliably
develops exactly this bug class. So a mutator that receives a shell command **as a string**,
edits the string, and emits a new string for the client's shell to parse independently is not
abstractly risky — it is the precise shape of a real, patched CVE.

Design consequence, and it constrains the tier rather than removing it: **prefer mutating
structured tool input** (argv arrays, typed fields) over shell-command strings. Where a vendor
exposes only a string, either round-trip it through the same parser the executor uses or treat
`mutator` as `Declined` for that tool shape — an honest decline beats a rewrite whose meaning
grim cannot verify.

**9. A hook must not be able to weaken the gate that authorizes it.** PromptArmor's
"Hijacking Claude Code via Injected Marketplace Plugins" (2025-10-16) is directly on point: a
malicious plugin shipped a `UserPromptSubmit` **hook** that overwrote Claude's own
`settings.local.json` permissions, after which a separate injection payload drove a
now-pre-approved `curl` exfiltration. The vendor-side root cause — *no integrity check on what a
hook is allowed to touch* — applies to grim's own approval store: hooks run at user privilege,
and D5's approval records live in `<workspace>/.grimoire/state.json` / `$GRIM_HOME/state/global.json`,
which a hook can write. The ADR must state how grim detects or resists a hook editing its own
approval record; "the file is ours" is not a control.

Related, and already decided in D7a: grim never writes into a *vendor's* trust store either.

**5. Visible to the model, not just to grim.** Where the client supports it, a mutation should
also emit a `systemMessage` / `additionalContext` line describing the rewrite, so the agent's
own transcript records that its command was altered. No vendor does this by default.

**6. Loudest approval copy.** The D5 approval prompt must name the tier in plain language for
mutators specifically — "this hook can rewrite commands before they run" — distinct from the
observer and gatekeeper wording.

## Forward design — v1 ships 3 clients, the design must fit 15

Owner constraint: the abstraction is designed against the full roster now, even though v1
materializes only claude / codex / copilot. Concretely, the seams that must exist in v1 so the
remaining twelve are additive (Principle 9) rather than a redesign:

| Seam | Why it must exist in v1 | Who forces it |
|---|---|---|
| **Install-shape abstraction** on the vendor trait — `OwnFile`, `SpliceConfig`, `CodegenModule` | v1 already spans two shapes; the third must not require reshaping the trait | opencode, kilo, amp, openclaw (codegen) |
| **Per-hook capability resolution** (`Native`/`Degraded`/`Declined` per hook × client, not per kind) | v1's three clients all support every tier, so a per-kind gate would look sufficient and would then have to be widened for the first client that does not | kiro, goose, cline, antigravity |
| **Per-`(vendor, event)` response table** with a closed permitted-field set | Codex's fail-closed reserved field already forces it in v1 | codex now; kiro (no JSON at all), goose (binary) later |
| **Filename-as-identity** support in the own-file shape | v1's own-file clients name their own files freely | cline (filename **is** the event name, from a hardcoded allow-list) |
| **Payload-delivery variance** — do not assume stdin is always readable | v1 is stdin everywhere | kiro's IDE bug #7375 delivers `{}` on stdin and puts the payload in a `USER_PROMPT` env var |
| **Restart/reload reporting** per client | Claude hot-reloads, Codex is fine, Copilot CLI needs a restart — already divergent in v1 | droid (startup snapshot), kilo, amp (no hot reload at all) |
| **Native-event escape hatch** (`<vendor>.event`) | Keeps the canonical set small and honest from day one | antigravity (5 events, none of them `SessionStart`), gemini (`BeforeModel`), cursor (~20) |
| **Non-command handler kinds modelled as native-only** | Prevents a portable hook from silently claiming a surface a trampoline cannot serve | Claude `http`/`prompt`/`agent`/`mcp_tool`, Cursor `prompt`, Kiro `agent` |

## Relationship to existing artifacts

- [`adr_hooks_support.md`](../adr/adr_hooks_support.md) — **Proposed,
  2026-06-03. Needs revision, not implementation.** Its vendor table surveys
  Windsurf / Continue / Aider (not grim clients) and omits 9 of grim's 17; it
  predates the `KindSupport` tri-state, the 17-client expansion, and the
  shipped `sync_config` engine; and its Option 2/3 split does not consider a
  runtime trampoline at all.
- [`research_ide_hooks.md`](./research_ide_hooks.md) — **superseded** by
  `research_hooks_vendor_survey.md` for vendor facts (same date, same staleness,
  wrong client set). Still useful for its portability and security analysis.
- `adr_structured_vendor_metadata.md` — establishes that hooks get a dedicated
  kind rather than an object-valued metadata field (F7).
- `adr_vendor_config_and_selection.md` — the `sync_config` / detection /
  pool-refcount context a hook registration inherits.

## Durable search terms

`claude code hooks reference` · `cursor hooks.json beforeShellExecution` ·
`codex hooks trust model /hooks approval` · `copilot hooks preToolUse
fail-closed` · `gemini cli hooks BeforeModel` · `PreToolUse additionalContext`
· `hook payload stdin schema` · `agent lifecycle event portability` ·
`hook trampoline normalize payload` · `ARG_MAX environment size limit hook`
