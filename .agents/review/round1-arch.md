# Round 1 — architect review (rv-arch)

Verdict: **no Block. Principle 9 clears. 6 Warn, 4 Suggest.**
Scope read: src/, catalog/, docs/, .claude/rules/ (115 files, ~25k insertions).
Did NOT run task verify or any test; every code assertion checked at the cited line.
Did not audit test/ or src/tui/app.rs beyond its hook gates.

## Block — (none)

Two candidates examined and cleared, with reasoning:
1. `src/api/artifact_status.rs:22-24` moves `rename_all` lowercase -> kebab-case. Not a wire
   break: all five pre-hook variants (Installed, Stale, Modified, Missing, Outdated) are single
   words, so both emit byte-identical tokens. Buys `not-armed` over `notarmed`.
2. Plain-table column additions (status 5->6, install 4->5, new hook list 6). `docs/src/stability.md`
   Unstable section: only exit codes and structured JSON are contracts.

## Warn

- **W1 (actionable)** `docs/src/stability.md:373` and `docs/src/configuration.md:187` state
  `grim config set options.experimental.hooks false` / `unset` are "refused with exit 65" and that
  there is "no CLI route back". The code permits both and warns (`src/command/config.rs:707-730`,
  `:795-801`, `:970-973`). Commit order proves the docs stale: reversal `0a51be5` 00:38; doc text
  `75dc8e8` 00:07, never corrected. `catalog/hooks/README.md:268` already documents the shipped
  behaviour, so the branch ships a documented exit code contradicting its own published skill.
  Two stale code comments carry the dead claim: `src/command/config.rs:791`, `:968`.
- **W2 (actionable)** `src/command/status.rs:1093-1116` `requires_client_approval` is a per-vendor
  `match ClientTarget` in a shared module — the D-1 shape `src/install/vendor.rs:1193` names. Its own
  doc defers "promote to `Vendor::hook_approval()` when vendor.rs is next open"; vendor.rs WAS open
  (+1287 lines, eight new hook methods). Total match, so fail-safe, but on the wrong side of the seam.
- **W3 (actionable)** `RUNTIME_SOURCES` (`src/command/hook.rs:104-110`) scans five files; the runtime's
  production imports reach `crate::hook::audit`, `crate::install::hook_dispatch`, `crate::oci::hook`,
  `crate::cli::exit_code`. `hook_dispatch.rs` is shared with the install path where `grim_home()` is
  legitimate. Verified clean today by grep; nothing pins it. Add a closure list with a per-list needle set.
- **W4 (actionable)** `.agents/adr/adr_hooks_support.md:388-392` Decision B still specifies
  `<workspace>/.grimoire/hooks/<name>/` — the pre-SEC-1 layout WP-T removed as a security defect.
  ADR touched once at acceptance (`25bf4d5`), never amended.
- **W5 (actionable)** `.agents/adr/adr_hooks_support.md:336-339` amendment A4 specifies
  `GRIM_EXPERIMENTAL_HOOKS`, deleted by owner decision `24a14bb`. The ADR's own precedence rule makes
  A4 authoritative for an ADR-only reader, and it is false.
- **W6 (actionable)** `.agents/plans/plan_hooks_artifact_kind.md:7-9,74` Status block is three waves
  behind (says wave 6 in flight; waves 6/7/8 have merged).

## Suggest

- **S1 (actionable)** Two generators survive for the string codex hashes: `hook_launcher::registered_command`
  (`:327`) / `registered_command_powershell` (`:374`) are test-only; production is
  `vendor::registration_command` (`src/install/vendor.rs:626`). `vendor.rs:611-624` says exactly one must
  survive merge. Byte-identical today only because `VERDICT_EXIT_CODES` (`hook_launcher.rs:152`) is empty
  for all three v1 clients.
- **S2 (actionable)** `src/api/install_report.rs:72` types tier as `String`; `src/api/hook_report.rs:39`
  types the same concept as `HookTier`. Wire-identical today. `client: String` is correct as-is.
- **S3 (deferred)** `src/install/path_anchor.rs:732` keeps `PathAnchor::Workspace` as a compat candidate
  for a layout no release ever wrote — slightly undercuts the "never shipped" argument.
- **S4 (actionable)** `src/hook/trust.rs:349-357` records an owner-owed ordering decision
  (`trust_hooks = false` beats `--allow-hooks`) only as a code comment.

## Principle 9 verdict (the branch's load-bearing argument, graded)

**Sound, and consistently applied.** `AGENTS.md` scopes the freeze to *released* surfaces; a layout no
released binary ever wrote is not one, and the migration/reaper/self-heal machinery exists to protect
users with files at the old path — there are none. Consistently applied because the branch does NOT reach
for the same argument for the lock and config, where it documents the forward-compat break explicitly with
precedent. Pushback: applied over-conservatively at S3.

Schemas: every addition verified genuinely additive (lock `[[hook]]` skip-if-empty; declaration hash emits
`hooks` only when non-empty at the correct JCS position so DECLARATION_HASH_VERSION stays 1; `trust_hooks`
tri-state; `skip_serializing_if` absent from all of `src/api/` — verified by grep, not by module doc).
Renderers: `json_key` escaping is the identity function on ordinary keys, so self-heal holds.

## Boundaries

C-007 is real: `src/app.rs:59-63` returns before `Context::new` at `:66`, pinned by an offset-comparing
source test; `every_declared_runtime_module_is_checked` catches the add-a-module-forget-to-list failure.
Zero `unwrap/expect/panic!/unreachable!/todo!` in the production half of all five runtime files or
`src/hook/audit.rs`. One trust predicate (`decide`), two compositions (write-side `arming`, read-side
`hook_arming`), three report consumers. Measured code lines: `invoke` 135, `converge_clients` 111,
`project` 106 — no new god function; `install_one` is pre-existing (350, +26).

## Design vs decided

No unrecorded divergence between code and plan. The gap is the ADR was never amended after acceptance
(W4, W5). Bundle membership was already anticipated in the ADR — not a divergence.
