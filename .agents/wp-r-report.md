# WP-R — arming composition: implementation report

**Status:** implemented, verified, committed on `hex/hooks-artifact-kind--wp5-r`.
**Gates:** `cargo fmt` clean · `cargo clippy --locked --all-targets -- -D warnings` clean ·
`cargo test --bin grim` **2880 passed** · `task --force verify` green (**1060 acceptance passed,
1 xfailed**, 51 AI-config passed).
**Executed proof:** a real hook artifact built, released to `localhost:5000`, declared, armed, and
**fired** — dispatch table, registration, launcher and dispatcher output all pasted in § 5.

`.agents/wp-j2-report.md` § A2 was right: nothing armed. It does now.

---

## 1. The six steps, and where each one lives

`sync_for_state`'s documented contract put all six steps in one per-client function. **Step 4
could not stay there**, and that is a defect in the merged contract rather than an implementation
convenience — see § 6 F-1. The split now is:

| Step | Where | How |
|---|---|---|
| **1. Refuse early** | `hook_registrar::converge_clients` — once per command | `arming_refusal(grim_home, workspace, scope)` covers causes 1–3 (relative `$GRIM_HOME`, `$GRIM_HOME` inside the workspace, a control character in the launcher or table path). A refusal returns `NotArmed(cause)` for **every** hook-capable client and writes nothing at all: no launcher, no table, no registration. Cause 4 (`DispatchLocked`) is raised where it is observable, at the `converge_root` call. |
| **2. Generate the launcher** | `converge_clients` → `generate_launcher` | `hook_launcher::generate(grim_home, current_exe())`, idempotent, **only when the union is non-empty** — a pure reap must not create the shim it is about to orphan. `std::env::current_exe` is read here, not inside `generate`, so that generator stays a pure function of its arguments as its own doc requires. A failure to resolve it is an `io::Error`, not a refusal: there is no `ArmRefusal` cause for "grim does not know where it lives", and inventing one would report an environment failure as a consent decision. |
| **3. Compute the desired set** | `desired_entries(vendor, state, roots, trust)`, per client | The policy is applied **structurally**: `trust = |src| policy.arms(src)`, so a gated or untrusted hook simply is not in the set. "Off" and "untrusted" reach the runtime as an absent entry, never as a runtime check the runtime is forbidden to make (C-007). `desired_entries` now returns `DesiredHook { entry, manifest, event }` — the dispatch row **and** the `HookEntry` it was projected from, so the table and the registration are derived from one read of one `hook.toml`. |
| **4. Write the dispatch entry** | `converge_clients`, **once per command, with the union over every hook-capable client** | `converge_root(grim_home, &token, root, &union)` under the advisory lock. `root_token` mints the HMAC; `root_scope_for` maps the resolved scope. Written **before** the registrations, deliberately: a registration whose table entry is missing degrades to no-match ⇒ exit 0, whereas a table entry with no registration is inert. |
| **5. Converge the client surface** | `sync_for_state`, per client | Registrations are assembled through `Vendor::hook_registration` — the single assembly site, unchanged. Then per surface: `converge_own_file` renders the whole document (codex, copilot) or deletes it when the desired set is empty; `converge_splice` upserts each marked element (claude) and then **enumerates and reaps `owned − desired`** via `json_splice::owned_nested_handlers` + `remove_nested_handler`, over **every** event member the client can spell — not only the members the desired set names, or an element under a dropped event stays armed forever. Add-strict, remove-tolerant, following `opencode_config`'s discipline. |
| **6. Git-exclude hygiene** | `sync_for_state`, claude · project only | `ensure_settings_local_excluded` / `drop_settings_local_exclude`. **Never a gate** — every outcome arms anyway. `AlreadyTracked` is the one outcome logged at `warn`, because it means the user's own arming *will* show up in `git status`; the rest are `debug`. |

**Two surfaces, two documents, and they are not interchangeable.** Three new `Vendor` methods keep
that knowledge per vendor instead of a `match vendor.name()` in the shared module (the silent-drift
shape D-1 is about):

- `hook_config_path(workspace, scope)` — the promotion `sync_for_state`'s own doc owed. The
  location beside `hook_surface`'s shape, so the driver is writable generically.
- `hook_file_document(&[HookRegistration])` — codex nests handlers in per-event **matcher groups**
  and forbids a `version` key (`HooksFile` is `deny_unknown_fields` at the top level; one unknown
  key drops **every** hook in the file); copilot takes a **flat** per-event array with the matcher
  on each entry and **requires** `version: 1`. Match-all is spelled by **omission** on both,
  because copilot rejects `*` as an invalid regex.
- `hook_splice_shape()` + `hook_spliced_handler(reg)` — claude's `hooks.<Event>[].hooks[]` keyed on
  `matcher`, with `*` as the match-all group value (Claude's own documented spelling, and
  deliberately *not* shared with the two `OwnFile` clients). The shape is split out because the
  **reap** needs the key names with no registration in hand.

**Convergence no longer rides `Vendor::sync_config`.** The three hook vendors' `sync_config`
overrides are deleted (they did nothing else); `vendor_opencode`'s stays. Reasons in § 6 F-1.

## 2. Policy down, consent up — and the proof read-only commands cannot prompt

**Policy (`src/hook/policy.rs`, new).** `HookPolicy` owns the invocation-level half of
`trust::ArmingQuery`: the feature flag, `--allow-hooks`, the interactivity classification, and the
authored `[[registries]]` of **both** config scopes each tagged with its own scope. Pure — no
terminal, no config write, no clock. It exposes `verdict(&LockedSource) -> Option<Arming>`,
`arms(...)`, and `refusal_reason(...)`. `None` means "no registry pin", which is *not* armed: a
path source has no `trust_hooks` entry that could express consent.

**It rides `InstallTarget`** (`hook_policy: Option<HookPolicy>`), attached by
`with_hook_policy` at the mutating boundary. **I contradicted the brief on one point, with
evidence:** the brief said to derive the policy *inside* `InstallTarget::parse`, "zero call-site
signature changes". That is not achievable. `parse(workspace, scope, flag_values, config_default,
vendors)` receives four projections chosen by each caller; the policy additionally needs
`options.experimental.hooks`, both scopes' `[[registries]]`, the `--allow-hooks` flag and the
invocation's interactivity. Adding them changes the signature at **all ~15 production call sites**,
three of which are the read-only commands that have no use for any of them — the opposite of zero
churn. So `parse` leaves the field `None` and the builder attaches it. The split lands in the same
place the brief wanted it and is *stronger*:

- **`grim status`, `grim search`, `grim context` hold `None`.** Hook convergence is gated on
  `Some`, so those three cannot arm, cannot reap, and — because the prompt lives above
  `InstallTarget` entirely — cannot prompt. There is no code path from a read-only command to
  `trust::prompt_for_registry`.
- **`None` is fail-safe for a forgotten mutating site**: convergence is skipped, so nothing is
  armed *and nothing already armed is reaped*. A policy defaulting to "hooks off" would instead
  silently disarm every hook the last install armed.

**Consent (`src/command/hook_consent.rs`, new).** `resolve(ctx, scope, lock, allow_hooks)` runs
once per mutating command, above the per-client loop: read both tiers → classify interactivity once
→ return early when the feature is off → prompt **once per registry** for hooks actually in *this*
lock → on acceptance take the **global** config's advisory lock and `persist_grant` → re-read the
global config and `adopt_grants`. Re-reading rather than synthesizing the entry is deliberate: the
namespaced locator is `persist_grant`'s own B5.2 rule, and a second spelling of it is how two call
sites come to disagree. `resolve_without_consent` is the shape `grim uninstall` takes — it must
converge (to keep *surviving* hooks armed) but must never ask a question in order to remove
something.

Every interactive failure degrades: an unwritable prompt, an unreadable answer, a lock it cannot
take, a grant it cannot persist, a config it cannot re-read — each leaves the policy without that
grant and the command continues (I3). A declined-or-unpersisted registry is recorded via
`record_decline`, so the reported reason is `ConsentDeclined` ("you said no") rather than
`NoTtyToAsk` ("nobody could be asked") — two states with two different remedies.

**Wiring:** `--allow-hooks` added to `install`, `update`, `add` (flag only — there is no
`GRIM_ALLOW_HOOKS`, deliberately). `install::run` and `dev_install` resolve and attach;
`update::run` resolves against the **freshly re-resolved** lock and drives convergence itself
(it calls `install_all_with_progress`, not `install_and_persist`); `add::install_added` resolves
against the single-entry projection, so the prompt names exactly the registry the user just asked
for; `uninstall::run` uses the no-prompt resolver. **The TUI is deliberately untouched** — it
refuses `ArtifactKind::Hook` on every path (WP-H's decision: "a third consent surface nobody
designed, on the keystroke path") and its alternate screen + raw mode make a stderr prompt both
illegible and unreadable. It therefore neither arms nor reaps, which is the correct hook-neutral
posture; giving it a gated policy would have silently disarmed hooks armed by `grim install`.

**Proof of the negative:** `test/tests/test_hook_arming.py::test_read_only_commands_never_prompt_for_hook_consent`
(parametrized over `status`, `search`, `context`) and
`test_read_only_commands_write_no_hook_trust_into_global_config`, both with a declared untrusted
hook and the feature flag **on** — the exact state that makes `install` prompt. Asserted on the
prompt's own strings, not on "did it hang", so a *silent* consent evaluation fails too. Plus
`install::target::tests::parse_never_attaches_a_hook_policy_so_a_read_only_command_cannot_arm_or_prompt`
at unit level. Every negative carries a positive control in the same function.

## 3. `#[expect(dead_code)]` attributes discharged

Deleted because their REMOVAL TRIGGER fired (an unfulfilled `expect` is a hard error under
`-D warnings`, so none of these could survive first use):

- `src/hook/trust.rs` — **6**: `NotArmedReason`, `GrantSource`, `arming`, `interactivity`,
  `prompt_for_registry`, `persist_grant`. **Attribute deletions only** — `git diff` on that file is
  38 lines, all `-`.
- `src/install/hook_registrar.rs` — **6**: `GIT_EXCLUDE_RELATIVE` (now used in the hygiene log
  lines, which is exactly what its doc reserves it for), `HookSync`, `desired_entries`,
  `root_scope_for`, `ensure_settings_local_excluded`, `drop_settings_local_exclude`.
- `src/install/hook_dispatch.rs` — **2**: `root_token`, `converge_root`. ⚠ **This file is on my
  do-not-touch list.** The change is two attribute deletions and nothing else; becoming their
  caller makes the expectations unfulfilled, which fails the build. Reported rather than silent.
- `src/install/hook_launcher.rs` — **1**: `generate`. I initially over-deleted three here and
  restored the two I am not the caller of (`CommandSpec`/`registered_command` remain dead — their
  production consumer is still owed, see § 6 F-3).
- `src/install/vendor.rs` — **4** stale `#[allow(dead_code)]` on `hook_surface`,
  `hook_event_name`, `hook_tier_support`, `hook_registration`, whose own reason string named
  "WP-J2, the first production caller".

`GrantSource::ConsentPrompt` and `NotArmedReason::ConsentDeclined` were never constructed by
anything. Rather than suppress them, `HookPolicy` now constructs both — a just-accepted grant
reports `ConsentPrompt` instead of `GlobalConfigEntry`, and a declined registry reports
`ConsentDeclined`. Both refinements are narrow: a decline only fires where nothing granted (so it
cannot override `trust_hooks = false` or `--allow-hooks`), and the re-label only touches an
`Armed(GlobalConfigEntry)` verdict (so `--allow-hooks` keeps reporting the flag rather than
claiming a durable grant the config does not carry).

## 4. `src/api/install_report.rs` — **withdraw S-002's second half**

**Decision: withdraw it in favour of `grim status`'s `arming[]`.** Reasons, in order of weight:

1. **The report has no shape for it.** `InstallReport` is one row per artifact
   (`Kind | Name | Target | Status`, `{"items": [...]}`), and arming is per **`(artifact, client)`**.
   Adding "on which clients" means either a nested array inside a row — which breaks
   `subsystem-cli-api.md`'s single-table plain rule — or N rows per artifact, which changes
   `items[]` cardinality on a frozen JSON surface from one-per-artifact to
   one-per-artifact-per-client. Consumers key on the former.
2. **"At which tier" is not an artifact-level fact.** Tier is per `[[hooks]]` entry, so one
   artifact can arm at several tiers on one client. There is no column for a set.
3. **Two surfaces for one fact is the drift this repo keeps paying for.** `grim status`'s
   `arming[]` already ships `{client, cause, message, transient}` per pair, with a `gated` /
   `untrusted` / `not-armed` row state and a documented precedence — designed for exactly this
   question. A second, differently-shaped answer would go stale, and § 6 F-2 shows the two would
   *already disagree* today.
4. **The actionable half already ships on the install path**: a hook that does not arm produces a
   `warn` naming the artifact and the distinguishing reason (§ 5 shows both wordings), which is
   what a user acts on. What is missing is a positive confirmation line, and the right place to
   close that additively is the surface that already reports arming.

**Recommendation instead:** give `grim status` the one thing it cannot currently see — see F-2.

## 5. Executed evidence

Real binary (`target/release/grim`), real registry (`localhost:5000`), real client config.
Scratchpad transcript: `armlab/run.sh`, `armlab/clone.sh`.

**S-001 — gated (the default) skips with a warning, exit 0:**

```
$ grim add --kind hook localhost:5000/acme/shell-guard:1
 WARN grim::install::installer: hook 'shell-guard' not installed: hooks are gated;
      enable them with `grim config set options.experimental.hooks true`
Kind  Name         Pinned                                                    Status
hook  shell-guard  localhost:5000/acme/shell-guard@sha256:6cd16e62d4d0…      added

$ grim status --format json
hook shell-guard state=gated arming=[{"client":"claude","cause":"feature-flag-off", …}]

$ ls $GRIM_HOME/hooks          → No such file or directory   (nothing armed)
$ cat ws/.claude/settings.local.json → No such file           (nothing registered)
```

**S-002 — flag on, registry untrusted, no TTY ⇒ declines; `--allow-hooks` ⇒ arms:**

```
$ grim install
 WARN hook 'shell-guard' not installed: this registry has not been trusted for hooks
      and there is no terminal to ask on; pass --allow-hooks
hook  shell-guard  …/.grimoire/hooks/shell-guard  skipped

$ grim install --allow-hooks
hook  shell-guard  …/.grimoire/hooks/shell-guard  updated
```

**The dispatch table** (`$GRIM_HOME/hooks/dispatch.json`) — one row, keyed on the arming client,
under an opaque root token:

```json
{ "schema": 1,
  "roots": { "9b91d85cad7b297b0c73f5a13dc5a07c": {
    "root": "…/armlab/ws",
    "hooks": [ { "artifact": "shell-guard", "id": "guard", "client": "claude",
                 "event": "PreToolUse", "tier": "observer", "matcher": "Bash",
                 "handler": { "command": "sh guard.sh" }, "timeout": 5,
                 "payload": "stdin", "payload_dir": "…/ws/.grimoire/hooks/shell-guard",
                 "resolved_digest": "sha256:6cd16e62d4d0…" } ] } } }
```

**Claude's registration** (`ws/.claude/settings.local.json`) — one marked element, absolute
launcher path, opaque root, no `${GRIM_HOME}`, no `exec`:

```json
{ "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ {
  "com.grimoire.managed": "hook-dispatcher",
  "type": "command", "timeout": 5,
  "command": "L='…/armlab/home/hooks/bin/grim-hook'\n[ -f \"$L\" ] && [ -x \"$L\" ] || exit 0\n\"$L\" run --client claude --event PreToolUse --table '…/armlab/home/hooks/dispatch.json' --root 9b91d85cad7b297b0c73f5a13dc5a07c\ns=$?\ncase \"$s\" in 0) exit 0 ;; *) exit 0 ;; esac"
} ] } ] } }
```

**The launcher** — `0755`, absolute recorded binary, no `$PATH` fallback:

```
-rwxr-xr-x 1 mherwig mherwig 219 grim-hook
#!/bin/sh
# generated by grim — do not edit; `grim install` regenerates it
G='…/target/release/grim'
[ -f "$G" ] && [ -x "$G" ] || exit 0
exec "$G" hook "$@"
```

**Self-heal (Principle 9)** — a second `grim install --allow-hooks` reports `unchanged` and both
files are byte-identical:

```
7a2056fa…  home/hooks/dispatch.json      498482d4…  ws/.claude/settings.local.json
hook  shell-guard  …  unchanged
7a2056fa…  home/hooks/dispatch.json      498482d4…  ws/.claude/settings.local.json
```

**It fires.** The registered command, run as a client would:

```
$ printf '{"…","tool_name":"Bash","tool_input":{"command":"ls"}}' \
    | $GRIM_HOME/hooks/bin/grim-hook run --client claude --event PreToolUse \
        --table $GRIM_HOME/hooks/dispatch.json --root 9b91d85c…
 WARN grim::command::hook::pipeline: shell-guard/guard is declared `observer`, so its
      `allow` verdict was ignored — only a `gatekeeper` may return a verdict …
dispatcher exit=0
```

That warning is the proof: the matcher matched, the payload was spawned with the envelope on stdin,
and its response was projected and tier-clamped. Exit 0, so no client is blocked.

**Audit trail.** `$GRIM_HOME/hooks/hook_audit.jsonl` **exists but is empty** after an `observer`
whose verdict was clamped — consistent with WP-K's G-5 (declines write `NoMatch` records; a clamped
observer writes none). Not a WP-R surface; recorded because the brief asked for the contents.

**S-008 — uninstall reaps both:**

```
$ grim uninstall hook shell-guard
hook  shell-guard  uninstalled
$ cat $GRIM_HOME/hooks/dispatch.json   → { "schema": 1, "roots": {} }
$ cat ws/.claude/settings.local.json   → {}      (the marked element gone)
```

At unit level the same cycle is pinned by
`hook_registrar::tests::a_recorded_hook_arms_a_dispatch_row_and_a_registration_then_reaps_both`,
plus `turning_the_feature_flag_off_disarms` and
`a_user_authored_hook_in_the_same_config_survives_arming_and_reaping` (the user's document comes
back byte-equal after grim's element is reaped).

---

## 6. What I found **wrong**

### ⛔ SEC-1 — Block, found by execution: a cloned repository's own committed `.grimoire/` arms, offline

**Attacker T3, invariant I2.** A workspace carrying its own `.grimoire/state.json` + hook payload
arms on a fresh machine with **no network fetch and no local install history**, as soon as the
victim's *global* config trusts the registry the **committed record names**.

Reproduced (`armlab/clone.sh`): donor workspace armed normally; `.grimoire/` copied to a fresh
`clone/`; a fresh `$GRIM_HOME` given a global `trust_hooks = true` for `localhost:5000/acme`;
then, with `GRIM_OFFLINE=1`:

```
$ GRIM_OFFLINE=1 grim install
hook  shell-guard  …/clone/.grimoire/hooks/shell-guard  unchanged
$ cat victim-home/hooks/dispatch.json
{ "roots": { "135020f4c06b6ee19d054fd36b80ec9d": { "root": "…/clone",
    "hooks": [ { "artifact": "shell-guard", "client": "claude",
      "payload_dir": "…/clone/.grimoire/hooks/shell-guard", … } ] } } }
$ cat clone/.claude/settings.local.json   → the marked registration, written
```

**The chain.** `.grimoire/state.json` and the payload are both repo-resident and both committable.
`install_one`'s `integrity_gate` compares the *recorded* hash against the on-disk payload — the
attacker controls both, so they match and it short-circuits to `AlreadyInstalled` without fetching.
Convergence then reads `hook.toml` from the record's own `payload_dir` and arms it. The
`resolved_digest` is never re-verified against the payload bytes — `desired_entries`' own doc says
it is "provenance rather than a gate" (W4). The project `grimoire.toml` can also set
`[options.experimental] hooks = true` for itself, so the feature flag is not a barrier here.

**Not N1** (no commit access needed — the victim cloned) and **not N4** (no gate was shown). It is
exactly I2's "approved the right thing, checked at the wrong time", reached through a repo-resident
record. The **only** control that fires is B4's global-only grant rule, and it is keyed on a
registry name the attacker chose.

**I did not fix it**, and that is a scope judgement rather than an omission. Every candidate is a
decision above this WP:

1. **Move hook payloads out of the workspace** (e.g. `$GRIM_HOME/hooks/payload/<root>/<name>/`) and
   have convergence derive the payload dir from `$GRIM_HOME` rather than from the record. Sound and
   structural — it makes I1 cover the manifest grim arms from — but it is a **layout change** on a
   surface WP-J2 shipped, and S-003 pins the payload at `<scope>/hooks/<name>/`.
2. **Refuse to arm from a record grim did not write in this process** at project scope. Cheap, but
   it breaks the ordinary "install once, converge on every later command" flow.
3. **Re-verify the payload against the registry layer digest before arming.** Correct, but it costs
   a network round trip on every install and cannot work offline.

Hashing the payload against `record.content_hash` does **not** help: the attacker supplies that
hash. Pinned as a **strict xfail** —
`test_hook_arming.py::test_a_cloned_workspaces_own_committed_hook_state_must_not_arm` — so the
exposure is visible in CI, cannot be closed silently, and turns green the moment it is fixed.
Owner call needed.

### ⛔ F-1 — `sync_for_state`'s documented step order is wrong, and following it corrupts the table

The merged doc puts step 4 (write the dispatch entry) inside the **per-client** function.
`converge_root` replaces a root's `hooks` vector **wholesale**, while `desired_entries` is **per
vendor** — so calling the first with the second's output once per client makes the last client's
write **erase every earlier client's rows**. The F-1 field (`DispatchEntry::client`) is necessary
for the union to be unambiguous but is not by itself sufficient; nothing about a per-row client name
prevents a wholesale overwrite.

Corrected: step 4 moved up into `converge_clients`, called **once** with the union over every
hook-capable client, sorted on `(artifact, id, event, client)` so a no-change rewrite is
byte-identical. Pinned by
`the_dispatch_union_carries_every_arming_client_deterministically`, which converges the clients in
the *reverse* order and asserts both rows survive.

The same finding kills the "hook convergence rides `Vendor::sync_config`" design for a second
reason: that seam is called with the **clients this command happened to touch** (the removed
record's clients in `uninstall`, the union of target ∪ pruned ∪ reaped in `update`). A set narrowed
that way drops a sibling client's rows from the union. `converge_clients` therefore derives its own
client set from the hook-capable roster and ignores the caller's — which also makes every command
idempotent and self-healing regardless of what it was asked to do.

### ⛔ F-2 — `grim status` cannot see an `--allow-hooks` arming, and reports it as `gated`

Executed, immediately after a successful arming:

```
$ grim status --format json
hook shell-guard state=gated arming=[{"client":"claude","cause":"registry-not-trusted", …}]
```

The hook **is** armed and **does** fire (§ 5). `status::hook_arming` derives its verdict from the
config alone (`trust::decide` + `hooks_enabled`), and `--allow-hooks` is per-invocation and
deliberately never persisted, so status has nothing to read. It errs toward under-claiming, which is
the safe direction for a guardrail — but "gated" while a guardrail is live is still misleading, and
it is the state every CI user will be in.

**Recommended fix (not mine — `src/command/status.rs` is WP-H's):** let `hook_arming` read the
**dispatch table**, which is the machine-local arming authority and is already derived rather than
recorded. A row present for this `(root, client, artifact, id)` **is** armed, whatever the config
says. That keeps one surface and makes it complete — and it is why § 4 withdraws the install-report
half rather than adding a second surface that would disagree with this one.

### ⛔ F-3 — `config set options.experimental.hooks false` is refused, so the flag has no route back

`config.rs::refuse_disarm_via_config` refuses both `set … false` and `unset` (exit 65) with
*"run `grim install` to disarm"*. Its own doc already flags this as an **open owner decision
(review W6)**: a `true` on disk has no CLI route back to `false` or absent, and `grim install` does
not write config.

WP-R changes the stakes rather than the code: convergence now exists, and `grim install` **does**
disarm correctly once the flag is off (`turning_the_feature_flag_off_disarms`, and the acceptance
test that edits the file). So the refusal's premise is now satisfiable — the missing half is only
permission to clear the flag. I left the refusal untouched (it is explicitly owner-deferred) and the
acceptance test edits `grimoire.toml` directly, with a comment saying why. **One verb should gain
permission to write and converge, or the message should name a route that exists.**

### The three mid-flight items from the orchestrator — resolved

**1. The stale `sync_for_state` fall-through comment (Block) — already discharged by this
commit, not fixed separately.** The relayed Block is real about the tree it was read from and
the reasoning is right: `locate_canonical` handles `ArtifactKind::Hook` explicitly
(`installer.rs:2499` and `:2517`), so a hook record *is* producible by a shipped seam and the
comment's "no shipped seam can produce either" was falsified by WP-J2's own install branch.
Both the comment and the unconditional `Err(unsupported_kind())` fall-through were deleted
wholesale when `sync_for_state`'s body was written — `rg 'No shipped seam can produce'
src/install/hook_registrar.rs` returns nothing.

Verified by execution that the user-visible symptom is gone. On a legitimately installed,
armed hook:

```
$ grim install --allow-hooks 2>&1 | grep -c "not armed"   → 0
$ grim update  --allow-hooks 2>&1 | grep -c "not armed"   → 0
$ grim uninstall hook shell-guard 2>&1 | grep -c "not armed" → 0
```

(The one place that line still appears is a genuine C-017 refusal — running with a *relative*
`GRIM_HOME` produces `hooks not armed for claude: GRIM_HOME is a relative path, so the dispatch
table would resolve inside the workspace`, which is cause 1 doing its job.)

**The hand-edited-`state.json` refusal is deliberately gone, and that is the right call.** It
existed *because* no legitimate record could reach it. Now that one can, a record in
`state.json` is treated as legitimate — and the control that stops a *forged* one from arming is
the trust gate, not a blanket refusal: a project config cannot grant, so a hostile record needs a
**global** grant naming the registry it claims. That is the same gate SEC-1 above shows is the only
one standing, which is why SEC-1 is filed rather than closed by re-adding a refusal that would
also have refused every real hook.

**2. `--allow-hooks` — DECISION: shipped (option 1), and it is already in this commit.** The
orchestrator's correction that the flag did not exist was accurate before this change; it now
exists on `install`, `update` and `add`, wired to `ArmingQuery::allow_hooks`, and every message
that names it is followable. Why ship rather than drop:

- **C-023 has no other answer.** "No TTY never asks" plus "two config gates" leaves a CI user who
  *wants* hooks with no route at all except hand-writing a global `[[registries]]` entry into the
  runner's home — which is strictly worse security theatre than one audited flag, because it
  persists a durable grant to get a one-shot effect.
- **The prompt text already names it** (`trust.rs:611`, merged, not mine to change): *"Non-interactive
  runs never ask — pass --allow-hooks."* Shipping neither would have left a **merged, user-facing
  prompt naming a flag that does not exist** — precisely the ⛔ the orchestrator warned against,
  reached from the other direction.
- **N4 is satisfied, not violated.** A user bypassing a gate they were shown is an explicit
  non-goal; the obligation is an honest, legible prompt, and it is unchanged.

**The env-var question, answered: `GRIM_ALLOW_HOOKS` must not exist, and does not.**
`rg -i 'GRIM_ALLOW_HOOKS' src/ AGENTS.md docs/` finds it only in `src/hook.rs`'s and
`declaration.rs`'s prose *stating that it was deleted* (owner decision 2026-08-17, `24a14bb`,
withdrawing C-026). `AGENTS.md`'s env table does not list it. Nothing in this commit adds an
environment form: the flag is clap-only, deliberately, because the environment is routinely
repo-carried and a repository must never grant itself trust (B6). **Owed to WP-M, and it is a
doc-only row:** `docs/src/configuration.md` + the `grim install`/`update`/`add` rows in
`subsystem-cli-commands.md` need the flag documented as a **per-invocation** escape that does not
turn the feature on, with an explicit "there is no environment form".

**Coordination consequence the orchestrator should route:** WP-N removed the third gate from the
catalog on the evidence that it did not exist. There are now **three** gates again — feature flag,
per-registry `trust_hooks`, `--allow-hooks` — so the catalog rows WP-N corrected need the third one
restored.

**3. Hooks as bundle members — the three consent requirements, all met, none needing a
bundle-side file.**

- **The trust predicate keys on the member's own `LockedSource`.** Already structural:
  `desired_entries` evaluates `trust(&record.source)` per install record, and
  `hook_consent::resolve` groups by `artifact.source.pinned().registry()` — the *member's* pin, not
  a container's. A bundle from registry A carrying a member pinned to registry B therefore needs a
  grant for **B**. Pinned by the new
  `the_trust_predicate_is_evaluated_per_record_not_once_per_install`, which puts two hook records
  from two registries in one state and asserts only the trusted one enters the desired set — so a
  refactor hoisting the predicate out of the record loop fails.
- **A bundle-delivered hook is not invisible.** `hook_consent::resolve` now emits one `warn` line
  per bundle-delivered hook, naming the bundle and the member's registry, **before** any prompt and
  **regardless of verdict** — a bundle carrying a hook is worth knowing even when the registry is
  already trusted. Deliberately disclosure, not a second consent surface: the prompt still names
  the registry only (S-002 post-reversal), because per-hook prompting is the re-prompt habituation
  the owner reversed D5 to avoid. It reads `LockedArtifact::bundles`, which is empty today and
  populates itself the moment the bundle-side WP lands — no coupling to that WP's files.
- **The arming is gated, not the payload — and a bundle install is not a partial failure.** The
  S-001 skip is the *declined-vendor* shape, not a refusal: `InstallOutcome::Skipped`, a warning, and
  **exit 0**. A bundle of five skills and one gated hook installs the five and reports the hook
  `skipped`. It is a *skip* rather than materialize-then-leave-inert because S-003 puts the payload
  on disk for an approved install and the plan names
  `test_declined_*_vendor_warns_skips_and_uninstalls_clean` as S-001's template. **If the owner
  would rather a gated hook materialize its payload and simply not arm, that is a one-line change
  in `install_one` and I will make it** — flagging the tension rather than assuming.

**Not acted on, as instructed:** `src/command/publish.rs:1867`'s false "Hooks are bundle members
too" comment (the orchestrator is fixing it), and the bundle parser / wire format / member
expansion (its own WP).

### F-5 — `trust_hooks` is invisible in every report surface but one — **route to WP-M**

Reproduced exactly as WP-N described, by execution:

```
$ grim config get registry.acme.trust_hooks      → true
$ grim config registry show acme --format json   → {alias, oci, index, include, exclude, default, insecure}
$ grim context --format json  .registries[]      → {alias, url, kind, default, authenticated, include, exclude, insecure}
```

So a consent-bearing field is auditable only through `config get <exact key>`, while the analogous
per-registry **security** field `insecure` is surfaced in both report shapes. That asymmetry is
worth closing.

**It is not mine, and the reason is the frozen surfaces rather than the file boundary.** Both
additions land in report modules outside this WP (`src/api/context_report.rs`, and `config.rs`'s
registry report), and both are frozen JSON contracts: `context`'s `registries[]` needs the
always-present-null discipline, and `registry show`/`list`/`fields` are driven by
`RegistryField::ALL`, whose six names are documented **append-only with frozen positions**.
Appending a seventh addressable field is a deliberate CLI-surface extension that needs its own
acceptance tests in `test_config_registry.py` and `test_context.py` plus a docs row — exactly WP-M's
territory, and not something to smuggle into an arming commit.

**What I did instead**, so the one working route cannot regress silently:
`test_trust_hooks_round_trips_through_config_get` pins that `config get` reads the tri-state back
and that **absent stays absent** (exit 1) rather than reading as `false` — the property that stops a
later `grim add` from dropping an authored opt-out (B7).

### F-4 — smaller, recorded not fixed

- **`hook_launcher::registered_command` / `CommandSpec` / `registered_command_powershell` are still
  dead.** `Vendor::hook_registration` generates the same string from its own
  `registration_command`, which is the duplication that module's doc warns about ("two generators
  for a string a client executes"). The PowerShell half is the *only* generator of the Windows form
  and has no consumer, so `command_windows` is `None` on every registration — an owed Windows gap
  for codex (`commandWindows`) and copilot (`powershell`), already noted in `vendor.rs`.
- **`grim install --allow-hooks` on a previously-skipped hook reports `updated`, not `installed`.**
  The gated skip records a zero-output record, so the later real install sees a prior record.
  Defensible, mildly misleading; cosmetic, human-readable text only.
- **`write_config` in `test/src/helpers.py`** gained optional `hooks=` and `options=` kwargs
  (emitted only when non-empty, so every existing caller's config is byte-identical). Needed because
  no acceptance helper could declare a `[hooks]` table.

## 7. Files touched, and the overrun I am declaring

**In my declared set:** `src/install/target.rs`, `src/install/hook_registrar.rs`,
`src/command/{install,update,add}.rs`, `test/tests/test_hook_arming.py` (new).
**New:** `src/hook/policy.rs`, `src/command/hook_consent.rs` (+ `src/hook.rs`, `src/command.rs`
module registrations).
**`src/hook/trust.rs`:** attribute deletions only, as instructed.
**`src/tui/app.rs`: not touched** — see § 2.

**Overrun, declared rather than silently widened** (no other WP claims any of these; the concurrent
packages are wp5-n on `catalog/**` and the bench worker on `taskfiles/**`):

| File | Why the convention left no alternative |
|---|---|
| `src/install/vendor.rs` | The three new trait methods + `HookSpliceShape`/`SplicedHandler`. Per-vendor file shapes cannot live in a shared module without a `match vendor.name()`, which is the drift D-1 names; and `hook_config_path` is the promotion `sync_for_state`'s doc explicitly owed. |
| `src/install/vendor_{claude,codex,copilot}.rs` | The per-vendor implementations, plus deleting the three now-empty `sync_config` overrides. **Net reduction** in two of the three. |
| `src/install/installer.rs` | S-001's skip gate must fire **before any blob is fetched**, which is inside `install_one`; and `install_and_persist` is where the convergence pass belongs (it already owns the persist + sync steps for `install` and `add`). |
| `src/command/uninstall.rs` | S-008's deregistration half. `uninstall` bypasses `install_and_persist`, so it drives convergence itself — with the no-prompt resolver, since a removal must never ask a question. |
| `src/install/hook_dispatch.rs` | **On the do-not-touch list.** Two `#[expect(dead_code)]` deletions, zero logic — becoming their caller makes the expectations unfulfilled, which fails `-D warnings`. |
| `src/install/hook_launcher.rs` | One `#[expect(dead_code)]` deletion, same reason. |
| `test/src/helpers.py` | `write_config` could not declare a `[hooks]` table (§ F-4). |

## 8. Tests added

**Unit (`cargo test --bin grim`, 2880 total):**
`src/hook/policy.rs` — 8 tests: the flag answered before trust, project-may-restrict-never-grant,
no-TTY declines and names the flag, `ConsentRequired` is not `Armed`, `adopt_grants` replaces only
the global tier, a path source never arms even under `--allow-hooks`, bare-host and `index` entries
never grant, one distinct message per reason.
`src/install/hook_registrar.rs` — the arm-then-reap end-to-end proof, flag-off disarms, the user's
own hook survives, refuse-early writes nothing, the no-op fast path writes nothing at all, the
`owned − desired` reap with no record, `OwnFile` ownership is the path, the union over both clients.
`src/install/target.rs` — the two policy-absence negatives.

**Acceptance (`test/tests/test_hook_arming.py`, 14 passed + 1 strict xfail):** S-001 (both halves),
S-002 (no-TTY and `--allow-hooks`), global-grant-arms / project-grant-does-not, project
`trust_hooks = false` beats a global grant, self-heal, S-008 with a user-authored sibling hook,
flag-off disarms, S-007's post-reversal form on `grim update`, the three read-only-never-prompt
negatives ×2, and SEC-1 as a strict xfail. **Every negative carries a positive control in the same
function** — a build where nothing arms satisfies every "nothing was armed" assertion, which is the
state four waves shipped in.
