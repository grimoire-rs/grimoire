# ADR: `GRIM_CONFIG`, environment-named config layers

## Metadata

**Status:** Accepted
**Date:** 2026-11-13
**Deciders:** Michael Herwig (owner decisions recorded in `.agents/goals/issue-334.md` § Rules), Claude (hex-plan, `hex/issue-334`, for the open questions in § Decisions recorded)
**Beads Issue:** N/A — tracked as [grimoire-rs/grimoire#334](https://github.com/grimoire-rs/grimoire/issues/334), the "Redirectable collector" point of [grimoire-rs/grimoire#83](https://github.com/grimoire-rs/grimoire/issues/83)
**Related PRD:** N/A — goal file `.agents/goals/issue-334.md`
**Tech Strategy Alignment:**
- [x] Decision follows Golden Path in `.claude/rules/product-tech-strategy.md` (std `split_paths`, the `toml` crate already in the graph, no new dependency)
- [ ] OR deviation justified in Rationale section
**Domain Tags:** integration, security, devops
**Supersedes:** the "B alone, sealed" recommendation of `.agents/research/research_otel_scoped_destination.md` (a research recommendation, not an ADR)
**Superseded By:** N/A

## Context

A tool manager can deliver a file and environment variables, nothing more
(#334). ocx patches, direnv, devcontainers and shared profiles all compose one
environment for many tools, so a standard `OTEL_*` variable set for grim
reaches every OpenTelemetry-aware process. Grim needs a delivery channel only
it reads. Grim has no config layering today: the global and project
`grimoire.toml` are two scopes that never merge (`src/config/global_config.rs`
module doc), and `RawConfig` is `deny_unknown_fields`.

This ADR records the general mechanism. What the `[telemetry]` table means is
an amendment to `adr_otel_usage_telemetry.md`.

## Decision Drivers

- **Delivery without writes.** The deployer ships a file and sets a variable;
  nothing lands in `$GRIM_HOME` or a project.
- **Only grim reads it.** The variable and the file are grim-namespaced.
- **The user stays in control.** The user's own settings and every opt-out win
  over a deployer's file.
- **Room to grow.** The owner wants the same files to carry other
  organisation defaults later, starting with `[options.experimental]`.

## Industry Context & Research

**Research artifact:** `.agents/research/research_otel_scoped_destination.md`
(tool survey, env-pointed layers in git, pip, mise, Gemini CLI and kubectl).
The ocx `${installPath}` check is recorded in that file's 2026-11-13 addendum.
**Key insight:** tools that fear repository-injected environment gate keys by
location, never by pointer. Grim follows that: the pointer is trusted as the
user's environment, and what a pointed-at file may carry is limited by key.

## Considered Options

### Option 1: one `GRIM_CONFIG` variable holding a path list of TOML layers (chosen, owner decision)

| Pros | Cons |
|---|---|
| One grim-only variable, any deployer can set it | A new config surface with its own precedence story |
| Several layers compose (org, team, user) without a tool rewriting a file | Grim had no layering, so this is new code, not reuse |
| Telemetry is one table; later tables are additive | A repository-supplied environment can name a file (T3, see below) |

### Option 2: grim-prefixed telemetry variables only (`GRIM_TELEMETRY_OTLP_ENDPOINT` and siblings)

| Pros | Cons |
|---|---|
| Smallest change | Covers telemetry only; every later org default needs another variable family |
| No file parsing | Credentials still have to live in the environment |

### Option 3: a fixed-path managed layer (`/etc/grimoire/…`)

| Pros | Cons |
|---|---|
| Location-gated, so a repository cannot point at it | Needs root or MDM; a tool manager cannot write there |
| | Sits above the user, against "the user stays in control" |

## Decision Outcome

**Chosen option:** Option 1, decided by the owner. Option 2 survives as one
variable, `GRIM_TELEMETRY_OTLP_HEADERS` (telemetry ADR amendment), for a
credential that already lives in the environment.

### The mechanism

1. **Value.** `GRIM_CONFIG` holds a list of file paths split with the
   platform path-list separator (`std::env::split_paths`: `:` on Unix, `;`
   on Windows). A blank value is unset. An empty entry is skipped, as `PATH`
   does.
2. **Entries are absolute.** A relative entry is refused (see Decisions
   recorded, D-3). No `~` or variable expansion.
3. **Files are regular TOML files**, read through `config::read_capped` (the
   same size cap as `grimoire.toml`). An entry that is not a regular file (a
   directory, a FIFO, a device) is refused before it is opened: opening a
   FIFO with no writer blocks, and the read runs in `Context::new`, so it would
   hang every command. Swapping the path between check and open needs write
   access to it (N2). Each file is one layer.
4. **Later wins.** The loader only reads and parses; each consumer merges
   its own table, in list order. The rule every consumer follows: a scalar key
   in a later file replaces the earlier value, and a table of free-form entries
   (headers, resource attributes) merges per entry, later entry wins.
5. **What a file may carry.** Version 1 reads only `[telemetry]`. Any other
   top-level key is ignored, with one warning per run that names the file and
   the keys. **Never from a `GRIM_CONFIG` file:** workspace consent records
   (hook and settings), artifact declarations (`[skills]`, `[rules]`,
   `[agents]`, `[mcp]`, `[hooks]`, `[settings]`, `[bundles]`) and registry
   credentials. A later version may admit `[options.experimental]`: the flag
   is not the security gate, because a committed `grimoire.toml` can already
   set it; consent is the gate, and consent has no file or environment form.
6. **No write path.** `grim config` neither reads nor writes `GRIM_CONFIG`
   files, and reports nothing from them in version 1.
7. **Read lazily, by the consumer.** In version 1 the telemetry gate reads
   the layers in `Context::new`, and only when no environment opt-out has
   already decided the run (telemetry ADR amendment). An opted-out run never
   opens a `GRIM_CONFIG` file. The `grim-hook` launcher never reads it.

### Precedence, lowest to highest

| Rung | Source | Status |
|---|---|---|
| 1 | Generic ambient variables (`OTEL_*`) — only for keys the consumer says they fill | v1 |
| 2 | `GRIM_CONFIG` files, left to right | v1 |
| 3 | An auto-detected user config file (for example a `[telemetry]` table in `$GRIM_HOME/grimoire.toml`) | **reserved, not in v1** |
| 4 | Grim-namespaced environment variables (`GRIM_TELEMETRY`, `GRIM_TELEMETRY_OTLP_HEADERS`, `GRIM_OFFLINE`) and the cross-tool opt-outs (`DO_NOT_TRACK`, `OTEL_SDK_DISABLED`) | v1 |
| 5 | Command-line flags (`--offline`) | v1 |

A project `grimoire.toml` is not a rung for any key a `GRIM_CONFIG` file may
carry, and never will be for a telemetry destination: a committed file would
redirect telemetry on clone with no environment at all.

### Decisions recorded

Question → research → decision, for each open point the owner left to the plan.

- **D-1. What does an unparseable or unusable `GRIM_CONFIG` file do?**
  Research: telemetry must never change an exit code (`Telemetry::finish`
  doc, `test_export_failure_never_changes_outcome`). Skipping a broken layer
  has two failure modes. A broken organisation layer would let a run that is
  still armed (by `GRIM_TELEMETRY` or another layer) fall back to a different
  destination, such as a host-wide `OTEL_*` endpoint. A broken later layer
  may be the one that carries the user's `enabled = false`. (The environment
  opt-outs are not at risk: they are checked before any file is read.)
  Decision: **fail closed for the consumer, never for the command.** This
  includes `GRIM_TELEMETRY=1` with a broken layer: the run is unarmed, because
  the layer could have named the destination. A missing, unreadable, oversized, non-TOML or
  type-invalid layer, or a relative entry, prints one warning on stderr and
  leaves telemetry unarmed for the run. The command, its stdout and its exit
  code are unchanged. The warning names the file path and, for a parse error,
  the line and column only; it never quotes file content, because the failing
  line can hold a header value (the `toml` crate's error text quotes the
  source line). An unknown key inside `[telemetry]` is not an error: it is
  named in the ignored-keys warning and skipped, so a file written for a newer
  grim still works. Considered and rejected: skipping a missing file with a
  debug line, so an optional entry does not warn on every command. Version 1
  has no optional-entry use case, and a mistyped organisation path would then
  drop the organisation's destination silently.
- **D-2. Stability class.** Research: the telemetry surface is Unstable
  until 1.0 (`stability.md` § Unstable, Telemetry bullet); this mechanism's
  version 1 deliberately leaves rung 3 and every non-telemetry table open.
  Decision: `GRIM_CONFIG`, the `[telemetry]` table and
  `GRIM_TELEMETRY_OTLP_HEADERS` are **Unstable until 1.0**, listed in the same
  `stability.md` bullet and excluded from the frozen environment-variable row.
  Freezing them now would freeze precedence before rung 3 exists.
- **D-3. Relative entries.** Research: a relative entry resolves against the
  working directory, so a user who writes `GRIM_CONFIG=grim.toml` in their own
  profile would read a committed `grim.toml` from every repository they `cd`
  into: a T3 path with no repository environment at all (CWE-426, untrusted
  search path). The ocx deployment uses an absolute `${installPath}` value.
  Decision: refuse relative entries (D-1 handling).
- **D-4. Where a later auto-detected user config file sits.** Research: the
  issue asks for "an extra settings layer below the user's own config"; the
  ocx chain and grim's own `GRIM_DEFAULT_REGISTRY` keep environment as the top
  rung. Decision: **rung 3, above every `GRIM_CONFIG` file and below the
  grim-namespaced environment.** Adding it later changes no rule inside the
  `GRIM_CONFIG` rungs. The ADR that adds it must say whether it counts as a
  layer for the telemetry destination unit: under per-entry header merge, a
  user file that sets `endpoint` would receive an organisation layer's
  headers. A system-wide file, if one is ever added, sits below `GRIM_CONFIG`.

### Threat model (scoped to `.claude/rules/arch-threat-model.md`)

- **T3, repository-supplied environment names a file.** `.envrc`, a trusted
  `.mise.toml`, devcontainer `containerEnv` or CI `variables:` can set
  `GRIM_CONFIG` to a repository file that arms telemetry and names an
  endpoint. That collects command names and artifact references, and
  receives `GRIM_TELEMETRY_OTLP_HEADERS` or an organisation file's headers if
  the repository layers its file after the organisation's. This is the class
  the telemetry plan's decision 19 (adversary A-1) already records and the
  owner accepted: every mechanism that injects repository environment already
  grants that repository code execution, or ownership of the environment, and
  such code can read the organisation file directly. **No new gate; no
  widening.** It narrows the exposure in one place: ambient
  `OTEL_EXPORTER_OTLP_*` headers never reach a `GRIM_CONFIG` endpoint.
- **T3 without environment, closed by D-3.** Only a relative entry could make
  a committed file a layer; it is refused.
- **I1 (nothing armable in a repository).** No `GRIM_CONFIG` key arms
  execution. Consent never comes from a file (mechanism point 5), so a future
  `[options.experimental]` in a file changes no gate.
- **I6 (no secrets in argv or environment).** I6 governs what grim puts into
  argv and child environments; grim adds nothing to either. It reads an
  operator-supplied credential from the environment, as it already does for
  `OTEL_EXPORTER_OTLP_HEADERS`, `GRIM_RATE_TOKEN` and `GRIM_ANNOUNCE_TOKEN`.
  Every child process grim spawns (git, `gh`/`glab`, MCP servers) inherits
  `GRIM_TELEMETRY_OTLP_HEADERS` from grim's environment, as it inherits
  `GRIM_RATE_TOKEN` today; that is the same class, so the docs recommend the
  file form for credentials.
- **ERR-17 (no secret in a log line).** Header values stay in
  `secrecy::SecretString`; warnings never quote file content; endpoint
  userinfo is refused and never echoed.

### Consequences

**Positive:** a fleet deploys grim telemetry with one variable and one file,
and no other tool sees the destination or the token. The same channel can
later carry other organisation defaults additively.

**Negative:** a second precedence story beside the two `grimoire.toml`
scopes. Every run with `GRIM_CONFIG` set and no opt-out reads its files
(bounded by the size cap). A misconfigured file prints a warning on every
command until fixed.

**Risks:** a later table admitted from `GRIM_CONFIG` that turns out to be
security-relevant. Mitigation: mechanism point 5 is the gate list, and adding
a table is an amendment to this ADR.

## Validation

- [ ] Contracts C-001…C-003 and C-007…C-008 of `.agents/plans/plan_grim_config_telemetry.md` pass their named tests
- [ ] `stability.md` lists the three names under Unstable (`test_stability_page_lists_grim_config_as_unstable`)

## Links

- [Plan](../plans/plan_grim_config_telemetry.md)
- [adr_otel_usage_telemetry.md](./adr_otel_usage_telemetry.md) — the `[telemetry]` table, arming and destination rules
- `.claude/rules/arch-threat-model.md` — T3, I1, I6
- `.agents/plans/plan_otel_usage_telemetry.md` § Decisions recorded, item 19

---

## Changelog

| Date | Author | Change |
|---|---|---|
| 2026-11-13 | hex-plan | Initial record of the owner's `GRIM_CONFIG` decisions and the four open points (D-1 to D-4) |
