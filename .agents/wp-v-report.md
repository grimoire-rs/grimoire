# WP-V — first-party example hooks

Two publishable `hook` packages in `catalog/hooks/`, wired into
`publish.toml`, covered by `test/tests/test_example_hooks.py`, and
documented by a walkthrough in `catalog/hooks/README.md` that was executed
end to end before it was written down.

**⛔ One blocker for the branch, in `src/**` (not my file set): `grim publish`
cannot resolve a version for a hook entry, so `task catalog:release` — and the
release workflow that calls it — exits 65 as soon as this manifest lands.
Three-line fix, § Findings F-1.**

## The set, and why it is this set

| Package | Tier | Events | Entries |
|---|---|---|---|
| `tool-call-logger` | `observer` | `PreToolUse` (matcher `*`), `SessionStart` | 2, sharing one payload script |
| `command-guard` | `gatekeeper` | `PreToolUse` (matcher `Bash`) | 1 |

- **`tool-call-logger` is the mandatory observer**, and it is deliberately the
  one a reader meets first (the README says so). Two entries rather than one
  because a pair bound to different moments through a single payload tree is
  the arrangement `[[hooks]]` exists for, and it costs nothing: the script
  branches on the exported `GRIM_HOOK_*` scalars. `SessionStart` also exercises
  the "no tool to match on" shape, which a `PreToolUse`-only example hides.
- **`command-guard` is the gatekeeper**, with a trivial, obviously-safe policy
  (one destructive literal) whose own description and payload comments state
  that the tier **is not a security boundary** and that a substring match is
  trivially evaded. It exists to show a verdict crossing the projection into
  Claude's `hookSpecificOutput.permissionDecision`, which no observer can show.
- **No `mutator`, deliberately.** It is the highest-risk tier, and the example
  a reader would actually want — rewriting a shell command — is precisely the
  one grim `Declined`s per client under Decision K, because a tool whose input
  is a shell-command string never admits a mutator. A first-party example is
  what people copy; shipping one that teaches a pattern grim refuses to honour
  is worse than shipping none. Recorded in `catalog/hooks/README.md` so the
  omission reads as a decision rather than an oversight.

Both use `argv = ["sh", "<script>"]` (never `command`, never `./script`),
POSIX `sh`, and the stdin-in / stdout-out / exit-0 contract. Neither writes
outside a temp path, opens a socket, or touches the workspace.

## Executed evidence — each example actually fired

All of the below is real output from the walkthrough run (local `registry:2`,
sandboxed `$GRIM_HOME` and `$HOME`, project at `…/walk/demo`).

### Arming

`grim install` after `options.experimental.hooks = true` + a **global**
`registry.examples.trust_hooks = true`:

```
root token: 6d745c497ed85fed46e5608bfc135700 -> …/walk/demo
  command-guard    refuse-recursive-root-delete claude PreToolUse  gatekeeper Bash {'argv': ['sh', 'guard.sh']}
  tool-call-logger log-session-start            claude SessionStart observer  None {'argv': ['sh', 'log.sh']}
  tool-call-logger log-tool-call                claude PreToolUse  observer  *    {'argv': ['sh', 'log.sh']}
```

Payload targets landed under `$GRIM_HOME/hooks/payload/<workspace-key>/<name>`
— nothing armable in the workspace. The Claude registration carries the
absolute launcher path (no `$GRIM_HOME` in the command) and the opaque root
token.

### Firing — the gatekeeper's verdict reaching the client

Running the *registered command itself*, with the payload Claude sends:

```
$ printf '%s' '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf / --no-preserve-root"},…}' | sh -c "$CMD"
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"command-guard (a demonstration hook) refused a command containing the literal rm -rf /"}}
(exit 0)
```

The harmless leg (`ls -la`) produced **no output** and exit 0 — no opinion, no
verdict. Both legs matter: exit 0 alone proves nothing, since every refusal
path exits 0 by design (I3).

### Firing — the observer's side effect

```
$ cat "$GRIM_EXAMPLE_LOG"
PreToolUse client=claude tool=Bash hook=tool-call-logger/log-tool-call tier=observer
PreToolUse client=claude tool=Bash hook=tool-call-logger/log-tool-call tier=observer
SessionStart client=claude tool=none hook=tool-call-logger/log-session-start tier=observer
```

Two `PreToolUse` lines because `*` matches `Bash` too, so the observer fired
alongside the gatekeeper on both invocations; the third is the `SessionStart`
entry.

### The audit trail

```json
{"…","hook_id":"refuse-recursive-root-delete","event":"PreToolUse","client":"claude","tier":"gatekeeper","digest":"sha256:b92d6c3e…","verdict":"deny","outcome":"completed"}
{"…","hook_id":"log-tool-call","event":"PreToolUse","client":"claude","tier":"observer","digest":"sha256:f625e48c…","verdict":"noopinion","outcome":"completed"}
{"…","hook_id":"log-session-start","event":"SessionStart","client":"claude","tier":"observer","digest":"sha256:f625e48c…","verdict":"noopinion","outcome":"completed"}
```

### Teardown, verified

`grim uninstall hook command-guard` + `… tool-call-logger` ⇒ `rows: []`,
`.claude/settings.local.json` back to `{}`, payload directory empty.

## Acceptance tests

`test/tests/test_example_hooks.py`, **9 passed**. It releases the *real*
`catalog/hooks/<name>` directories (no fixture manifest anywhere in the file),
arms them, and fires the dispatcher exactly as the generated launcher does.
Every assertion is on a side effect:

| Test | Asserts |
|---|---|
| `…observer_example_logs_a_tool_call` | the exact log line, field for field |
| `…observer_example_logs_a_session_start` | the second entry, sharing the payload tree |
| `…observer_never_denies_anything` | logger armed **alone**, fed the guard's own trigger ⇒ ran, no verdict |
| `…gatekeeper_example_denies_and_the_verdict_reaches_the_client` | both legs; `permissionDecision == "deny"`, exit 0 |
| `…gatekeeper_example_leaves_an_audit_record` | verdict, tier, and the **pinned digest from the dispatch row** |
| `…examples_do_not_arm_on_install` | `state: gated`, cause `feature-flag-off`, zero rows + positive control |
| `…uninstalling_the_examples_disarms_them` | row, registration, payload all gone; a fire on the *pre-uninstall* root token runs nothing |
| `…every_example_hook_is_wired_into_the_catalog` | `catalog/hooks/` ≡ `publish.toml` `[hooks]`, description companion exists |
| `…every_example_declares_its_tier_in_its_description` | tier named in the published description; the gatekeeper disowns "security control" |

## Gates

- `task catalog:verify` — both hooks build alongside every other package.
- `task verify` — **1083 passed**. (`--force` not used; it was blocked by a
  permission classifier this session, as briefed.)
- `.claude/tests/uv.lock` and `test/uv.lock` reverted; nothing staged with
  `git add -A`; the `commit-verified` stamp was written by `task verify`
  itself, never by hand.
- `shellcheck` / `shfmt` are **not installed on this machine**, so
  `task shell:verify` skipped them. Both payloads pass `sh -n` and were
  hand-checked against the `-i 4 -ci` style; CI is the first real lint of them.

## Findings — defects in `src/**`, reported not fixed

### F-1 (Block) — a hook entry can never inherit a publish version

`src/command/publish.rs:685` — `resolve_versions`'s `tables` array lists
skills, rules, agents, bundles, mcp, and **omits `&mut manifest.hooks`**. A
hook entry that omits `version` therefore keeps `None`, and `validate_entry`
rejects it:

```
catalog/publish.toml: entry 'command-guard': missing version (set a per-entry, top-level, or --version value)
```

This breaks `task catalog:release` for the **whole catalog** (validation runs
before any push) the moment these entries land, and with it the
`publish-catalog.yml` release job. There is no manifest-side workaround worth
taking: the catalog contract is that every package omits `version` and
inherits grim's release version via `--version <git tag>`, so pinning
`version = "0.13.0"` on the hooks would freeze them at 0.13.0 forever while
everything else moved (an explicit per-entry version wins over `--version`),
and `version = "${version}"` is not expanded either — the placeholder is
substituted in the same loop.

I left the entries version-less, which is the contract-correct form. **The
one-line fix is adding `&mut manifest.hooks` to that array.** Verified by
probe: with a version pinned by hand, the rest of the hook publish path is
already correct — kind order (`… → mcp → hooks → bundles`), the
`grimoire-rs/hooks/<name>` repository segment, cascade tags
`0.13.0,0.13,0,latest`, and the description companions all planned exactly as
intended.

### F-2 (Warn) — `grim publish --only <hook-name>` always fails

`src/command/publish.rs:1595` — the `all_names` set for `--only` validation
chains the same five maps and omits `manifest.hooks.keys()`:

```
$ grim publish --manifest catalog/publish.toml --dry-run --only tool-call-logger
… --only 'tool-call-logger': name not found in manifest; known entries: ai-config-authoring, grim, grim-authoring, grim-essentials, grim-usage
```

The error message is actively misleading — the entry *is* in the manifest.
`task catalog:release -- --only <hook>` is documented in `catalog/README.md`
as the publish-one-package-by-hand route, so this is a real hole in a
documented workflow.

### F-3 (Warn) — a hooks-only manifest reports "no packages declared"

`src/command/publish.rs:1584` — `total_entries` sums the same five maps
without `manifest.hooks.len()`, so a manifest declaring only hooks exits 65
with "no packages declared in manifest". Not reachable from this catalog
(which has skills too), but it is the same omission and belongs in the same
fix.

All three are one omission repeated: the `hooks` map was appended to
`PublishManifest`, to `validate_manifest`, and to `plan_entries`, but not to
the three helper aggregations. A test that publishes a hooks-only manifest
with an inherited version would have caught every one.

### F-4 (Warn) — the feature flag has no CLI route back to `false`

`src/command/config.rs:728` — `refuse_disarm_via_config` refuses both
`grim config set options.experimental.hooks false` **and**
`grim config unset options.experimental.hooks` (exit 65) without ever
consulting arming state. Reproduced *after* uninstalling every hook, with zero
dispatch rows and an empty registration: still refused. Its message —
"run `grim install` to disarm" — is circular, since `grim install` cannot
disarm a flag it cannot see turned off. Hand-editing `grimoire.toml` is the
only route, which `test_hook_arming.py` already works around and attributes to
an owner-deferred decision (review W6). Documented as a known gap in
`catalog/hooks/README.md` so the walkthrough's teardown is honest.

### F-5 (Warn, pre-existing — and being fixed concurrently) — `grim hook list` returned no rows

On this branch's base (`7bbc348`) `grim hook list` printed its six-column
header and **no rows** with three hooks armed, matching the stale-stub note in
`subsystem-cli-commands.md`. A concurrent package landed the real
implementation while this was being written, so **treat this as fixed and
verify against your own build** rather than re-filing it. The walkthrough
points readers at `grim status` (which is the arming report either way) and
version-qualifies the `hook list` caveat to 0.13.0, so the README does not go
stale when that fix merges.

## Nothing found wrong in the hook *format*

`hook.toml` behaved exactly as `src/oci/hook.rs` documents. Kind inference
picked `hook` from `hook.toml` with no `--kind` flag; the C-019 first-token
rule, the matcher allowlist, and the name/stem rule all matched the prose.
