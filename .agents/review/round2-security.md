# Round 2 — security

Adversarial security review of `git diff 01ce10f..HEAD` (branch `hex/hooks-artifact-kind`,
nine commits fixing round-1 Blocks). Every finding is bound to
`.claude/rules/arch-threat-model.md`: attackers **T1–T5**, non-goals **N1–N5**,
invariants **I1–I6**.

Evidence was produced with `target/release/grim` (built 2026-08-18 17:10) against the
manual-rig registry on `localhost:5050`, isolated `GRIM_HOME`s under
`/tmp/claude-1000/…/scratchpad/sec{2,3,4}`. No file in the repo was modified except this
report.

## Verdict summary

| # | Severity | What | Attacker | Invariant |
|---|---|---|---|---|
| S2-1 | **Block** | `root-key` is a valid binding name, so a hook bound as it materializes a **directory** over the machine-local HMAC key path and **permanently disarms every global hook on the machine** | T1 (bundle publisher picks the member's binding name) | I3, I5 |
| S2-2 | **Warn** | `decide`'s condition 5 reads the *authored* `insecure` flag, so `GRIM_INSECURE_REGISTRIES` (repo-carryable) makes a plain-HTTP fetch arm with **no prompt** — W8's control bypassed from the environment; the new test module pins the wrong half of it | T3/T4 to set the env + the W8 network attacker (T2's class) | I4 |
| S2-3 | **Warn** | `reap_dead_roots` reaps on `Path::exists()`, which is false for *inaccessible* as well as *absent* — an unmounted/unreadable workspace silently disarms that project's guardrails, permanently (no re-arm without a re-install) | none / T5 in a shared-checkout case | I3, I5 |
| — | not a defect | `expand_payload_dir` (item 2), `matches_tool`'s `\|` split (item 3), `binding_name_refusal`'s parse gate against traversal (item 5) | — | — |

Items 2, 3 and 5 of the brief were attacked and **held** — see “Attacked and held” below,
including the exploit attempts that failed and why.

---

## S2-1 — Block: a hook bound as `root-key` disarms every global hook, permanently

**File:** `src/oci/hook.rs:154` (`RESERVED_ARTIFACT_NAMES`), reached through
`src/oci/hook.rs:219` (`binding_name_refusal`) — the round-2 fix — and
`src/install/hook_dispatch.rs:265` (`payload_dir`) / `:103` (`ROOT_KEY_FILE`).

**Attacker:** **T1**. `src/command/add.rs:229-236` states it in its own words: *“a bundle
member's binding name is picked by the bundle and never passes through `grim add` at
all”*, and names `installer::install_one` as the boundary. That boundary is
`binding_name_refusal`, and it accepts `root-key`. Also reachable by ordinary user error
(`grim add --global … --name root-key`, `[hooks] root-key = …` in global config).

**Invariants:** **I3** (grim degrades to “the feature is off” — here it degrades to *every*
guardrail off, machine-wide, from one artifact) and **I5** (the control stops existing with
only a `WARN` line as the trace). This is exactly the class the reserved list was created
for: `is_reserved_binding_name`'s own doc says of `bin` that *“every armed hook on the
machine, for every client and every workspace, silently stops firing”*. `root-key` is a
fourth grim-owned entry under `$GRIM_HOME/hooks/` and it was left off the list — the
refusal message even enumerates the namespace as `{bin,dispatch.json,payload}`, which is
now an incomplete statement of grim's own layout (`AGENTS.md` correctly lists three things
there: the table, the launcher, **and the root-key map**).

**Why the round-2 fix does not catch it:** `root-key` is a *valid* `SkillName`
(`[a-z0-9]+([.-][a-z0-9]+)*`, `src/skill/skill_name.rs:47`), so the new parse gate passes
it, and `is_reserved_binding_name` is exact equality against three literals that do not
include it. `payload_dir` then joins it: global scope is `hooks/<name>`
(`hook_dispatch.rs:206`), i.e. **`$GRIM_HOME/hooks/root-key`** — byte-for-byte
`root_key_path()` (`hook_dispatch.rs:161`).

### Reproduction (executed)

Fresh `GRIM_HOME` (a first install — the machine key does not exist yet), global config
declaring a hook bound as `root-key` plus a real gatekeeper guardrail:

```toml
[options]
default_registry = "localhost:5050"
clients = ["claude"]
[options.experimental]
hooks = true
[[registries]]
alias = "rig"
oci = "localhost:5050/grimoire"
trust_hooks = true
[hooks]
root-key = "localhost:5050/grimoire/hooks/tool-logger:1"
write-guard = "localhost:5050/grimoire/hooks/write-guard:1"
```

```
$ grim --global lock
Kind  Name      Pinned                                                             Action
hook  root-key  localhost:5050/grimoire/hooks/tool-logger@sha256:1adbdff43eacc…    locked

$ grim --global install
WARN grim::install::hook_registrar: hook root token could not be derived; nothing armed error=Is a directory (os error 21)
Kind  Name         Target                                    Status     Armed
hook  root-key     …/sec2/home/hooks/root-key                unchanged  —
hook  write-guard  …/sec2/home/hooks/write-guard             installed  —

$ ls -la …/sec2/home/hooks
drwxr-xr-x 2 mherwig mherwig  80 root-key        <-- a DIRECTORY where the HMAC key belongs
drwxr-xr-x 2 mherwig mherwig  80 write-guard
$ ls -la …/sec2/home/hooks/dispatch.json
ls: cannot access '…/hooks/dispatch.json': No such file or directory
```

`write-guard` is a `gatekeeper` and reports **`installed`**. There is no dispatch table at
all, so nothing fires — for any client, any workspace, forever, because
`machine_key` → `read_root_key` (`hook_dispatch.rs:482`) hits `EISDIR` on every subsequent
run too. `getrandom`-minting cannot recover: the create path is `create_new` on the same
path.

**Established-machine variant.** With the key already present the untracked-clobber gate
catches it (`grim install` refuses: *“destination … already exists for client claude and was
not created by grim; rerun with --force”*), and `write-guard` stays armed — verified, the
dispatch row survived. Under `--force` the key file is **destroyed** (verified: `600 regular
file` → `755 directory`), after which no root can ever be re-derived; that path is **N4**
(the user was shown a legible gate naming the exact path), so the finding is the
fresh-machine path, which needs no flag and shows no gate.

### Minimal fix

Add `root-key` to `RESERVED_ARTIFACT_NAMES` (`src/oci/hook.rs:154`, array length 3 → 4) and
update the refusal message's `{bin,dispatch.json,payload}` enumeration to include it. Both
call sites (`installer.rs:457`, `hook_registrar.rs:1213`) then refuse before materialization
and before arming, with no further change.

Two things worth doing in the same change, since the list is now provably derivable rather
than enumerable:

- Derive the reserved set from the constants it is about (`ROOT_KEY_FILE`, `DISPATCH_FILE`,
  `PAYLOAD_DIR`, `hook_launcher::LAUNCHER_DIR`) instead of restating three literals, so the
  next file grim puts under `hooks/` cannot be forgotten. The current miss is precisely a
  literal list drifting from the layout it guards.
- `hook_audit.jsonl` and `hook_audit.jsonl.1` are safe **only incidentally** — the
  underscore makes them unrepresentable as a `SkillName`. The transient envelope files
  `payload-<pid>-<slot>.json` (`pipeline.rs:1034`) *are* representable and live directly in
  `hooks/`; a binding-name collision there costs one hook one firing, which is not worth a
  finding on its own but is the same class. Nesting the global payload tree under
  `hooks/payload/global/` (the project scope already nests) would dissolve the whole class
  — but that is a layout move under Principle 9, so the reserved-name fix is the right
  round-2 answer.

### Test the fix needs

The new `a_binding_name_that_is_not_a_plain_name_is_refused` test
(`src/oci/hook.rs:1598`) asserts the reserved names refuse *for the reserved reason* by
iterating `RESERVED_ARTIFACT_NAMES` itself — so it is vacuous for a name missing from the
array and passed while this hole was open. Add `root-key` as a **literal** in the refusal
list (not via the constant), so the test fails if the entry is ever dropped from the array.

---

## S2-2 — Warn: `GRIM_INSECURE_REGISTRIES` defeats condition 5's plain-HTTP rule

**File:** `src/hook/trust.rs:327` (`decide`'s condition 5), fed by
`src/hook/policy.rs:169` (`insecure: entry.insecure`) and `src/command/status.rs:886`.

**Attacker:** **T3/T4** to place the environment variable (`.envrc`, `.mise.toml`,
devcontainer `containerEnv`, CI `variables:` — the repo-carried-environment argument grim
itself makes when it deletes `GRIM_ALLOW_HOOKS`, `trust.rs:262-266`), plus the network
attacker W8 exists for (**T2**'s substitution class). **Invariant I4** — default-deny for
anything that executes.

Condition 5 asks *“did the entry declare `insecure = true`?”*. The question that matters is
*“will this fetch go over plain HTTP?”*, and the two diverge, because
`plain_http_hosts_with` (`src/oci/access/registry_client.rs:146-158`) **unions**
`GRIM_INSECURE_REGISTRIES` for **any** host, with no `[[registries]]` entry involved:

```rust
for r in extra.iter().cloned().chain(insecure_registries()) { … }   // registry_client.rs:153
```

So a victim with a perfectly ordinary global entry —

```toml
[[registries]]
oci = "registry.example.com/acme"      # no `insecure`
```

— gets the **implicit** grant (namespaced, global, `Oci`, `insecure == false`), while a
cloned repository's `.envrc` carrying `GRIM_INSECURE_REGISTRIES=registry.example.com`
moves the whole fetch to plain HTTP. W8's stated reasoning then applies in full: *“the
first resolution that produces the digest pin is itself attacker-influenceable on the
wire, so the pin cannot rescue it”* — and the hook arms with **no prompt**.

### Evidence

The executed half — an entry with **no** `insecure` flag, transport forced to plain HTTP
purely by the environment variable, arming a `gatekeeper` non-interactively (`< /dev/null`,
so no prompt was possible):

```
$ GRIM_INSECURE_REGISTRIES=localhost:5050 grim --global install < /dev/null
Kind  Name         Target                          Status     Armed
hook  write-guard  …/sec4/home/hooks/write-guard   installed  claude (gatekeeper)
$ jq -r '.roots[].hooks[].artifact' …/sec4/home/hooks/dispatch.json
write-guard
```

The traced half — with `entry.insecure == false`, condition 5's clause
`(!entry.insecure || explicit_grant || is_loopback(entry.locator))` short-circuits on the
first term, so `is_loopback` is never consulted and the host is irrelevant. The loopback
host above is therefore standing in for a routable one; the decision path is identical, and
that identity is the finding.

### Where the new tests pin the wrong half

`only_a_loopback_host_is_exempt_from_the_insecure_rule_b3` (`trust.rs:835`) closes with:

> *“the `evil.dev:5000` row above is also what pins the env-var independence this
> function's doc promises: `is_loopback` is pure over its argument, so
> `GRIM_INSECURE_REGISTRIES` cannot widen the exemption whatever it holds.”*

That claim is **true of the exemption and false of the control**. The env var does not
widen `is_loopback`; it makes condition 5's *predicate* wrong one level up, and the test
module reads as though the env var has been ruled out of the trust decision entirely. The
comment is the most load-bearing sentence in the module and it currently certifies a
property narrower than the one it sounds like.

Directly answering the brief's question — **is the `is_loopback` exemption safe?** As
written, for the attacker it names (off-machine wire substitution), yes: loopback has no
network position. It is *not* safe against **T5**, which the exemption's rationale does not
mention: on a shared machine another unprivileged user can bind an unused
`127.0.0.1:<port>` and serve artifacts, and an `insecure` loopback entry then arms code
from them with no prompt. That is a narrower, more contrived case than the env-var bypass
above (it needs the victim to have authored an `insecure = true` loopback entry), so I file
it as part of this finding rather than separately — but the exemption's doc should name T5
and say it is accepted, rather than resting on “no network position”, which answers only
T2. The `[::1]` asymmetry is the tell: `127.0.0.1` is exempt and `[::1]` is not, and there
is no security difference between them.

### Minimal fix

At the two `AuthoredRegistry` construction sites, populate `insecure` from the **effective
transport** rather than the authored flag:

```rust
// src/hook/policy.rs:169
insecure: entry.insecure || host_is_plain_http(locator),
```

where `host_is_plain_http` tests the locator's host against
`plain_http_hosts_with(&config_insecure_hosts)`. This keeps `trust.rs` pure (the
environment is still read at the command boundary, not inside `decide`), keeps
`is_loopback`'s deliberate env-independence intact, and does **not** break the acceptance
suite or the manual rig: their hosts are loopback, so condition 5's `is_loopback` arm still
grants. Then fix the test comment to say what it actually pins.

---

## S2-3 — Warn: `reap_dead_roots` cannot tell “gone” from “not visible from here”

**File:** `src/install/hook_dispatch.rs:835-843` (`reap_dead_roots`).

**Attacker:** none in the ordinary case (a reliability defect in a security control);
**T5** in a shared-checkout case. **Invariants I3, I5** — and the principle the hook
subsystem states about itself in `trust.rs:70-74`: *“a silently-absent guardrail that a user
believes is enforcing is worse than no guardrail.”*

The predicate is:

```rust
token == keep || entry.root == "global" || Path::new(&entry.root).exists()
```

`Path::exists()` is `fs::metadata(self).is_ok()`, so it answers **false for every error**,
not only `NotFound`: `EACCES` on any ancestor, `ENOTCONN`/`ESTALE` on a dropped network
mount, `EIO`. The doc argues this away —

> *“A root that is merely unreachable (an unmounted share) is reaped and re-armed by the
> next `grim install` in it; a client cannot run in a workspace it cannot see, so the
> arming was already inert.”*

— and the argument holds only while the path stays invisible. It does not hold for a
**transiently** invisible one, which is the common case for exactly the setups grim runs in:
a project on an SMB/NFS share, a WSL cross-distro mount (this very repo lives under
`/mnt/wsl/share/dev/…`), a removable volume, a devcontainer bind. Sequence, all
single-user, no attacker:

1. Project `P` on a network mount has an armed `gatekeeper`.
2. The mount drops. The user runs `grim install` in *any other* project, or globally — the
   reap fires on **every** converge, reaps `P`'s root, and logs one `info!` line carrying a
   **count and no path**.
3. The mount returns. The client runs in `P`. The guardrail is gone and nothing re-arms it:
   the reap's own recovery story is “the next `grim install` in it”, which a developer
   opening an editor in a working checkout has no reason to run.

Same shape, no attacker needed, for `$GRIM_HOME` on a shared/roamed home directory (the
default is `~/.grimoire`, which is not machine-local on an NFS home): host A's converge
reaps host B's workspace roots because those paths do not exist on A, and the root key is in
the same shared directory so the tokens collide rather than diverge.

The **T5** variant: a checkout under another user's tree (`/home/alice/shared-proj`, a
layout that exists on build boxes). Alice `chmod 700 ~` → Bob's `exists()` is false → Bob's
guardrails for that project are reaped on his next converge. Alice needs no privilege over
Bob and Bob gets a count with no path.

**Not findings, checked:** the workspace path in the table is **not** attacker-influenced
(`entry.root` is written by `converge_root` from the grim-resolved scope, not from any
record or repository file, and the table lives at `0o600` inside a `0o700` directory —
verified `stat`: `700 hooks`, `600 dispatch.json`, `600 root-key`, `700 hooks/bin`). Nor can
a repository make the reap **skip**: it cannot write the table, and a root that never reaps
is only a leak against the byte cap, which the same commit's warning now covers. The
lock/atomic-write folding is correct — one lock, one write, no window for a concurrent
install to re-add what this one dropped.

### Minimal fix

Reap only on a definite absence:

```rust
let gone = matches!(
    std::fs::symlink_metadata(&entry.root),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound
);
token == keep || entry.root == "global" || !gone
```

`symlink_metadata` also stops a workspace symlink whose *target* vanished from reading as a
live root, and every non-`NotFound` error now retains the entry — which is the fail-safe
direction for a guardrail. Worth adding to the `info!` line: the reaped `root` paths, not
just the count, since “which project just lost its hooks” is the only actionable part.

The new test `a_dispatch_root_is_reaped_only_once_its_workspace_is_gone`
(`hook_dispatch.rs:1330`) proves the `NotFound` leg only (it `drop`s a `TempDir`), so it
passes unchanged under the fix and does not currently pin the wrong behaviour — it is
simply silent on the inaccessible case. Add a third leg: a workspace made unreadable
(`chmod 000` on its parent, `#[cfg(unix)]`) must **not** be reaped.

---

## Attacked and held

Recorded so a later round does not re-spend the effort.

**Item 5 — `binding_name_refusal` against traversal (`src/oci/hook.rs:219`).** The parse
gate is the right shape and I could not defeat it. `SkillName::parse`
(`src/skill/skill_name.rs:47`) admits only ASCII `[a-z0-9]` runs joined by single `-`/`.`,
1–64 chars, no leading/trailing/consecutive separator. Every form I tried is
unrepresentable rather than enumerated: unicode lookalikes and any non-ASCII (charset
check), `C:` and `\` (uppercase + disallowed chars), URL-encoded `%2e%2e` (`%`), `.`/`..`
and `.hidden` (separator edge rules), `>64` chars (length), `""` (empty). Case-folding
tricks are dead because uppercase is refused outright, so `Bin`/`BIN` cannot slip past the
reserved check on a case-insensitive filesystem. Windows device names (`con`, `nul`,
`com1`) are valid `SkillName`s and would fail `mkdir` on Windows — an availability nuisance
for one artifact, not an escape, and not reachable on the platforms under test.

**Coverage of the gate is complete for the write paths.** Both paths that turn a declared
hook into a directory under `$GRIM_HOME` pass it: `installer::install_one`
(`src/install/installer.rs:457`, before the integrity gate, before the fetch, before any
write) and the arming seam `hook_registrar::desired_entries`
(`src/install/hook_registrar.rs:1213`, which additionally resolves through
`AnchoredPath { anchor: GrimHome }` with `Containment::Strict`). The other `payload_dir`
callers are read-only or downstream of those two: `InstallTarget::path_for`
(`src/install/target.rs:284`) is called by the installer *after* the gate and by
`expected_outputs`; `command/hook/list.rs:271` and `:333` only read a manifest for a report.
The residual is that a *pre-existing* traversing record still reaches `path_for` before
`desired_entries` refuses it, but `Containment::Strict` and the untracked-clobber gate both
sit in front of any write — and the record itself is N1/N2 to author.

**Item 2 — `expand_payload_dir` (`src/command/hook/pipeline.rs:962`).** No escalation.
The publisher already controls every `argv` element verbatim, including `argv[0]`, so
substituting a grim-derived path into a string the publisher wrote grants nothing they did
not already have: `argv = ["/bin/sh", "-c", …]` needs no token. `argv[0]` specifically —
the expansion cannot *redirect* the spawn, because the only value substituted is
`entry.payload_dir`, which comes from `payload_relative(root, name)` under `$GRIM_HOME`
(post-SEC-1) and never from the record. There is no double-expansion (`command` is
deliberately left for `sh -c`, pinned by `the_shell_form_is_not_pre_expanded`), no other
variable is expanded, and `$GRIM_HOOK_DIRECTORY` is correctly left intact. C-019's
payload-relative-`argv[0]` rule is unaffected: it is re-applied at arming time against the
materialized tree (`validate_installed` → `payload_relative_file`, `src/oci/hook.rs:860`),
which strips the same two token spellings before deciding, so the expansion cannot walk
around it. One cosmetic residual, not filed: `to_string_lossy` on a non-UTF-8 `$GRIM_HOME`
would substitute replacement characters and the handler would simply fail to spawn
(degrades off, same as `envelope.rs:441` already does for the env var).

**Item 3 — `matches_tool`'s `|` split (`src/command/hook/run.rs:448`).** The change is in
the safe direction and the empty-alternative case is doubly closed. Grim's dialect treats
an empty alternative as matching **nothing** (exact-name path, no tool is named `""`), which
is the narrow direction; the wide vendor reading is unreachable anyway, because
`classify_matcher` (`src/install/vendor.rs:396`) sends any matcher with an empty
alternative to `MatcherForm::NotTranslatable`, which declines the client and never writes a
dispatch row. A publisher therefore cannot author a matcher that widens what a gatekeeper
fires on beyond the residuals WP-B already documents (claude/codex are start-anchored and
tail-**open**, so `Bash` also selects `BashOutput` — a pre-existing, disclosed,
fail-safe-for-gatekeeper residual, and `matcher_may_select_shell_command_tool` is correctly
prefix-aware **and** `|`-splitting for the `mutator` refusal, with `NotTranslatable`
answering `true`, i.e. refusing). Narrowing so a guardrail silently never fires is likewise
not reachable through the split: grim narrower than the vendor means grim withholds, and the
vendor gate runs first, so a matcher that arms at all fires on at least what grim admits.
Nit, not filed: the added case `(Some("Ba*|Read"), Some("Bash"), true)` documents a
glob-inside-alternation shape that `classify_matcher` refuses to arm on every v1 client, so
it pins unreachable behaviour; and the condition numbers in
`decide_grants_only_on_a_global_oci_namespaced_secure_entry` (`3`/`4`/`5`) are off by one
against the six conditions enumerated on `decide`'s doc (scope is 1, kind 2, `grants` 3,
bare host 4, insecure 5).

**`persist_grant` writes nothing that widens trust** (the brief's third trust question).
Verified end to end: it is a read-modify-write over `GlobalConfig::load`, matches on locator
**equality** rather than `grants`' prefix rule (so a bare `ghcr.io` entry gains a namespaced
sibling instead of a flag — pinned by `a_bare_host_entry_gains_a_sibling_never_a_flag`), and
the B7 hazard is genuinely closed downstream: `write_config`
(`src/command/add.rs:1136-1145`) emits **both** states of `trust_hooks`, so another entry's
explicit `trust_hooks = false` survives the rewrite rather than collapsing to absence and
silently re-arming. A namespaced global entry grants for its namespace at any depth and
stops at the segment boundary, which is the documented intent and is now executed by
`a_grant_stops_at_a_path_segment_boundary_b2` — including the `acme-evil` row that a plain
`starts_with` would have failed. Untested by the new module, and worth a line each if a
later round wants them: `arming()`'s composition order (the ⚠-Owed `OptedOut`-beats-
`--allow-hooks` choice at `trust.rs:369` has no test, so nothing pins the owner's decision
either way), and that `persist_grant` preserves a sibling entry's `trust_hooks = false`
(the property holds, but through `write_config`'s tests, not through one that names the
re-arming consequence).

**Non-goals, recorded rather than filed.** A hand-written or committed `state.json` naming
a traversing binding is refused at the arming seam, and authoring one is **N1/N2** anyway.
An armed hook deleting or flooding its own audit trail (`2 * MAX_LOG_BYTES`, no archive) is
code running at user privilege by grant — **N2**; the trail is tamper-*evidence* against
grim's own writes, which is what I5 claims. `--force` over the machine key (S2-1's
established-machine variant) is **N4**: the gate fired, named the exact path, and said it
was not grim's.

One cosmetic observation from the S2-1 reproduction, below the bar for a finding: when
convergence aborts before the launcher is generated, `$GRIM_HOME/hooks/` is left at `0o755`
rather than `0o700`, because the payload materializer's `create_dir_all` creates it first
(umask `022`) and `ensure_hooks_dir` only narrows a directory it created itself
(`hook_dispatch.rs:409-419`); `hook_launcher.rs:512` is what normally sets `0o700`, and on
the aborted path it never runs. In that state there is no table and no key to disclose, so
nothing sensitive is exposed — but if S2-1 is fixed by any route other than refusing the
name, re-check that ordering.
