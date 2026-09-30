# WP-N — First-Party Catalog Drift Review (hooks artifact kind)

Branch `hex/hooks-artifact-kind--wp5-n`, based on `8d999ba`.
Scope: `catalog/taskfile.yml` + `catalog/skills/{grim-usage,grim-authoring,ai-config-authoring}/**`.
Nothing outside that set was touched.

## Gates

| Gate | Result |
|---|---|
| `task catalog:verify` | exit 0 |
| `task --force verify` | exit 0 — 1046 acceptance tests passed |

Both were re-run against the **final** tree after the last content edit.
`.claude/tests/uv.lock` and `test/uv.lock` were reverted after each run; the
working tree holds only the 15 modified files plus the one new file.

The worktree needed `git submodule update --init --recursive` before it could
build — `external/docker_credential` and `external/rust-oci-client` were absent,
so `cargo build` failed on a missing `Cargo.toml`. Worth knowing for any future
agent worktree.

---

## Every enumeration found and fixed

Found by grep, not by assumption. `file:line` is the pre-edit line.

### Kind counts and kind lists

| Location | Was | Now |
|---|---|---|
| `catalog/skills/grim-authoring/SKILL.md:3` (frontmatter `description`) | five kinds listed, no hook | hook added, plus a hook-authoring trigger phrase |
| `catalog/skills/grim-authoring/SKILL.md:7` (`metadata.summary`) | "skill, rule, agent, mcp, and bundle" | + hook |
| `catalog/skills/grim-authoring/SKILL.md:8` (`metadata.keywords`) | no `hook` | + `hook` |
| `catalog/skills/grim-authoring/SKILL.md:14` | "publishes five artifact kinds" | six |
| `catalog/skills/grim-authoring/SKILL.md:19` | heading `## The Five Kinds` | `## The Six Kinds` (+ new table row, + the inference-order note) |
| `catalog/skills/grim-authoring/references/bootstrap-existing-repo.md:17,129` | `[The Five Kinds][five-kinds]` + anchor `#the-five-kinds` | six / `#the-six-kinds` |
| `catalog/skills/grim-usage/SKILL.md:3` (frontmatter `description`) | "skills, rules, agents, and bundles"; command list had no `hook` | six kinds named; `hook` added to the command list |
| `catalog/skills/grim-usage/SKILL.md:8` (`metadata.keywords`) | no `hooks` | + `hooks` |
| `catalog/skills/grim-usage/SKILL.md:15` | "distributes five artifact kinds" | six, hooks named |
| `catalog/skills/grim-usage/references/consume.md:66` | `--kind` (skill, rule, agent, bundle, mcp) | + hook, + the append-not-insert rule |
| `catalog/skills/grim-usage/references/troubleshooting.md:205` | `add` "asks for `--kind`" with no value list | value list spelled out incl. `hook` |
| `catalog/skills/grim-usage/references/updating.md:33` | **"Tier-1 invariants (the four kinds…)"** — already wrong before hooks (body said five) | six, named, plus an instruction to re-derive the count from `grim add --help` and a new step 5 whose whole job is re-counting |

### `grim schema --kind` value lists

| Location | Was | Now |
|---|---|---|
| `grim-authoring/SKILL.md:170` | `<config\|publish\|lock\|mcp>` | `…\|hook>` |
| `grim-usage/SKILL.md:80` (command map row) | no `hook.toml` | + `hook.toml` |
| `grim-usage/references/publish.md:259` (`#editor-schema`) | `config\|publish\|lock\|mcp` | `…\|hook` |
| `grim-usage/references/publish.md:360` | already only cited `--kind publish` — correct, left alone | — |
| `grim-authoring/references/updating.md` | no schema-command pointer | added; `--kind hook` named as the shortest path to `hook.toml` ground truth |

### Batch-publish kind ordering

| Location | Was | Now |
|---|---|---|
| `grim-authoring/references/release-checklist.md:65` | skills → rules → agents → mcp → bundles | skills → rules → agents → mcp → **hooks** → bundles |
| `grim-usage/references/publish.md:91` | same, in prose | same fix, plus an explicit note that `mcp`/`hooks` precede `bundles` because the sequence is fixed, **not** because a bundle can hold them |
| `catalog/taskfile.yml` | five per-kind build loops (skills, rules, agents, mcp, bundles) | sixth loop for `hooks/*/`, inserted after `mcp` and before `bundles`, plus `hooks/**/*` in `sources:` |

### Registry fields (`trust_hooks` is a new 7th field)

| Location | Was | Now |
|---|---|---|
| `grim-usage/references/registries.md:87` | field table ended at `insecure` | `trust_hooks` row added, with the global-grants / project-restricts-only asymmetry |
| `grim-usage/references/registries.md:419` | frozen field order `…, exclude, insecure` | `…, insecure, trust_hooks`, marked append-only |
| `grim-usage/references/registries.md:493` | `fields` output described as `(oci/index/default/include/exclude/insecure)` | `+/trust_hooks` |

### `grim hook` surface

- `grim-usage/SKILL.md` command map: new `grim hook list` row.
- `grim-usage/SKILL.md`: new callout framing **`grim hook run` as not-a-command-you-type**, explicitly paired with `grim mcp` ("speaks JSON-RPC on stdout rather than to a person"), naming its four launcher-supplied flags.
- `grim-usage/references/consume.md`: new `## Hooks` section (gates, what you get today, `grim hook list` with its real output, the same `hook run` framing).
- `grim-usage/references/troubleshooting.md`: new `## A Hook That Never Fires` section — opens by stating this is *expected, not a fault*.

### New file

`catalog/skills/grim-authoring/references/hook-spec.md` — the per-kind
authoring reference, routed from `grim-authoring/SKILL.md`'s routing table.
Covers `hook.toml`'s four top-level keys, the `[[hooks]]` field table, the four
canonical events, the three tiers and their per-event validity, the matcher
dialect, the handler forms, the payload-not-executable trap, vendor override
tables, reserved artifact names, the client set, publishing, and a
16-row validation-pitfall table. Every pitfall row is a message I reproduced.

### Other hook-shaped edits

- `grim-authoring/SKILL.md`: new `## Hooks Ship Disarmed` section; hook row added
  to the metadata-location asymmetry table (it is the *only* kind where a
  misplaced catalog key is a hard error rather than silent loss); hook bullet
  added to "Which Clients Host Which Kind"; local-dev-loop section now states
  a hook has no path form and why.
- `grim-authoring/references/bundle-spec.md`: "Three optional tables" → "and
  only three", with the reproduced parse error and a new pitfall row —
  **neither `mcp` nor `hooks` is bundleable**.
- `grim-authoring/references/release-checklist.md`: pre-release list gained the
  hook-needs-no-flag item and the interpreter-form item (renumbering 5→6);
  exit-65 triage table gained 7 hook rows.
- `grim-authoring/references/bootstrap-existing-repo.md`: inventory step leads
  with the hook arm and warns that a stray `hook.toml` reclassifies a
  directory; metadata backfill notes the hook exception; verify loop and
  `publish.toml` skeleton updated.
- `grim-usage/references/publish.md`: build kind-detection sentence, header
  scope line, `publish.toml` example gained `[hooks.shell-guard]` plus the
  conventional-source-path list, `pin = true` bullet generalized ("any other
  kind — skill, rule, agent, mcp, or hook").
- `grim-usage/references/troubleshooting.md`: kind-inference gotcha gained the
  hook arm and its ordering; exit-65 list gained a hook-manifest bullet; exit-78
  section now includes hooks in the "nowhere to go" set.
- `ai-config-authoring/SKILL.md` + `references/choosing-types.md`: the claim
  **"Hooks must be re-implemented per client"** was the real drift here — grim
  now packages one declaration and translates it. Corrected *narrowly*: still
  several technologies, packageable across Claude/Codex/Copilot only, OpenCode's
  plugin surface not one grim writes, experimental and off by default. Codex
  moved into the hook row of the vendor-disagreement table ("unsurveyed for the
  other seven" → six).
- All three `references/updating.md` files: re-research protocol steps, durable
  search terms, and canonical links for hooks; each now carries an explicit
  instruction **not** to widen the arming claims without checking the binary.

---

## Executed command output backing each documented shape

Binary: `target/release/grim`, built from `8d999ba` in this worktree.

**`grim hook --help`** (backs the `hook run` framing verbatim):
```
Dispatch armed lifecycle hooks, and list what is armed.

`grim hook run` is invoked by the launcher grim generates, not by hand; `grim hook list` is the user-facing surface.

Commands:
  run   Dispatch armed hooks for one client event (invoked by the generated launcher, not by hand)
  list  List declared hooks with their tiers, events, and per-client arming state
```

**`grim hook run --help`** (backs the four flags and the exit-0 contract):
```
Usage: grim hook run [OPTIONS] --client <CLIENT> --event <EVENT> --table <TABLE> --root <ROOT>
…Reads only the dispatch table named by `--table`; resolves no scope and no
configuration. Exits **0** in every case grim controls…
```

**`grim hook list`** (backs the column set and the empty declared set):
```
$ grim hook list
Hook  Tier  Events  Client  State  Detail
$ echo $?
0
$ grim hook list --format json
{
  "items": []
}
```

**`grim add --help` / `grim build --help` / `grim release --help`** — all three
report `[possible values: skill, rule, agent, bundle, mcp, hook]`, and `add`'s
help states the append-not-insert rule and that "a hook may be declared with
the feature flag off".

**`grim schema --help`** possible values:
```
- config:  The `grimoire.toml` declaration file …
- publish: The `publish.toml` batch-release manifest
- lock:    The `grimoire.lock` lockfile …
- mcp:     The MCP server descriptor (`mcp/<name>.toml`)
- hook:    The hook manifest (`hook.toml`)
```
`grim schema --kind hook` emits a real schema (`$defs.CanonicalEvent` with the
four events, `HookEntry`, …). All three schemas carry hooks:
`--kind config` top level has `hooks`; `--kind lock` has `hook`;
`--kind publish` has `hooks` (`PublishEntrySpec` = `description, path, pin,
repository, version`).

**`grim build` on a hook fixture** — kind is inferred, no flag needed:
```
$ grim build …/shell-guard
Kind  Name         Path            Layer Digest      Status
hook  shell-guard  …/shell-guard   sha256:bdbbc6cc…  built
$ echo $?   # identical output and digest with --kind hook
0
```

**The publisher-facing refusal (C-019)** — the trap the brief asked for:
```
$ grim build …/bad-guard --kind hook       # command = "./guard.sh --strict"
hook 'direct-exec' runs the payload file './guard.sh' directly; a payload delivered
through a registry is not executable — name an interpreter instead, e.g.
argv = ["sh", "${GRIM_HOOK_DIR}/./guard.sh"]
$ echo $?
65
```

**Every other validation rule, each reproduced (all exit 65):**
```
tier 'mutator' is not valid at event 'PostToolUse'
tier 'gatekeeper' is not valid at event 'SessionStart'
hook 'native-only' declares tier 'mutator' on a native-only event: 'mutator' requires event = "PreToolUse"
invalid matcher '^Bash$': expected only [A-Za-z0-9_*?./-|]
matcher of 257 bytes exceeds the 256-byte limit
invalid matcher: an empty matcher is ambiguous across clients; omit 'matcher' to match every tool
duplicate hook id 'obs'
hook manifest name 'other-name' must equal the directory stem 'meta-guard'
hook artifact name 'bin' is reserved: 'bin' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact
hook 'obs' declares both 'argv' and 'command'; exactly one of them is required
hook 'obs' declares no handler: set exactly one of 'argv' or 'command'
hook 'obs' declares no event: set 'event' or a single '<vendor>.event' override
hook manifest schema version 2 is not supported (this grim understands 1)
key 'clod' is not a per-client override table: expected '<client>.<field>' naming a client grim supports
unknown field `summary`, expected one of `schema`, `name`, `description`, `hooks`
```
Accepted (exit 0): `[hooks.claude] timeout = 10` + `[hooks.codex] event =
"PermissionRequest"`; a native-only moment at tier `observer`.

**A bundle cannot hold a hook:**
```
$ grim build …/hook-bundle.toml --kind bundle
invalid TOML: TOML parse error at line 6, column 2
unknown field `hooks`, expected one of `skills`, `rules`, `agents`, `summary`,
`keywords`, `description`, `license`, `repository`, `deprecated`, `replaced-by`
$ echo $?
78
```

**The gates:**
```
$ grim config list --all | grep -i hook
options.experimental.hooks
$ grim config registry fields          # 7 rows; last one:
trust_hooks  boolean  Trust hooks from this registry  Controls whether hooks resolved from
this registry may arm their clients. In global config, unset means trusted… In project
config the key may only restrict…
$ grim install --help | grep -c allow-hooks    # and add/update/lock/status/hook
0
```

**The taskfile loop actually fires** (proved with a throwaway
`catalog/hooks/probe-guard/`, since an untested loop is a liability; fixture
deleted afterwards, tree confirmed clean):
```
==> grim build hooks/probe-guard
Kind  Name         Path               Layer Digest      Status
hook  probe-guard  hooks/probe-guard  sha256:b195c0da…  built
==> grim build bundles/grim-essentials.toml
```

---

## Deliberately left undocumented pending WP-R

Per the brief, nothing aspirational was written. Specifically **owed to a later
docs pass, once WP-R lands**:

1. **The consent-prompt flow.** Not documented anywhere. No shipped surface
   presents it.
2. **`grim hook list`'s per-hook rows in practice.** The column set and the
   `{"items": []}` envelope are documented (both executed); the per-client
   `arming` array's live semantics are described only as the report's *shape*,
   never as observed behaviour. No worked non-empty example is shown, because I
   could not produce one — the declared set is empty on every real machine
   until a hook is published and declared.
3. **What install reports for a hook.** `grim install`'s hook path, its
   `gated` / `skipped` / `armed` status tokens, and the `HookArmingCause`
   vocabulary (`feature-flag-off`, `registry-not-trusted`,
   `client-has-no-hook-surface`) all exist in `src/api/artifact_status.rs` — I
   documented **none** of them. Owed once reachable.
4. **`--allow-hooks`.** The per-invocation gate exists in design prose
   (`src/config/declaration.rs:165`) but **no such flag exists on any
   subcommand** (verified: 0 hits across `install`, `add`, `update`, `lock`,
   `status`, `hook`). I removed the third gate from the two places I had first
   written it and documented **two** gates, not three. If WP-R ships the flag,
   both the `grim-authoring` "Hooks Ship Disarmed" section and
   `consume.md`'s gate table need a third row.
5. **The launcher, the dispatch table, `$GRIM_HOME/hooks/` layout.** Named only
   obliquely (in the reserved-names explanation, which quotes grim's own error
   text). No layout is documented.
6. **The response-projection / verdict vocabulary.** Not documented at all —
   no `permissionDecision`, no per-client token tables. A hook author cannot
   yet learn from the catalog what JSON to emit. This is the largest genuine
   documentation gap the kind still has, and it is correctly WP-M/WP-R's to
   fill once arming works.
7. **`grim status` for hooks.** `src/command/status.rs` has hook-aware
   reporting (`feature_enabled`, `trust_hooks` roll-up). Not documented.

Where a line describes what a *client* does with an entry, `hook-spec.md`
opens with an explicit disclaimer that it is the format's contract and must be
re-confirmed against the binary — so no sentence there reads as an observation
of arming.

---

## Things I found **wrong** in the plan, the catalog README, or merged code

Ordered by how much they matter. None are in my file set, so none were fixed.

### 1. `src/install/hook_registrar.rs:313-321` — stale comment, contradicted by its own tree (Block-tier for WP-R)

`sync_for_state`'s fall-through comment asserts:

> "**No shipped seam can produce either**: `installer::locate_canonical` refuses
> `ArtifactKind::Hook` before any record is written…"

`locate_canonical` does **not** refuse Hook — `src/install/installer.rs:2499`
and `:2517` handle it explicitly (`ArtifactKind::Hook | ArtifactKind::Skill`),
and eight async tests at `installer.rs:5960-6300` exercise the full hook
install path by name:

```
a_hook_materializes_one_shared_payload_dir_with_one_output_per_client
an_installed_hook_payload_carries_no_exec_bit
re_materializing_a_hook_leaves_the_record_not_modified
a_surfaceless_client_is_reported_as_skipped_never_armed
a_codex_hook_at_project_scope_is_skipped
a_project_hook_install_writes_nothing_armable_into_the_workspace
a_new_hook_digest_re_materializes_and_moves_the_recorded_pin
a_locally_modified_hook_payload_is_refused_until_forced
```

So a hook record **is** producible by a shipped seam, and the comment's
premise — that only a hand-edited `state.json` reaches line 328 — is false. The
consequence is not cosmetic: line 328 returns
`Err(unsupported_kind())`, and `sync_config` callers log an `Err` as a warning,
so **a legitimately installed hook makes every install/update/uninstall emit a
spurious "hooks not armed" warning** for the affected client. Whoever writes the
convergence body must fix or delete this comment, not build on it.

### 2. `src/command/publish.rs:1867` — wrong justification for the ordering

> `// Hooks are bundle members too, so they publish BEFORE bundles for the same`

Hooks are **not** bundle members. `RawBundleSource`
(`src/config/project_config.rs:774-782`) is `deny_unknown_fields` over exactly
`skills`, `rules`, `agents` — no `hooks`, no `mcp`. Proved by execution above
(exit 78). The *ordering* is right; only the reason is wrong, and it is the
kind of wrong comment that invites someone to add a `[hooks]` bundle table
"for consistency". `mcp` sits in the same position with no such claim attached.

### 3. `trust_hooks` is invisible in every report surface (Warn-tier, and it is a security-review gap)

The field is settable and readable, but absent from all three places a user
would look — while the analogous per-registry security field `insecure` is
present in all three:

```
$ grim config set registry.acme.trust_hooks false      # exit 0, written to the file
$ grim config get registry.acme.trust_hooks            # false
$ grim config registry show acme --format json
{"alias":"acme","oci":"ghcr.io/acme","index":null,"include":[],"exclude":[],
 "default":true,"insecure":false}                       # ← no trust_hooks
$ grim config registry list --format json               # ← no trust_hooks
$ grim context --format json | jq .registries            # ← no trust_hooks
```

There is also no `--trust-hooks` / `--no-trust-hooks` flag on
`config registry add` or `set` (unlike `--insecure` / `--no-insecure`), so the
dotted key is the only write path. Net effect: a user cannot audit which
registries may arm hooks from any report, and cannot manage the field with the
verb built for managing registry entries. Adding the fields is additive and
`always-present-null` fits `subsystem-cli-api.md`. I documented `config get` as
the inspection route, which stays correct either way.

### 4. `grim schema`'s own `about` text is stale (Warn-tier)

```
$ grim --help | grep schema
  schema  Print the JSON Schema for grimoire.toml, publish.toml, or grimoire.lock
$ grim schema --help | head -1
Print the JSON Schema for grimoire.toml, publish.toml, or grimoire.lock
```

It omits **both** `mcp` (pre-existing drift) and `hook`. The `--kind` value
help underneath is correct. One `about` string in `src/command/schema.rs`.

### 5. Hook build errors print their source chain twice (Warn-tier)

```
$ grim build …/meta-guard --kind hook
invalid hook manifest: TOML parse error at line 4, column 1
  |
4 | summary     = "A one-line blurb"
  | ^^^^^^^
unknown field `summary`, expected one of `schema`, `name`, `description`, `hooks`
: TOML parse error at line 4, column 1
  |
4 | summary     = "A one-line blurb"
…
```

The `HookError::Toml` `#[error]` string already renders the inner error, and
the `{err:#}` boundary appends `source()` again — so the whole multi-line TOML
diagnostic is emitted twice, joined by a bare `": "`. Per
`quality-rust-errors.md`, this is the `#[error(transparent)]` case.

### 6. The C-019 hint interpolates the raw token, producing a malformed suggestion (Suggest-tier)

For `command = "./guard.sh --strict"` the hint reads:

> `argv = ["sh", "${GRIM_HOOK_DIR}/./guard.sh"]`

The `./` is carried through, so the suggested line has a `/./` in it. It works,
but the message is the publisher's first contact with the rule and it should
paste cleanly. `payload_relative_file` already normalizes `CurDir` away for the
*check*; the diagnostic should quote the normalized form.

### 7. `catalog/README.md` has not been updated for the hook kind (out of my file set, so unfixed)

Three specific gaps:

- The layout block lists `rules/<name>.md` and `agents/<name>.md` as "(when the
  first … package lands)" but has **no `hooks/<name>/` line at all** — even
  though `publish.toml` has a `[hooks]` table and `taskfile.yml` now builds
  them.
- "Registry refs are kind-segmented" names only `skills/` and `bundles/`.
- **"Keeping content honest"** is the drift-review duty itself, and its trigger
  list is `docs/src/{artifacts,clients,publishing,vendor-metadata,commands,package-index}.md`
  + `src/command/**` + `src/mcp/**`. It should name the new hook docs page
  (WP-M's) and, more importantly, `src/oci/hook.rs` — the file that actually
  defines the published format. As written, a future change to `hook.toml`'s
  schema triggers no review of `grim-authoring`.

Recommend folding these into WP-M, which already owns docs prose.

### 8. Pre-existing catalog drift I fixed opportunistically, worth flagging as a process signal

`grim-usage/references/updating.md:33` called the kind set "the **four** kinds"
while `SKILL.md:15` in the same package said **five**. The file whose stated job
is preventing drift had itself drifted, and by a whole kind. That is why I added
an explicit re-count step keyed to `grim add --help`'s generated
`[possible values: …]` rather than to prose.

Separately: `grim-usage/SKILL.md` documents `grim describe`, and the binary has
it — but `.claude/rules/subsystem-cli-commands.md`'s command table does **not**
list `describe` (nor `hook`). That rule is WP-M's file; flagging it here so it
is not missed.

---

## Commit

One commit, `chore(catalog):`. `chore:` is correct per `AGENTS.md` — the whole
diff is AI-config/skill content plus the subsystem taskfile that validates it.
It ships no user-visible grim behaviour, and the catalog publishes at grim's own
release version with no per-package bump, so a changelog entry would describe a
release the diff does not cause. The `feat(hook):` commits in this stack are
what belongs in the changelog.
