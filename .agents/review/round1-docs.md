# Round 1 — doc-reviewer report (`hex/hooks-artifact-kind`, tip `a5399fd`)

Question: **is the documentation true, and would a user succeed by following it?**

Method: verified against `target/release/grim` (built here from `a5399fd` after
`git submodule update --init --recursive`; `grim 0.13.0`) and against `src/**`.
No claim below is sourced from another doc.

`mdbook` is **not installed** in this environment, so **no render check was
performed**. Anchors/links were checked by extracting every heading anchor from
`docs/src/**` and resolving every intra-book link against it (method noted in
the relevant finding).

Status: COMPLETE. **7 Block, 10 Warn, 4 Suggest**, plus a
"Verified true" section at the end recording what was checked and held, so a
second round does not re-spend the build.

Two of the seven Blocks (**B-2**, **B-6**) and one Warn (**W-3**) are the exact
failure mode this branch's history predicted: a closed enumeration that grew by
one and a doc that was not re-counted. **B-6** additionally pastes a `grim build`
transcript that does not reproduce.

---

## Block

### B-1 — `docs/src/mcp-servers.md:366-367` — "a bundle accepts skill, rule, and agent members only" is false

The Limitations bullet reads:

> - **MCP descriptors cannot be bundle members yet.** A [bundle](./concepts.md#bundles)
>   accepts skill, rule, and agent members only.

The first sentence is still true. The second is false as of `ebb37bc`
(`feat(bundle): accept hooks as bundle members`): a bundle accepts **hook**
members too. `src/command/build.rs:421-430` pushes `ArtifactKind::Hook`
members from a `[hooks]` table, and `src/config/project_config.rs`'s
`BundleSource` lists `hooks` among its accepted tables.

Verified — a bundle source with `[hooks]` builds, one with `[mcp]` is rejected
and the error names the accepted set:

```
$ grim build ./hookbundle.toml --kind bundle --format json
{ "kind": "bundle", "name": "hookbundle",
  "layer_digest": "sha256:e868ab78ebf8865aedb7ed743a518830093cb40782da192d5131107c76885cf4",
  "annotation_count": 2, "status": "built" }

$ grim build ./mcpbundle.toml --kind bundle --format json
invalid TOML: TOML parse error at line 4, column 2
  |
4 | [mcp]
  |  ^^^
unknown field `mcp`, expected one of `skills`, `rules`, `agents`, `hooks`,
`summary`, `keywords`, `description`, `license`, `repository`, `deprecated`,
`replaced-by`
```

Correction: keep the MCP limitation, drop the closed enumeration — e.g. "A
bundle accepts skill, rule, agent, and [hook](./artifacts.md#hooks) members;
an MCP descriptor cannot be one yet." **actionable**

### B-2 — `catalog/skills/grim-authoring/references/hook-spec.md:288` and `:361` — "**Two** artifact names are refused" — there are three

> ## Reserved Names
>
> Two artifact names are refused: `bin` and `dispatch.json`. Both name part
> of grim's own launcher namespace…

and the pitfall row `| Artifact named `bin` or `dispatch.json` | … |`.

`src/oci/hook.rs:154` is the authority:

```rust
pub const RESERVED_ARTIFACT_NAMES: [&str; 3] = ["bin", "dispatch.json", "payload"];
```

`payload` was added when the payload tree moved to
`$GRIM_HOME/hooks/payload/<workspace-key>/<name>/` (SEC-1, `975fc66`), and
`.claude/rules/subsystem-file-structure.md:148` states three correctly. The
published publisher-facing skill still says two:

```
$ grim build ./payload --kind hook
hook artifact name 'payload' is reserved: 'payload' names part of grim's own hook launcher under $GRIM_HOME/hooks/ — rename the artifact

$ grim build ./dispatch.json --kind hook
hook artifact name 'dispatch.json' is reserved: …
```

This is the same class of drift the branch already had to fix twice in this
file's neighbours: a closed enumeration that grew by one.

Correction: "Three artifact names are refused: `bin`, `dispatch.json`, and
`payload`", and the same in the pitfall row. **actionable**

### B-3 — `docs/src/clients.md` — Codex's Hook `✓` carries no caveat for the known "silently never fires" case under a non-POSIX login shell

The matrix gives [Codex] `✓` for Hook, and `{#gap-hooks}` says Claude, Codex
and Copilot "read the same wire contract, so one canonical hook projects to
all three". Nothing on the page, in `docs/src/artifacts.md`, or in
`catalog/hooks/README.md` mentions that a Codex user whose login shell is
`fish` or `nushell` gets a hook that registers, reports armed, and never runs.

grim's registered command is POSIX `sh` (`src/install/hook_launcher.rs:338-343`):

```
L='…/grim-hook'
[ -f "$L" ] && [ -x "$L" ] || exit 0
"$L" run --client codex --event PreToolUse --table … --root …
s=$?
case "$s" in 0) exit 0 ;; …*) exit 0 ;; esac
```

and `src/install/hook_launcher.rs:89-92` records the consequence:

> POSIX-`sh` only, throughout: claude runs `/bin/sh`, copilot runs `bash`, and
> codex runs the **user's** `$SHELL -lc` (WP-B § 4) — which is why a `fish` or
> `nushell` user cannot execute this string at all. That is a real "hook
> silently never fires" class, out of this fold and watchlisted (WP-M).

`s=$?` and `case …esac` are both syntax errors in `fish`, so the whole string
fails to parse; Codex does not block on a non-zero hook exit (stated in
`.claude/rules/vendor-capability-watchlist.md`, `preToolUse` fails-closed row,
as a Copilot-only property), so the failure is silent.

The branch's own watchlist row names the doc obligation and it was not
discharged:

> | Hooks run through `$SHELL -lc` | Codex | … | *found by execution.* … | emit a
> shell-neutral registration (or an exec-form argv) for Codex; **until then,
> document the `$SHELL` constraint** rather than widening the one-liner |

I could not paste a live `fish` failure — no `fish` or `nu` is installed in
this environment (`command -v fish nu` returns nothing) — so the evidence is
the emitted command string plus the code's own statement of the constraint.

Why Block rather than Warn: a `gatekeeper` on Codex is the one configuration
where a reader is told the guardrail is armed (`grim status` reports
`installed`/`untrusted`, not `not-armed`, because grim's own work *is*
complete) while it can never fire. That is the "trusting a guardrail that is
not one" case.

Correction: add a `{#gap-codex-shell}` Known-gaps entry on
`docs/src/clients.md` naming the `$SHELL -lc` dependency and the POSIX
requirement, and cross-reference it from `docs/src/artifacts.md`'s
not-a-security-boundary blockquote. **actionable**

### B-4 — `catalog/skills/grim-authoring/references/hook-spec.md` — the publisher-facing tier reference presents `gatekeeper` as a security control and never states it is not a boundary

`hook-spec.md:133-145` is the authoring reference a hook publisher reads, and
its whole framing is that a `gatekeeper` blocks:

> | `gatekeeper` | Return a verdict that blocks the operation | only events where some client can express a verdict |

reinforced at `:138` — degrading a tier "would report **a security control** as
installed when it is not" — and at `:129-131` ("Relocating a `PreToolUse`
guardrail onto `PostToolUse` … runs the handler *after* the damage. Accept the
decline instead."). Nowhere in the file's 386 lines does the phrase "not a
security boundary", "defence in depth", or "fails open" appear (grep over the
file: no hits).

The design says the opposite, in `docs/src/artifacts.md:458-462`:

> **A grim hook is defence in depth, never a security boundary.** A
> `gatekeeper` that does not fire — because grim is not installed, the launcher
> is missing, or the client never registered it — is *by design*: every layer
> fails open …

and the code implements exactly that: `[ -f "$L" ] && [ -x "$L" ] || exit 0` in
every registration (`src/install/hook_launcher.rs:339`), plus the `Allow`-drop
and the whole-table-reject paths.

`catalog/hooks/README.md:16` and both example manifests do carry the warning —
so the omission is specifically in the reference a publisher is pointed at to
*write* a gatekeeper, which is the reader most likely to rely on it.

Correction: state the non-boundary contract once in `hook-spec.md`'s Tiers
section, matching `docs/src/artifacts.md:458`, and reword `:138`'s "a security
control" so it does not assert the opposite. **actionable**

### B-5 — `docs/src/json-interface.md:255-266` — the `cause` → `state` table is presented as complete and omits `not-registered`

> Each cause maps to exactly one `state` token, so a consumer can group without
> maintaining its own table:

Eight rows follow. `HookArmingCause` has **nine** variants
(`src/api/artifact_status.rs:103-153`, tokens at `:247-260`); the missing one is
`not-registered` → `not-armed` (`state()` at `:163-167`).

It is not a future cause: it is the **reporting half of this branch's own
wave-8 audit fix P-1** (`05b6d20 fix(install): keep a declined hook out of the
dispatch table`), reached from `grim status` through
`src/command/status.rs:958` (`merge_not_registered`), with the message

```
grim registered nothing here for this client, so nothing runs — usually its tier
at that event or its matcher; re-run grim install to see the reason it reports
```

Two of the file's own frozen-contract claims make the omission load-bearing:
the sentence quoted above tells a consumer the table is a substitute for its
own mapping, and `docs/src/stability.md:394-396` tells them to "Branch on the
`cause` enum, which is frozen — see [Hook arming](./json-interface.md#hook-arming)".
A consumer who does exactly that will not recognize the one cause that reports
a *declined* hook — the state the P-1 fix exists to make legible.

`not-registered` is also the only cause whose meaning a reader cannot guess
from the token, which is what makes the omission worse than a count error.

Correction: add the row

| `not-armed` | `not-registered` | grim registered nothing for this client — usually the entry's tier at that event, or its matcher. Re-run `grim install` to see the reason. |

**actionable**

### B-6 — `catalog/skills/grim-authoring/references/bundle-spec.md:86-100` — asserts `[hooks]` is not a bundle member table, contradicts the same file 40 lines earlier, and pastes a transcript that does not reproduce

The file states the correct model at `:39-56`: four member tables, `[hooks]`
among them, with a working example. Then at `:86-100` it states the opposite:

> **Three of grim's six artifact kinds cannot be bundled.** There is no
> `[mcp]` table and no `[hooks]` table (nor `[bundles]`), and because the
> parser is strict, writing one is a hard error rather than a silently
> dropped member:
>
> ```
> $ grim build ./hook-bundle.toml --kind bundle
> unknown field `hooks`, expected one of `skills`, `rules`, `agents`, `summary`,
> `keywords`, `description`, `license`, `repository`, `deprecated`, `replaced-by`
> ```
>
> Ship an MCP server or a hook as its own reference and let the consumer
> `grim add` it directly.

The pasted transcript is fabricated: `[hooks]` builds, and the accepted-field
list in the *real* error names `hooks`:

```
$ cat hookbundle.toml
summary = "test"
[skills]
a = "ghcr.io/acme/a:1"
[hooks]
h = "ghcr.io/acme/h:1"

$ grim build ./hookbundle.toml --kind bundle
Kind    Name        Path                 Layer Digest       Status
bundle  hookbundle  ./hookbundle.toml    sha256:e868ab78…   built

$ grim build ./mcpbundle.toml --kind bundle       # [mcp] instead of [hooks]
unknown field `mcp`, expected one of `skills`, `rules`, `agents`, `hooks`,
`summary`, `keywords`, `description`, `license`, `repository`, `deprecated`,
`replaced-by`
```

`src/command/build.rs:421-430` pushes the `Hook` members; `src/oci/bundle.rs:8`
names the accepted set as "(skill, rule, agent, mcp, hook)" on the wire.
Only **two** of six kinds cannot be a bundle *source* member (`mcp`,
`bundle`), not three.

The instruction that follows the false transcript ("Ship … a hook as its own
reference") tells a publisher to abandon a capability the tool has, so this is
the "instructions that fail" half of Block as well as the false-statement half.

Correction: delete `:86-100` outright (its content is already stated correctly
at `:53-56` and `:83`), or reduce it to the `[mcp]`/`[bundles]` exclusion with
the real `[mcp]` transcript. **actionable**

### B-7 — `catalog/skills/grim-authoring/references/bundle-spec.md:61` and `references/hook-spec.md:340` — a bundle-delivered hook faces "**per-hook** consent"; consent is per **registry**

> …then meets the same install-time gate a directly declared hook meets: the
> experimental feature flag first, then **per-hook consent**. (bundle-spec.md:61)

> …faces the identical install-time gate a directly declared hook faces — the
> feature flag first, then **per-hook consent**. (hook-spec.md:340)

There is no per-hook consent anywhere in the design. The second gate is the
per-**registry** `trust_hooks` field, and the interactive prompt is asked once
per registry and grants for every hook from it:

- `src/hook/trust.rs:509` — "Ask, **once**, whether hooks from `registry` may arm"
- `src/hook/policy.rs:32` — "a mutating command … prompts at most once per registry"
- `src/hook/policy.rs:36-38` — "`trust_hooks` on a `[[registries]]` entry is the **only** consent surface"
- `docs/src/configuration.md:842` — "grim asks — once, naming the **registry** rather than the artifact"

The same skill's own `SKILL.md:96` gets it right ("**Is the artifact's registry
trusted for hooks?** The per-registry `trust_hooks` field"), so this is drift
within one published package, not a considered disagreement. It is also the
reversed pre-D5 design the branch already corrected once in `513c685`.

Consequence for a reader: a publisher tells consumers to expect a prompt per
hook, and a consumer who granted trust for one hook from a registry silently
gets every later hook from that registry armed without a further prompt — the
opposite expectation, on the gate that governs code execution.

Correction: "the feature flag first, then the per-**registry** `trust_hooks`
grant, answered against the member's own source" in both files. The clause that
follows each occurrence ("the trust decision keys on the member's own source")
is already correct and should stay. **actionable**

---

## Warn

### W-1 — `docs/src/mcp-servers.md:323-325` — stale batch-publish kind order (hooks omitted)

> [`grim publish`](./commands.md#publish) releases entries in a fixed kind
> order: **skills → rules → agents → mcp → bundles**, alphabetical within
> each kind

The real order is **skills → rules → agents → mcp → hooks → bundles**
(`src/command/publish.rs:1874-1881`, `add_kind!` block sequence). The two other
places that state the order were updated on this branch and agree with the code
(`docs/src/publishing.md:910-912`, `docs/src/commands.md:1433-1434`,
`.claude/rules/subsystem-cli-commands.md:36`,
`catalog/skills/grim-authoring/references/release-checklist.md:77`); this one
was missed, and it is the copy a reader lands on from the MCP page.

Correction: insert `→ hooks` between `mcp` and `bundles`. **actionable**

### W-2 — `catalog/skills/grim-usage/references/consume.md:461-465` — "in global config an unset value means trusted" omits the two exceptions that make it false

> The per-registry `trust_hooks` field is a tri-state with a deliberate asymmetry:
> in **global** config an unset value means trusted, because configuring a
> registry there is itself the trust act, and `false` opts it out.

An unset `trust_hooks` on a **global** entry grants only when the entry names a
namespace, is an `oci` locator, and is not `insecure`. `src/hook/trust.rs:324-327`:

```rust
granted |= entry.scope == ConfigScope::Global
    && entry.kind == LocatorKind::Oci
    && (!is_bare_host(entry.locator) || explicit_grant)
    && (!entry.insecure || explicit_grant || is_loopback(entry.locator));
```

So a global `oci = "ghcr.io"` (bare host — the single most common way a user
configures a registry), a global `index = "…"` entry, and a global
`insecure = true` non-loopback entry all grant **nothing** with `trust_hooks`
unset. A reader who follows this skill would expect their global `ghcr.io`
entry to arm hooks and get the consent prompt instead — or, in CI, `gated`.

The unit test named for it passes:

```
$ cargo test --release --bin grim bare_host
test hook::policy::tests::a_bare_host_and_an_index_entry_never_grant_implicitly ... ok
test result: ok. 7 passed; 0 failed
```

`docs/src/configuration.md:828-836` states all three exceptions correctly, and
`grim config registry fields` states the bare-host one; the published skills are
the outliers. The same unqualified sentence appears a second time in
`catalog/skills/grim-usage/references/troubleshooting.md`, step 2 of **"A Hook
That Never Fires"** — "In *global* config an unset value means trusted" — which
is the worse of the two placements: it is a diagnostic checklist for a user
whose hook is not arming, and a bare-host `ghcr.io` entry is the single most
likely cause it walks them straight past.

Correction: add the namespace qualifier and the `index` exception, mirroring
`docs/src/configuration.md:814` ("Grants, provided the entry names a namespace
rather than a bare host"). **actionable**

### W-3 — `catalog/skills/grim-authoring/references/hook-spec.md` — the `id` charset and length limit are undocumented, and absent from the Validation Pitfalls table that claims to cover build failures

`a9a115f` added rule 10: an `id` is bounded to `HOOK_ID_MAX_BYTES = 128`
(`src/oci/hook.rs:131`) over ASCII letters, digits, `_`, `-` and `.`
(`src/oci/hook.rs:693-700`), enforced at `grim build` **and** re-checked at the
install seam. Neither `hook-spec.md`'s `[[hooks]]` field table nor its
16-row "Validation Pitfalls" table mentions it, and `docs/src/artifacts.md:422`
describes `id` only as "Stable id, unique within the artifact".

The `matcher` charset sits in the same tables and *is* documented, which makes
the omission asymmetric rather than a uniform level of detail.

```
$ grim build ./badid --kind hook       # id = "log:tool/call"
invalid hook id 'log:tool/call': expected only ASCII letters, digits, '_', '-' and '.'
$ echo $?
65
```

Correction: add the charset + 128-byte limit to the `id` row in both field
tables, and a pitfall row with the message above. **actionable**

### W-4 — a documented hook refusal reports the wrong kind: "path sources are not supported for **mcp** artifacts"

`catalog/skills/grim-usage/references/consume.md` states, correctly, that
"**`[hooks]` accepts registry references only** — a path value there is
refused", and `hook-spec.md:317` explains why ("A hook has no local-path
source"). The refusal fires — but names the wrong kind:

```
$ printf '\n[hooks]\nlocal-hook = "./myhook"\n' >> grimoire.toml
$ grim status
grimoire.toml: artifact 'local-hook': path value './myhook' is not usable:
  path sources are not supported for mcp artifacts
```

`src/config/project_config.rs:883` hardcodes the string `"mcp artifacts"` for
every `PathValues::Rejected` map, and `[hooks]` is now a second such map
(`src/command/add.rs:492` builds the same sentence from the real kind, so the
two paths disagree).

A user who followed the doc reads a message about a kind they never declared.
The doc is true; the diagnostic contradicts it.

Fix is in code, not in the docs — **deferred** to the code reviewer
(`src/config/project_config.rs:883`, `src/config/config_error.rs:240`).

### W-5 — `catalog/skills/grim-usage/references/consume.md:417` vs. `catalog/skills/grim-authoring/references/mcp-spec.md:161-167` — "an MCP server is not bundleable" is true of the authoring parser only, and one of the two files says so

`consume.md:416-418` — "A bundle's member tables are `[skills]`, `[rules]`,
`[agents]`, and `[hooks]` — **an MCP server is not bundleable.**"

`mcp-spec.md:161-167` states the precise version added on this branch:

> (The restriction is the authoring parser's, not the wire format's: a bundle
> whose members document names an `mcp` member does resolve, so do not read
> this as "the resolver rejects mcp members".)

That qualification is correct — `src/oci/bundle.rs:8` lists `mcp` among the
member kinds a bundle layer may carry, and `ArtifactKind::Mcp` flows through
`BundleMember` unrestricted; only `read_bundle_members`
(`src/command/build.rs:395-431`) has no `[mcp]` loop, and `BundleSource`'s
`deny_unknown_fields` rejects the table. So the flat "not bundleable" in
`consume.md` (a *consumer*-facing file, where the resolver behaviour is what
matters) is the imprecise one.

Correction: carry `mcp-spec.md`'s parenthetical into `consume.md`, or soften to
"a bundle you author with grim cannot list an MCP server". **actionable**

### W-6 — `docs/src/artifacts.md:464` heading "The two gates" vs. the catalog's "three independent questions" — same model, two incompatible counts, in the two places a reader compares

`docs/src/artifacts.md` says **two** gates (`:395` "two deliberate opt-ins",
`:464` heading, `:478` "Until both pass") and treats `--allow-hooks` as an
escape stated afterwards. `catalog/skills/grim-usage/references/consume.md:435`
and `catalog/skills/grim-authoring/SKILL.md:86` say **three** independent
questions, `--allow-hooks` being the third.

Both descriptions are individually faithful to
`src/hook/trust.rs:363-379` — arming is `feature_enabled && (opted_out ? no :
allow_hooks || trusted || consent)`, so the flag is one of three satisfiers of
the second conjunct, not a third conjunct. Neither count is false. But a reader
who has read both is left unable to say how many gates there are, on the one
surface where the exact shape of the gate matters, and `catalog/hooks/README.md:123`
("Two independent gates, on purpose") lands on the docs side while its own
walkthrough uses `--allow-hooks` as the CI route.

Correction: pick one framing and use it in all three (the catalog's "three
questions, and the third does not enable the feature" is the one that survives
a CI reader). Not Block: no statement is false, and no user is misled about
what to type. **actionable**

### W-7 — three files show `grim config get options.experimental.hooks` as printing `false`; it prints nothing and exits 1

- `catalog/skills/grim-authoring/SKILL.md:88` — `grim config get options.experimental.hooks    # false unless someone set it`
- `catalog/skills/grim-usage/references/consume.md:456` — same command, same comment
- `catalog/skills/grim-usage/references/troubleshooting.md` — "A Hook That Never
  Fires": `grim config get options.experimental.hooks        # exit 1 = never set; prints false = off`

The flag is an emit-only-when-true boolean: `config set … false` **removes the
key** rather than writing `false`, so `config get` can never report `false`.

```
$ grim config get options.experimental.hooks          # never set
$ echo $?
1

$ grim config set options.experimental.hooks false >/dev/null
$ cat grimoire.toml | tail -4
#:schema https://grimoire.rs/schemas/grimoire-config.schema.json
[skills]

[rules]

$ grim config get options.experimental.hooks          # explicitly false
$ echo $?
1
$ grim config get options.experimental.hooks --format json
{ "key": "options.experimental.hooks", "value": null, "set": false, "scope": "project" }
```

`trust_hooks` behaves the *opposite* way and the docs are right about that one —
`docs/src/configuration.md:807-810` says it "writes all three states — including
`false`, which round-trips rather than being dropped as an emit-only-when-true
boolean would be", and it does:

```
$ grim config set registry.r.trust_hooks false >/dev/null
$ grim config get registry.r.trust_hooks
false
$ echo $?
0
```

So the troubleshooting annotation is literally false for the flag and correct
for `trust_hooks`, on two adjacent lines of the same block.

Not Block: the reader's *conclusion* at that step is unchanged (no output still
means "not on", and the remedy is the same `config set … true`). The failure mode
is a consumer or agent that scripts on the printed value — it gets an empty
string and exit 1 where the doc promised the token `false`.

Correction: `# prints nothing and exits 1 unless it is on` on the flag line;
leave the `trust_hooks` line as it is. **actionable**

### W-8 — `.claude/rules/subsystem-cli-commands.md:39` (`grim schema` row) — the ⚠ note it carries is itself stale

> ⚠ The subcommand's own clap `about` string still reads "grimoire.toml,
> publish.toml, or grimoire.lock" — it omits both `mcp` and `hook`.

It does not, as of `05aac01` (`docs(cli): name every kind grim schema accepts`)
on this branch:

```
$ grim schema --help | head -1
Print the JSON Schema for grimoire.toml, publish.toml, grimoire.lock, mcp/<name>.toml, or hook.toml

$ grim schema -h | head -1
Print the JSON Schema for grimoire.toml, publish.toml, grimoire.lock, mcp/<name>.toml, or hook.toml
```

A ⚠ marker on a rule file is read as live work; leaving one that has been
discharged trains the next reader to skip them.

Correction: delete the ⚠ clause. **actionable**

### W-9 — `docs/src/publishing.md:90-91` — "the tree-backed kinds (skill, rule, agent) but not mcp or bundle" leaves `hook` unplaced, and a hook *is* tree-backed

> The in-tree READMEs above ride each artifact's own layer, so they cover the
> tree-backed kinds (skill, rule, agent) but not mcp or bundle.

A hook is a directory artifact packed wholesale, so a `README.md` at its root
rides the layer like a skill's:

```
$ grim build ./rdhook --kind hook --format json   # no README.md present
sha256:e6a6901f39e5953d8bb13bae2490118c65110b4f19c5f0f05cd759a12fc56a34

$ printf '# rdhook\n\nreadme body\n' > ./rdhook/README.md
$ grim build ./rdhook --kind hook --format json   # README.md added
sha256:3e7a89608ed0bb4085228cddb44ec4a3f05b9a271aa35f8d9eeeaacd6e29b12b
```

The preceding paragraph (`:85-86`, "MCP servers and bundles publish a single
JSON layer with no file tree of their own") is correct and complete for the two
kinds it names; it is the six-of-five enumeration in the sentence after it that
is wrong.

Correction: "(skill, rule, agent, hook)". **actionable**

### W-10 — the `GRIM_HOOK_*` environment allowlist a handler reads is documented only inside a shipped example script

`src/command/hook/*.rs` exports nine variables to a handler —
`GRIM_HOOK_CLIENT`, `GRIM_HOOK_CWD`, `GRIM_HOOK_DIR`, `GRIM_HOOK_EVENT`,
`GRIM_HOOK_NAME`, `GRIM_HOOK_PAYLOAD`, `GRIM_HOOK_SCHEMA`, `GRIM_HOOK_TIER`,
`GRIM_HOOK_TOOL`. Only two are documented: `GRIM_HOOK_DIR`
(`hook-spec.md:198-251`) and `GRIM_HOOK_PAYLOAD` (`docs/src/artifacts.md:430`,
`hook-spec.md:109`). The remaining seven appear nowhere in `docs/src/**` or
`catalog/skills/**` — grep over both trees returns only the two above.

They are not obscure: five of them are what the shipped `observer` example
actually uses, and `catalog/descriptions/tool-call-logger.md` publishes the
resulting line as the artifact's documented output —

```
$ printf '%s' '{"hook_event_name":"PreToolUse",…}' | sh log.sh   # verified: {} on stdout, exit 0
PreToolUse client=claude tool=Bash hook=tool-call-logger/log-tool-call tier=observer
```

— so the catalog invites a publisher to copy a script whose interface has no
reference page. `catalog/hooks/README.md:5` says the examples "exist to be
**read and run**", which makes the example the de-facto spec.

Correction: add the allowlist as a table in `docs/src/artifacts.md`'s hook
section (and a pointer from `hook-spec.md`), stating for each whether it is
always set and what it holds when the event has no tool. **actionable**

---

## Suggest
### S-1 — `src/command/publish.rs:9`, `:858`, `:3478` — three rustdoc comments still state "skills → rules → agents → bundles", omitting both `mcp` and `hooks`

`:1809` (`plan_entries`) states "skills → rules → agents → mcp → bundles" and
also omits hooks, while the code beneath it at `:1874-1881` adds `Mcp`, then
`Hook`, then `Bundle`. Outside the diff scope I was given (`src/**`), noted
because it is the same drift as W-1 and the next reader of that module will
copy it into a doc. **deferred**

### S-2 — `docs/src/commands.md:315` — the `grim add` signature omits `--allow-hooks`, which the prose two paragraphs down says is accepted

`grim add [--kind <skill|rule|agent|bundle|mcp|hook>] [--name <name>] [--no-install] [--force] <reference>`
— `--allow-hooks` is on the command (verified: `grim add --help` lists it) and
`:326-327` says so. `.claude/rules/subsystem-cli-commands.md:20` shows it in the
signature. **actionable**

### S-3 — `catalog/skills/ai-config-authoring/references/choosing-types.md:50` — "Claude Code, OpenCode, Copilot, Codex … unsurveyed for the other six" undersells how many clients have an upstream hook surface

The row is scoped to that file's own ten-client survey, so it is not false. But
`docs/src/clients.md` `{#gap-hooks}` now names **fifteen** clients with an
upstream hook mechanism (twelve declined on grim's schedule, three armed), and
this file is the one an author reads when *choosing* between a skill and a hook.
Pointing the row at `{#gap-hooks}` would keep it honest without re-surveying.
**actionable**

### S-4 — `docs/src/artifacts.md:481-484` states the no-`GRIM_ALLOW_HOOKS` decision without its reason

> …is the per-invocation escape, and there is deliberately no environment
> variable that does the same thing.

The reason is stated well at `docs/src/configuration.md:1038-1047` and in
`AGENTS.md`, and the artifacts page links neither. One clause ("because a
repository carries its own environment") or a link would stop a reader reading
it as an unfinished feature. **actionable**


---

## Verified true (checked and found correct — recorded so round 2 does not re-check)

**Closed enumerations that are correct.**

- `AGENTS.md:18` and `.claude/rules/product-context.md:101` — "23 subcommands".
  `grim --help` lists 24 rows including `help`, i.e. 23 real subcommands.
- `grim schema --kind` value set (`config`, `publish`, `lock`, `mcp`, `hook`) —
  matches `docs/src/commands.md`, `subsystem-cli-commands.md`,
  `grim-authoring/references/updating.md`.
- `--kind` value sets per command, all six kinds where documented:
  `add`/`build`/`release`/`remove` → `skill, rule, agent, bundle, mcp, hook`;
  `uninstall` → `skill, rule, agent, mcp, hook` (no `bundle`), which matches
  `docs/src/commands.md`'s `uninstall` text.
- `--allow-hooks` exists on exactly `install`, `update`, `add` and nowhere else
  (`lock`, `status`, `remove`, `uninstall` → 0 hits), as
  `docs/src/commands.md:487` and `subsystem-cli-commands.md` state.
- Batch-publish order in `docs/src/publishing.md:908-921`,
  `docs/src/commands.md:1433`, `subsystem-cli-commands.md`, and
  `release-checklist.md:77` — all read `skills → rules → agents → mcp → hooks →
  bundles`, matching `src/command/publish.rs:1874-1881`. (`docs/src/mcp-servers.md`
  is the one outlier — W-1.)
- `docs/src/artifacts.md:23-30` kind table (six rows) and `:42-50` inference
  rules. `:42` correctly scopes inference to "`grim build` and `grim release`" —
  `grim add <path>` does *not* infer `hook`, and the doc does not claim it does.

**The three arming gates.**

- The `[options.experimental] hooks` flag, the per-registry `trust_hooks`
  tri-state, and per-invocation `--allow-hooks` are described consistently and
  correctly in `docs/src/configuration.md:812-850`, `docs/src/commands.md:485-499`,
  `docs/src/artifacts.md:464-484`, and `catalog/skills/grim-authoring/SKILL.md:85-108`.
  `docs/src/configuration.md:812-836` is the most complete and matches
  `src/hook/trust.rs:308-334` exactly — `false` in either scope wins, a project
  entry may only restrict, a bare host never grants implicitly, an `index` entry
  never grants, an `insecure` entry needs an explicit `true` with loopback exempt.
- **There is no `GRIM_ALLOW_HOOKS` and no doc implies one.** Grep over `docs/`,
  `catalog/`, `.claude/`, `AGENTS.md` finds only statements that it does not
  exist. The absence is documented as a decision *with its reason* at
  `docs/src/configuration.md:1038-1047` ("a repository routinely carries its own
  environment … would let a repository grant itself trust on the machine of
  whoever cloned it"), `AGENTS.md:108` (naming the withdrawing commit and date),
  `grim-authoring/SKILL.md:105-109`, and `consume.md:445-450`. Requirement met.
- `trust_hooks` visibility, exactly as `docs/src/configuration.md:855-863` and
  `docs/src/stability.md:363-368` claim:

  ```
  $ grim config get registry.internal.trust_hooks      → true          (reads it)
  $ grim config list --all | grep trust                → registry.internal.trust_hooks  true
  $ grim config registry fields                        → documents trust_hooks
  $ grim config registry show internal                 → no trust_hooks column
  $ grim config registry list                          → no trust_hooks column
  $ grim context | grep -ic trust                      → 0
  ```

- Clearing the flag does not disarm, and both routes warn — as
  `docs/src/stability.md:373-385` and `catalog/hooks/README.md:265-276` state:

  ```
  $ grim config set options.experimental.hooks false
  WARN `grim config set options.experimental.hooks false` cleared the feature flag, but hooks
       already armed stay armed until convergence runs — run `grim install` to disarm them
  $ grim config unset options.experimental.hooks
  WARN `grim config unset options.experimental.hooks` cleared the feature flag, but hooks
       already armed stay armed until convergence runs — run `grim install` to disarm them
  ```

**`catalog/hooks/README.md`** (not re-run wholesale, per instruction — checked
for stale claims):

- `:284-297` "Two gaps this walkthrough found, both since fixed" — both fixes
  are present (`9f0a02f` populates `grim hook list`; `0a51be5` permits the
  config write, warning shown above).
- `:113` `Note: claude: feature-flag-off` is the real plain-format cell —
  `src/api/status_report.rs:252-261` renders `"{client}: {cause}"`, and
  `feature-flag-off` is the token (`src/api/artifact_status.rs:255`).
- `:152-165` and `:248-252` dispatch-table shape (`roots` → token →
  `{root, hooks:[{artifact, id, client, event, tier, matcher}]}`) matches
  `src/install/hook_dispatch.rs:503-614` field for field.
- `:30-33` `argv`-not-`command` and `:306` no-`--kind` inference both reproduce:

  ```
  $ grim build catalog/hooks/command-guard          # no --kind
  Kind  Name           Path                         Status
  hook  command-guard  catalog/hooks/command-guard   built

  $ grim build ./cmdform --kind hook                # command = "./guard.sh"
  hook 'x' runs the payload file './guard.sh' directly; a payload delivered through a
  registry is not executable — name an interpreter instead, e.g. argv = ["sh", "${GRIM_HOOK_DIR}/./guard.sh"]
  ```

- **Teardown genuinely disarms.** `grim uninstall` reaps the dispatch row, and an
  empty `roots[*].hooks` is the whole of arming: every registration's command is
  `"$L" run … --table … --root <token>`, and `grim hook run` with a token no root
  matches exits 0 having run nothing (verified below). The launcher surviving is
  harmless and is why `:237` lists three surfaces rather than four.
- Stale-claim caveat: `:123` "Two independent gates" is the docs-side framing —
  see **W-6**.

**`docs/src/clients.md`** — prose around the machine-checked cells:

- `{#gap-hooks}`'s arithmetic is internally sound and matches the table I
  extracted: 18 rows; Hook column `✓ ✓ ◐` for Claude/Codex/Copilot and `✗` for
  the other 15; the 15 split 2 (no upstream surface) + 1 (`agents`, not a
  client) + 12 (grim schedule), and the 12 are named individually.
- `{#gap-hooks-scope}` — Codex and Copilot global-only is real
  (`src/install/expected_outputs.rs:293-306` "…has no project-scope hook
  surface", `vendor_codex.rs:439`, `vendor_copilot.rs:448`), and Claude's
  exemption via `.claude/settings.local.json` matches
  `src/install/hook_registrar.rs:99`.
- `{#gap-hooks-ask}` — verified against `src/command/hook/projector.rs:263-297`:
  an inexpressible `ask` is written using the row's `deny` token, the author's
  reason still travels (`:302-318`), and a `tracing::warn!` records it. The four
  verdicts (`allow`, `deny`, `ask`, none) are `Decision`'s four variants
  (`src/command/hook/pipeline.rs:87-118`).
- The new scope-blindness and consent paragraphs at `:20-38` are accurate.
- **Not verifiable here:** every claim about what a *vendor* documents upstream
  (the twelve "documents its own hooks mechanism", the three shapes). No network
  access was used; `.claude/rules/vendor-capability-watchlist.md` date-stamps
  them `verified 2026-08-17` and is the right home for them.

**`hook-spec.md`'s error transcripts.** Every message quoted in its Tiers,
Reserved Names, and 16-row Validation Pitfalls tables reproduces verbatim, and
every one exits **65** as the table's header claims. Checked individually:
`mutator` at `PostToolUse`; `gatekeeper` at `SessionStart`; native-only
`mutator`; reserved `bin` / `dispatch.json` / `payload`; `summary` at top level;
`^Bash$` matcher; empty matcher; `schema = 2`; `clod.event`; duplicate `id`;
`command = "./guard.sh"`. (The two *counts* in that file are wrong — B-2, W-3.)

**`docs/src/commands.md`'s `grim hook run` section.** The fail-open contract is
real on all four documented paths:

```
$ grim hook run --client claude --event PreToolUse --table $D/dispatch.json --root deadbeef
WARN dispatch table … was not usable (UnknownSchema); no hook ran
$ echo $?  → 0

$ grim hook run … --table ./dispatch.json --root x
WARN grim hook run: the --table path is not absolute … nothing was read and no hook ran
$ echo $?  → 0

$ grim hook run … --table $D/nope.json --root x        → (silent)  exit 0
$ grim hook run … --event Nonsense …
WARN grim hook run: the --event value names no lifecycle event this grim understands
$ echo $?  → 0
```

`grim schema --kind hook | jq .title` → `hook.toml — Grimoire hook manifest`,
as `docs/src/commands.md` shows.

**`catalog/descriptions/*.md`** (the published package blurbs) — both are
honest, including the self-incriminating parts. `command-guard.md`'s claimed
outputs are exact, and its documented evasion really evades:

```
$ echo '{…"command":"ls -la"}'                | sh guard.sh  → {}                                    exit 0
$ echo '{…"command":"rm -rf / --no-preserve-root"}' | sh guard.sh
{"decision":"deny","reason":"command-guard (a demonstration hook) refused a command containing the literal rm -rf /"}   exit 0
$ echo '{…"command":"rm -fr /"}'              | sh guard.sh  → {}                                    exit 0
```

`tool-call-logger.md`'s line format and its `timeout = 5` claim both match
`log.sh` and `hook.toml`.

**Anchors, links, and rule-catalog sync.**

- `mdbook` is **not installed** in this environment, so no render check was
  performed and none is implied. Substitute: every heading anchor in
  `docs/src/**` (explicit `{#…}` plus the derived slug) extracted, then every
  intra-book link resolved against it — **21 files, 0 broken file targets, 0
  broken fragments**, including the five link definitions
  `docs/src/stability.md` added (`hook-manifest`, `hook-gates`, `trust-hooks`,
  `hook-list`, `options-experimental`) and the new `{#gap-hooks-ask}`.
- `docs/src/SUMMARY.md` lists all 20 content pages; no orphan, no dangling entry.
- `.claude/rules.md` is in sync with `.claude/rules/`: the new
  `arch-threat-model.md` row, its auto-load path row, and both declared overlap
  groups match the file's own `paths:` frontmatter.
  `task claude:tests` → **51 passed**, including
  `test_path_overlaps_declared_or_absent`.

**Other spot checks that held.** `docs/src/json-interface.md`'s `hook list` item
shape and `hook_arming` example match `src/api/`; `docs/src/concepts.md`'s
bundle-member and precedence text; `docs/src/package-index.md`'s `kind` value
list; `docs/src/vendor-metadata.md`'s "`hooks` is not in this registry, and it
never will be"; `subsystem-file-structure.md`'s reserved-name three-site split
and its `$GRIM_HOME/hooks/` layout table.
