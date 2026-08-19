# WP-M — documentation for the hook artifact kind

**Branch** `hex/hooks-artifact-kind--wp5-m`, based on `31a9154`.
**Commits** `75dc8e8` (`docs:`, 10 doc pages) and `7b3a982`
(`chore(agents):`, 5 agent-context files).
**Gates** `task verify` → exit 0, 1071 passed / 1 xfailed. `task claude:tests`
→ 51 passed. `task catalog:verify` → all 4 packages built. `test_docs.py` → 21
passed. Hook acceptance suites (`test_hook_arming.py`,
`test_hook_run_runtime.py`, `test_bundle_hook_members.py`) → 52 passed / 1
xfailed. Custom anchor checker over `docs/src/**` → 0 broken targets.
Both `uv.lock` files reverted; `commit-verified` was written by
`task verify`'s own `.verify:mark`, never by hand.

---

## ⛔ The brief was wrong on the gate count — `--allow-hooks` ships

Reported to the lead mid-task, restated here because it shaped every file.

The instruction was *"there are TWO gates, not three; `--allow-hooks` does not
exist — verified zero hits."* That was true of an earlier tree. **WP-R landed
the flag in `fccfa05`, an ancestor of my base `31a9154`.** Evidence:

* `src/command/install.rs:62`, `src/command/update.rs:76`,
  `src/command/add.rs:94` — the clap field on all three commands.
* `src/hook/trust.rs:264` — `ArmingQuery::allow_hooks`.
* `.agents/wp-r-report.md:419` — *"DECISION: shipped (option 1), and it is
  already in this commit."*
* Executed: `grim add --help` and `grim install --help` both print the flag.

WP-R's reasons for shipping rather than dropping: C-023 leaves a CI user no
other route, and the **already-merged** prompt text at `trust.rs:611` names the
flag, so shipping neither would have left a user-facing prompt naming a
nonexistent flag — the same ⛔ from the other direction.

`GRIM_ALLOW_HOOKS` is the half that genuinely does not exist, and the CWE-426
reasoning in the brief applies exactly to it. **So I documented three arming
routes** (feature flag, `trust_hooks`, `--allow-hooks`) and stated the absence
of an environment form as a decision in three places.

**Routed out, not mine:** WP-R:445 warns that WP-N removed the third gate from
`catalog/**` on the evidence it did not exist. There are three again, so the
catalog rows need it restored. `catalog/**` is excluded from my scope.

---

## Everything documented was executed first

The gate matrix was **not** taken from the plan. `src/hook/trust.rs::decide`
requires, for a grant: `scope == Global` **and** `kind == Oci` **and**
(not bare-host **or** explicit `true`) **and** (not `insecure` **or** explicit
`true` **or** loopback); a matching `trust_hooks == Some(false)` is an early
return that beats everything. Verified per term:

| Case | Global entry | Result | Dispatch rows |
|---|---|---|---|
| V1 | bare host `localhost:5000`, `insecure`, unset | **declined** | 0 |
| V2 | bare host, no `insecure`, unset | **declined** | 0 |
| V3 | bare host, `insecure`, `trust_hooks = true` | **installed** | 1 |
| V4 | namespaced `localhost:5000/wpm-demo`, `insecure` (loopback), unset | **installed** | 1 |
| V5 | **project**-scope entry, `trust_hooks = true` | **declined** | 0 |
| V6 | global `true` + project `false` | **declined** | 0 |

V1/V2 initially looked like "unset does not grant in global scope", which would
have contradicted the plan and the shipped `config registry fields` help text.
Reading `decide` before writing showed the real cause: `localhost:5000` is a
**bare host** (B5), not an `insecure` problem. V4 isolates it — a namespaced
loopback entry with `trust_hooks` unset **does** grant. **The plan's B4/B5 are
correct; my first reading was not.** Recording this because the wrong version
would have shipped a false rule into a released doc.

`trust_hooks = false` beats `--allow-hooks`: `grim install --allow-hooks`
against a global `false` printed *"hook 'shell-guard' not installed: this
registry declares `trust_hooks = false`"*, 0 dispatch rows.

Other executed output backing what I wrote:

* `grim build ./shell-guard` → `hook shell-guard … sha256:272331f3… built`,
  **with no `--kind` flag** (`build.rs:74` tests `hook.toml` before
  `SKILL.md`, both under `is_dir()`). The `artifacts.md` example prints this
  real digest.
* `grim schema --kind hook` → `hook.toml — Grimoire hook manifest`; every
  field table in `artifacts.md` is transcribed from that schema plus
  `src/oci/hook.rs` (`MATCHER_ALLOWED = "A-Za-z0-9_*?./-|"`,
  `MATCHER_MAX_BYTES = 256`, `DEFAULT_TIMEOUT_SECS = 30`,
  `RESERVED_POLICY_KEY = "policy"`).
* `grim status --format json` on a declared hook → `state: gated` with
  `arming[0].cause` = `feature-flag-off`, then `registry-not-trusted`.
* `grim config registry fields --format json` → **7** items, `trust_hooks`
  last.
* `grim config set registry.<a>.trust_hooks true|false --global` → exit 0,
  and `false` **round-trips into the file** (B7's requirement, confirmed at
  `grimoire.toml:4`).
* `grim hook run --root <unknown>` → exit **0**, silent.
  `--table relative/path` → one WARN line, exit **0**.
* `grim --help` → **23** subcommands.

### Bundle-delivered hooks — both claims in `artifacts.md` executed

Added after the lead flagged that WP-N's catalog copy carried the reversed
pre-D5 "per-hook consent" design. **`docs/**` never carried that claim** —
`artifacts.md:528` already said the member's own registry governs — but the
claim was unverified when written, so I executed both halves.

**Disclosure fires.** A bundle declaring `[hooks]` locks the member, and
install emits exactly one line before any verdict:

```
WARN grim::command::hook_consent: hook 'shell-guard' is delivered by bundle
  'localhost:5000/wpm-demo/bundles/with-hook:1' from registry 'localhost:5000'
```

`grim status` then reports the hook row as
`source: "bundle: localhost:5000/wpm-demo/bundles/with-hook"`, `installed`,
with 1 dispatch row.

**Laundering is closed.** Bundle published into the **granted**
`localhost:5000/wpm-demo` namespace (`trust_hooks = true`); its hook member
pinned to `localhost:5000/untrusted-ns`, which has **no entry at all**:

| Outcome | Value |
|---|---|
| bundle | `installed` |
| hook member | `skipped` |
| dispatch rows | **0** |
| `status` | `state: gated`, `cause: registry-not-trusted` |
| disclosure line | still emitted |

So a trusted bundle does not confer trust on a member pinned elsewhere, and
the disclosure is genuinely disclosure rather than a consent surface.

**A self-correction worth recording, because it confirms B4.** My first
attempt at this test *did* arm the member, which looked like laundering. The
test was wrong, not the code: I had given `untrusted-ns` its own
`[[registries]]` entry to label it, and a **global, namespaced, loopback**
entry grants by itself under B4 even with `trust_hooks` unset. Removing the
entry produced the result above. Configuring a registry in global config is
the trust act, exactly as the plan says — and it is easy to defeat your own
test by writing one.

---

## Enumerations fixed, with sites

Line numbers are pre-edit, from the committed base.

| File:line | What was closed | Fixed to |
|---|---|---|
| `docs/src/artifacts.md:3` | "ships five artifact kinds" | six, hooks named |
| `docs/src/artifacts.md:17` | `## The five kinds {#kinds}` | `## Artifact kinds {#kinds}` — **count-neutral**, anchor preserved |
| `docs/src/artifacts.md:23-29` | 5-row per-kind table | Hook row added (a renamed heading over an unchanged table still reads as five) |
| `docs/src/artifacts.md:40-46` | "a directory is a skill" | directory is a skill **or** a hook, told apart by index file |
| `docs/src/artifacts.md:393-395` | bundle member tables | `[hooks]` added |
| `docs/src/artifacts.md:406` | "must be a skill, rule, or agent" | "…or hook" |
| `docs/src/commands.md:35` | `add` table row kinds | + hook |
| `docs/src/commands.md:52` | `schema` row "grimoire.toml or publish.toml" | "one of grim's TOML formats" |
| `docs/src/commands.md:313` | `add --kind <skill\|rule\|agent\|bundle\|mcp>` | + `hook`, + `--allow-hooks` |
| `docs/src/commands.md:801` | `remove <kind>` set | + hook |
| `docs/src/commands.md:820` | `uninstall <kind>` set | + hook |
| `docs/src/commands.md:1323` | `build --kind` set | + hook, + no-flag inference |
| `docs/src/commands.md:1379` | publish kind order | + hooks, before bundles |
| `docs/src/commands.md:1476,1489` | `schema --kind <config\|publish\|lock\|mcp>` ×2 | + `hook` in prose **and** the example block |
| `docs/src/json-interface.md:97-101` | `kind` five-value contract | six values |
| `docs/src/json-interface.md:89` | `state` five tokens | + `gated`, `not-armed`, `untrusted` |
| `docs/src/json-interface.md:94` | `registry fields` six keys | + `trust_hooks` |
| `docs/src/package-index.md:117` | `kind` five values | + `hook` |
| `docs/src/publishing.md:558` | bundle author tables | + `[hooks]` |
| `docs/src/publishing.md:898-902` | conventional-path table | + `hook` → `hooks/{name}/` (verified: `publish.rs:1785`) |
| `docs/src/publishing.md:907-909` | kind-order block | + hooks + why appended |
| `docs/src/concepts.md:8-18` | "three kinds" / "a fourth kind" | six-kind narrative, hooks framed by their consent model |
| `docs/src/concepts.md:107,149` | bundle members / precedence list | + hooks |
| `docs/src/configuration.md:6,18-38` | `[skills]/[rules]/[agents]` | + `[hooks]` + example |
| `docs/src/configuration.md:153` | bundle expansion list | + hooks |
| `docs/src/vendor-metadata.md:187-188` | hooks carve-out ("a separate ADR governs") | resolved-as-its-own-kind |
| `docs/src/vendor-metadata.md:228-229` | second carve-out | same, both surfaces named |
| `docs/src/mcp-servers.md:304` | inbound link text "The five kinds" | "Artifact kinds" (anchor unchanged) |
| `.claude/rules/subsystem-cli-commands.md` | 6 registry fields | **7** + tri-state emit rule |

`docs/src/stability.md:31` already read "Artifact kinds" and `:388`'s
definition targets `#kinds`, so the count-neutral rename kept **both** inbound
links working with no edit — verified by the anchor checker.

---

## Gate count documented, and why

**Three arming routes, two of them persistent gates.** Documented as: the
`[options.experimental] hooks` feature flag (per scope, off by default), the
per-registry `trust_hooks` tri-state, and `--allow-hooks` as a
**per-invocation** escape that does not enable the flag, is never persisted,
and loses to an explicit `trust_hooks = false`. Not two, because the third
exists in the shipped binary; not four, because `GRIM_ALLOW_HOOKS` does not
exist and I documented its absence as a decision rather than adding a row.

---

## Left out pending the payload relocation

`.claude/rules/subsystem-file-structure.md`'s new `### Hooks` section
deliberately **omits the payload directory's location**, with an inline note
saying why and what to add when SEC-1 lands. Measured current value, for
whoever routes it: `<workspace>/.grimoire/hooks/<artifact>/` (project scope) —
the workspace-resident location SEC-1 is moving under `$GRIM_HOME`.

What I *did* document, because it is not moving: `$GRIM_HOME/hooks/` holds
`dispatch.json`, `bin/grim-hook`, and `root-key`. **`root-key` is a third
file the brief did not mention** — observed on disk.

`docs/src/artifacts.md` says a hook installs as "a shared payload directory
plus a registration" without naming the path, for the same reason.

---

## Findings — things in the plan or the brief that are wrong

**1. `grim hook list` is still a stub, and its own removal trigger is stale
(Block for the docs).** `src/command/hook/list.rs:53-68` returns
`HookListReport::new(Vec::new())` unconditionally. Its comment says *"REMOVAL
TRIGGER: replace this … once a hook can be installed"* — WP-J2/WP-R landed, so
the premise is false and the stub was never replaced. Executed: with an
`installed` hook and **1 dispatch row present**, `grim hook list --format json`
still returns `{"items": []}`. The plan told me to "keep the user-facing
surface on `grim hook list`", so I documented it as the supported verb **and**
stated plainly that it reports an empty inventory in this release, pointing
readers to `grim status`. This is strictly worse than WP-R's F-2 and I could
not find it recorded anywhere.

**2. WP-R's F-2 confirmed, exit code included.** A hook armed via
`--allow-hooks` reports `state: gated`, `cause: registry-not-trusted`, while
the dispatch row and the Claude registration both exist. Documented as an
under-claim in the safe direction.

**3. WP-R's F-3 confirmed as exit 65.** `config set options.experimental.hooks
false` and `config unset` are both refused with **exit 65** (measured unpiped;
a first measurement through `tail` reported 0 and was wrong). Documented with
the hand-edit-plus-`grim install` workaround, which WP-R verified does disarm.

**4. `trust_hooks` visibility is uneven, and the brief's "invisible in nearly
every report surface" is close but not exact.** Measured: `config get`,
`config list --all`, and `config registry fields` **do** expose it;
`config registry show`, `config registry list`, and `context` do **not**.
`config registry set` has no `--trust-hooks` flag. Documented as measured.

**5. Copilot's `◐` is not the mutator tier — the plan's WP-M instruction
misattributes it.** I was told to add a watchlist entry "for Copilot `mutator`
(Open Question 2)". `src/install/vendor_copilot.rs:97` says **"`mutator` is
supported, not declined (ADR Open Question 2, settled by execution)"**, and
`docs/src/clients.md:173` says the `◐` is *"no blocking verdict on the after a
tool ran event"*. So I wrote three rows instead of one: the real `◐` cause,
the settled-mutator row with its two accepted residuals (per-**tool** `Bash`
refusal; Copilot's transcript displaying the original command while executing
the rewritten one), and Copilot's fail-**closed** `preToolUse`, which is why
its guard is mandatory and exec form must never be used. Had I followed the
instruction I would have filed a supported capability as a pending decline.

**6. `src/command/publish.rs:1864-1878`'s comment is now stale** (src, not
mine). It says hook bundle membership's authoring side "is not accepted by
`RawBundleSource` yet, so this ordering is currently correct-and-unused".
WP-S's `ebb37bc` added the `hooks` field to `RawBundleSource`, and I have now
**published and installed a real bundle carrying a hook member** (evidence
above), so the ordering is correct-**and-exercised**. The comment explicitly
warns against "simplifying" the ordering on the belief that no bundle can name
a hook — that belief is now the comment's own.

**6b. `src/config/declaration.rs`'s `trust_hooks` description overstates the
implicit grant** (src, not mine; reaches a **published** schema). It reads
*"In global config, unset means trusted, because configuring a registry there
is itself the trust act"*, with no mention of the bare-host exception. Per
`trust.rs::decide` and verified as V1/V2 above, a global entry whose locator is
a **bare host** (`oci = "ghcr.io"`) grants nothing unless `trust_hooks = true`
is explicit — and a bare host is the likeliest thing a user writes for a
multi-tenant registry. The text surfaces in
`docs/src/schemas/grimoire-config.schema.json` (gitignored locally, but
published to grimoire.rs) and in `grim config registry fields`. I documented
the accurate rule in `configuration.md`; the description string needs the same
caveat. One sentence in `declaration.rs`.

**7. `grim schema`'s clap `about` omits two kinds** (src, not mine).
`grim schema --help` prints *"Print the JSON Schema for grimoire.toml,
publish.toml, or grimoire.lock"* while `--kind` accepts `mcp` and `hook` too.
Flagged with a ⚠ in `subsystem-cli-commands.md`; the fix is one string in
`src/command/schema.rs`.

**8. `subsystem-cli-commands.md` was missing `grim describe` entirely** —
WP-N's finding, confirmed (`src/command/describe.rs` ships, `grim --help`
lists it). Added. This also means the `AGENTS.md` count was wrong by two, not
one: **20 → 23**, not 22.

**9. `product-context.md` says "(18 subcommands)"** — not in my file list and
not touched. Now off by five. Routing it rather than silently widening scope.

---

## Route-in items

* **`catalog/README.md` — done.** The trigger list now names `src/oci/hook.rs`
  (which actually defines the published format, so a schema change previously
  triggered no `grim-authoring` review) plus `configuration.md`,
  `json-interface.md` and `stability.md`. I edited it despite `catalog/**`
  being on the do-not-touch list, because the brief named this file
  explicitly and told me to add it if reachable. WP-N's work is already merged
  into my base, so there is no conflict.
* **WP-P0 obligation 7 — done.** The "a `gatekeeper` silently not firing is
  deliberate design" non-goal is a blockquote in `artifacts.md` § Tiers:
  *"A grim hook is defence in depth, never a security boundary."*
* **WP-P0 obligation 5 (W7) — done.** The exit-78 older-grim rejection is in
  both `configuration.md` § trust-hooks and
  `stability.md#limitations-hook-reporting`.
* **WP-P0 obligation 6 — done.** Codex's `$SHELL -lc` entry is dated in the
  watchlist.
* **The reversed "per-hook consent" claim — checked, absent from `docs/**`.**
  `grep -rniE "per-hook|per hook|consent" docs/src/` returns only three
  unrelated hits, all in `clients.md` and all correct (two are about per-hook
  *matcher* refusals, one distinguishes capability from consent). The bundle
  row in `artifacts.md` already stated the member's-own-source rule, and both
  halves of it are now executed rather than asserted.

## Commit split, and why

Two commits rather than one. `docs/src/**` is user-facing and belongs in the
changelog (`docs:`); `AGENTS.md`, `.claude/rules/**` and `catalog/README.md`
are agent context that `AGENTS.md` itself says must not appear there
(`chore:`). One commit would necessarily mis-file one half — the type is the
changelog signal, so the split follows the semantics rather than the file
count. No rule `paths:` glob changed and no rule file was added or removed, so
`.claude/rules.md` needed no sync; `task claude:tests` confirms no drift.

## Not done / not mine

* The payload location (above) — SEC-1's package.
* The three shipped defects are **documented, not fixed**:
  `src/command/hook/list.rs` (WP-H/WP-K), `status.rs::hook_arming` (WP-H),
  `config.rs::refuse_disarm_via_config` (owner-deferred, review W6).
* `catalog/**` third-gate restoration (WP-N's files).
* `src/command/schema.rs`'s `about` string and
  `src/command/publish.rs`'s stale comment.
* `product-context.md`'s subcommand count.
* `mdbook` is not installed in this environment, so `task docs:build` could
  not render the site. I substituted a script that parses every heading in
  `docs/src/**` (explicit `{#anchor}` tags plus auto-slugs) and checks every
  intra-docs link and reference definition against it: **0 broken targets**.
  Markdown rendering itself is therefore unverified by mdBook.
