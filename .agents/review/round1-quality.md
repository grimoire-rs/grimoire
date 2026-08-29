# Round 1 — quality review (rv-quality)

Verdict: **0 Block, 9 Warn, 7 Suggest.**
Read: production half of all 16 hook modules plus hook-touching diffs in installer.rs, status.rs,
vendor.rs, path_anchor.rs, error.rs, command_error.rs, declaration.rs, hash.rs, grimoire_lock.rs,
effective_set.rs, resolver.rs, tui/app.rs, api/{artifact_status,hook_report}.rs.
Not reached in depth: json_splice.rs (+1376, only the two `*_nested_handler` entry points),
prune.rs, target.rs, client_target.rs, add/remove/update/publish.rs, and `mod tests` bodies.

## Block — (none)

Evidence for the dispatcher's core claims: split all 16 hook modules at `mod tests` and grepped the
production half — zero `unwrap()`, `expect(`, `panic!`, `unreachable!`, `todo!`; every path in
`run`/`dispatch`/`invoke`/`spawn_payload` returns Success or degrades to `no_opinion`; `HookError`
(`src/oci/hook.rs:1365-1460`) is thiserror with `#[source]` and lowercase no-period messages;
`Error::Hook` classifies to typed `ExitCode::DataError` (`src/error.rs:216`); `anyhow` only at
command boundaries.

## Warn — most severe first

- **W1 (actionable)** `src/install/vendor.rs:566-628` + `src/install/hook_launcher.rs:320-348` — two
  generators of the string **codex hashes**, each with its own `posix_single_quote`. The dead one's
  `expect` reason names consumers that landed and did not consume it. Guarded by `vendor.rs:1911`
  pinning them byte-for-byte, but that test loops a literal `[claude, codex, copilot]`, so a fourth
  hook-capable client with a non-empty `VERDICT_EXIT_CODES` diverges silently — and codex hashes the
  raw command text, so one byte un-trusts every approved hook.
- **W2 (actionable)** `src/install/hook_dispatch.rs:338-386` — two doc comments merged into one. The
  `///` block on `existing_root_token` opens with `root_token`'s contract ("creating that key on first
  use") and carries a `# Errors` claiming "An I/O failure CREATING …" — the opposite of the function it
  sits on, whose whole purpose is that a read-only command must not create key material. `root_token`
  (`:383`), the minting entry point, has no `///` at all. Split at line 352.
- **W3 (actionable)** `src/install/hook_registrar.rs:182-200` — two dead `From` impls; `From<DispatchError>`
  maps `Io(_) => DispatchLocked`, so a disk failure would report as a lock contention. Both
  `converge_clients` sites already match `DispatchError` explicitly and correctly. Delete both.
- **W4 (actionable)** `src/command/hook/pipeline.rs:640-655` — `outcome = RewriteDiscardedUnlogged` is set
  for every unlogged mutator including one that returned no `updated_input`; only `verdict = Some(Mutate)`
  is inside the `if`. `src/hook/audit.rs:192-204` defines that variant as "the rewrite was dropped", so the
  trail can read as a suppressed mutation that never existed. Move line 652 inside the `if`.
- **W5 (actionable)** `src/command/hook/pipeline.rs:796` vs `:839-842` — **the timeout does not bound the
  invocation the doc says it bounds.** `tokio::time::timeout` wraps only the `read` future; the success arm
  then `child.wait().await` unbounded, and `kill_on_drop` cannot fire while the Child is alive in that await.
  A payload that writes its answer, closes stdout and keeps running blocks the dispatcher — and the user's
  tool call — indefinitely. The over-cap case is NOT affected (stdout is moved into `read` and dropped, so a
  payload writing past `MAX_RESPONSE_BYTES` takes EPIPE/SIGPIPE). Wrap `child.wait()` in the remaining
  budget and `start_kill` on expiry.
- **W6 (actionable)** `src/command/status.rs:886-902` + `src/install/hook_registrar.rs:485-490` +
  `src/install/installer.rs:1206` — **three spellings of one predicate.** `status::client_has_hook_surface`'s
  doc says the Hook arm "is WP-J2's to add" and instructs collapsing to `client_supports_kind` in the same
  change; the arm landed on this branch, the collapse did not, and status.rs already imports
  `client_supports_kind`. `hook_clients` is a third copy. All three agree today, compared term by term.
  `path_anchor::is_declined_global_pair` is correctly NOT a fourth (scope-blind by contract).
- **W7 (actionable)** `src/install/installer.rs:457-462` vs `src/command/command_error.rs:83-100` — the
  reserved-name sentence exists twice and has already diverged.
- **W8 (actionable)** `src/install/hook_registrar.rs:914-915` — `owns_anything`'s doc claims
  `json_splice::owned_nested_handlers` "is not yet implemented (WP-D)". It is (`json_splice.rs:772`) and is
  called in production (`hook_registrar.rs:876`); the same doc then argues why swapping it in now would be
  wrong. Delete the stale clause, keep the load-bearing half.
- **W9 (actionable)** `src/command/hook_consent.rs:157-167` — an accepted grant whose `persist` failed is
  pushed onto `declined`, so the verdict becomes `ConsentDeclined` → "hook trust for this registry was
  declined". The user accepted; the WRITE failed. `src/hook/policy.rs:82-84` states that variant exists
  precisely because "you said no" and "nobody could be asked" have different remedies — this path names the
  wrong one, and the true remedy survives only in a `warn!`.

## Suggest

- **S1 (deferred)** `effective_set.rs:55`, `resolver.rs:225`, `status.rs:598`, `tui/app.rs:1770` — four
  hand-maintained copies of "the kinds a DesiredSet holds", all four edited by this branch. Add
  `DesiredSet::tables()`. The two deliberately partial arrays must stay hand-spelled.
- **S2 (actionable)** `src/command/hook/list.rs:296` vs `src/oci/hook.rs:772` — the `"event"` vendor-override
  key is spelled in two modules.
- **S3 (actionable)** `src/install/hook_launcher.rs:211` — doc names `ArmRefusal::LauncherPathControlChar`,
  which does not exist.
- **S4 (actionable)** `src/command/hook/list.rs:212-216` — `inputs: Option<&HookArmingInputs>` is `None` in
  all four unit tests, so every test of `assemble` skips the `merge_not_registered` branch (P-1's per-entry
  reporting half).
- **S5 (actionable)** `src/command/hook/run.rs:366` — `println!` panics on a broken pipe inside a module
  whose doc states twice that nothing panics.
- **S6 (actionable, cosmetic)** `src/command/hook/run.rs:186` sentence does not parse;
  `src/command/hook/pipeline.rs:894` `let _ = &mut command;` has no comment.
- **S7 (actionable)** `src/hook/policy.rs:406-411` lists the four `NotArmedReason`s literally, so a fifth
  can share a message and keep the test green — the D-5 shape.

## Comment accuracy — what was sampled

~30 load-bearing claims verified across 12 files. **Six wrong** (W2, W6, W8, W1's expect reason, S3, W5)
plus one doc/record contradiction (W4). The accurate ones are listed in the reviewer's message as a
positive result, including `vendor.rs:796-852`, `validate_grim_home:1035-1085` and its explicit
"what this deliberately does NOT cover", `run.rs:390-424` `client_admits` with its P-1 self-correction,
the projector's one-table claim, `read_table`, `machine_key`'s O_EXCL adopt-the-winner, `write_payload_file`'s
"(pid, slot), no caller byte in the name", and `converge_clients`' inverted step order.

Structural notes, not filed: `converge_clients` (173) and `invoke` (183) are long but each stays one
responsibility with numbered phases; `install_one` grew 666->719, extending a pre-existing god function.
