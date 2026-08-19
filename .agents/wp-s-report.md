# WP-S — hook bundle membership (authoring side)

Branch `hex/hooks-artifact-kind--wp5-s`, base `a620175`, one commit `ebb37bc`
(`feat(bundle): accept hooks as bundle members`). All gates green.

## Verdict on the scoping: it held, and it was incomplete

Your hypothesis was right about everything you asserted, and I verified each
claim by reading rather than trusting it:

| Your claim | Verified |
|---|---|
| `effective_set.rs:60` lists `(Hook, &set.hooks)` | Yes — the 5-tuple member iteration |
| `effective_set.rs:199` handles `Hook` in the declaration-removal arm | Yes, with the comment you quoted |
| `add.rs:704` `bundle_members_lock` already filters `hooks` | Yes, with the `mcp`-defect comment |
| `RawBundleSource` has no `hooks` and is `deny_unknown_fields` | Yes — **exit 78**, reproduced below |
| `build.rs` pushes exactly three kinds | Yes |

Beyond the authoring gap I also confirmed, by reading, that the **resolver**
(`collect_work`, `expand_bundles`, `merge_bundle_members`, `build_lock`),
**prune** (`src/install/prune.rs` — kind-blind via `lock.iter_artifacts()`),
**`grim lock`**, and **`grim status`** (including bundle-provenance rendering
for a hook row) all handle Hook correctly. No kind allowlist rejects a Hook
member anywhere: the only member-kind rejection in the tree is
`resolver.rs:476`, which is `Bundle`-specific (no recursion in v1).

**But the install side was not complete.** Three real defects, two of which I
fixed because your own test spec ("the inverse: dropping the bundle drops the
hook") is impossible to satisfy without them.

## Defect 1 (fixed) — `drop_from_lock` never processed `lock.hooks`

`src/command/remove.rs`: the effective-set retain pass fanned over
`skills`/`rules`/`agents`/`mcp` and **not** `hooks`, then restamped
`declaration_hash` to the post-edit value regardless. An undeclared `[[hook]]`
therefore survived in a lock that reads **FRESH**, and the next `grim install`
re-materialized *and re-armed* it.

This is verbatim the `mcp` defect the adjacent comment records as already
shipped once, but with a strictly larger blast radius: the resurrected artifact
is code a client runs automatically, after the user asked for it to be gone.

Reachable four ways, none exotic: `grim remove hook <name>`, `grim uninstall
hook <name>` (which calls `super::remove::drop_from_lock`), the TUI delete
action (shared `undeclare_and_unlock` seam), and removing a bundle that provided
a hook member.

Fix: `process(&mut lock.hooks);`.

## Defect 2 (fixed) — `evict_bundle_members` never evicted hook members

Same file, the **legacy** path. This one is not a corner case:
`effective_set` degrades for the **whole call**, so a single path-declared
bundle anywhere in the project — or a pre-cache/retagged lock — routes *every*
mutation through `evict_bundle_members`. It retained over four lists, not five,
so a dropped bundle's hook member survived with `stale` still `false`, the hash
restamped fresh, and `grim install` re-armed it.

Fix: `lock.hooks.retain_mut(evict);`.

Both fixes are in `src/command/remove.rs`, which is **not** in your forbidden
list (that names `install.rs`, `update.rs`, `add.rs`, `target.rs`,
`hook_registrar.rs`, `tui/app.rs`, `api/install_report.rs`). I did not need to
touch `add.rs` — `bundle_members_lock` was already correct.

## Defect 3 (found, NOT fixed — hand-off)

`src/install/status_badge.rs:149-160` — `find_by_repo` chains
`skills`/`rules`/`agents`/`mcp` and omits `hooks`, so `derive_badge` returns
`NotInstalled` for any installed hook. That mis-badges `grim search` rows, the
TUI search rows, and the deprecated-row filter at `search.rs:179`. One-line fix:
use `lock.iter_artifacts()` (the seam exists for exactly this, and
`status.rs:1018 find_locked` already uses it).

Left alone deliberately: it affects **every** locked hook, bundle-provided or
not, so it is independent of this requirement, and `src/install/**` is adjacent
to WP-R's live surface. Recommend routing to WP-R or WP-M.

## Executed evidence

### The exit-78 reproduction (before)

```
$ grim build --kind bundle stack.toml       # stack.toml has a [hooks] table
invalid TOML: TOML parse error at line 6, column 2
  |
6 | [hooks]
  |  ^^^^^
unknown field `hooks`, expected one of `skills`, `rules`, `agents`, `summary`,
`keywords`, `description`, `license`, `repository`, `deprecated`, `replaced-by`

EXIT=78
```

**Exit 78, not 65** — your number was right.

### After: the same file, exit 0

```
$ grim build --kind bundle stack.toml --format json
{ "kind": "bundle", "name": "stack",
  "layer_digest": "sha256:9b21d68f…", "status": "built" }
EXIT=0
```

### The wire layer, pulled back out of a real registry

Released a bundle with one member of every installable kind to
`localhost:5000`, then fetched the manifest and the layer blob directly:

```
layers: application/vnd.grimoire.bundle.v1+json sha256:acb5c131… 470
annotations: com.grimoire.kind=bundle,
             org.opencontainers.image.description="grimoire bundle of 4 members"
```

```json
{
  "members": [
    { "kind": "skill", "name": "code-review", "id": "localhost:5000/wp5s/code-review:1" },
    { "kind": "rule",  "name": "rust-style",  "id": "localhost:5000/wp5s/rust-style:1" },
    { "kind": "agent", "name": "reviewer",    "id": "localhost:5000/wp5s/reviewer:1" },
    { "kind": "hook",  "name": "shell-guard", "id": "localhost:5000/wp5s/hooks/shell-guard:1" }
  ]
}
```

The source authored `[hooks]` **first**, so this proves the order comes from the
sort, not from table or loop order.

### Ordering and digest stability

`BundleManifest::new` sorts by `(kind, name)` using `ArtifactKind`'s derived
`Ord`. The enum is `Skill, Rule, Agent, Bundle, Mcp, Hook` — `Hook` is **last**,
so a hook member appends and no pre-hook member moves.

```
BASELINE (pre-change binary):  sha256:0fc6d961f2c9bd99856502b75b35f2365e34c7b2f7902fea90a72e1c8fd7dbbb
AFTER    (post-change binary): sha256:0fc6d961f2c9bd99856502b75b35f2365e34c7b2f7902fea90a72e1c8fd7dbbb
```

Same skills+rules+agents bundle, both binaries, identical digest — an existing
bundle's digest does not move. Three consecutive rebuilds of the hook-bearing
bundle also produced one digest (`sha256:9b21d68f…` ×3).

### End to end: publish → bundle → add → lock

Published a real hook artifact and a bundle declaring it, then `grim add` the
bundle. The resulting `grimoire.lock`:

```toml
[[hook]]
name = "shell-guard"
pinned = "localhost:5000/wp5s/hooks/shell-guard@sha256:3604c2e5…"
bundle = "localhost:5000/wp5s/bundles/guard-stack"
bundle_tag = "1"

[[bundle]]
name = "guard-stack"
repo = "localhost:5000/wp5s/bundles/guard-stack"
pinned = "localhost:5000/wp5s/bundles/guard-stack@sha256:59dafbdb…"

[[bundle.member]]
kind = "hook"
name = "shell-guard"
id = "localhost:5000/wp5s/hooks/shell-guard:1"
```

Note the member's `pinned` is the **hook's own** digest, distinct from the
bundle's — your constraint #2 holds: the member's `LockedSource` is unambiguous
at the install seam.

`grim status`:

```
hook  shell-guard  bundle: localhost:5000/wp5s/bundles/guard-stack  …@sha256:3604c2e5…  gated  claude: feature-flag-off
```

```json
"state": "gated",
"arming": [{ "client": "claude", "cause": "feature-flag-off",
             "message": "hooks are disabled for this scope; run grim config set options.experimental.hooks true, then grim install",
             "transient": false }]
```

**Nothing armed** — constraint #1 holds. The hook did reach the install seam
(WP-R's registrar answered `the 'hook' artifact kind is not supported by this
build of grim`, which is that seam mid-construction), and the feature flag
gated it.

Removing the bundle dropped the `[[hook]]` entry entirely, and a subsequent
`grim install` reported `{"items": []}` — no resurrection.

## Tests

11 new unit tests, 10 new acceptance tests
(`test/tests/test_bundle_hook_members.py`).

**Both fixes proven by reversion**, not by assertion:

- reverting the two `remove.rs` lines fails **5 unit tests**
  (`remove_hook_drops_its_lock_entry`, `remove_hook_keeps_an_unrelated_entry`,
  `remove_bundle_via_sets_evicts_its_hook_member`,
  `legacy_bundle_eviction_drops_its_hook_member`,
  `legacy_bundle_eviction_keeps_hook_shared_with_other_bundle`)
- and **2 acceptance tests**
  (`test_removing_the_bundle_evicts_its_hook_member`,
  `test_removing_a_directly_declared_hook_drops_it_from_the_lock`).

`test_a_bundle_provided_hook_is_not_armed_by_membership_alone` pins the
*absence* of arming as the contract, so bundle membership can never quietly
become a route around the consent gate.

Test helpers gained `make_hook()` and a `hooks=` kwarg on `write_config()`
(emitted only when non-empty and last, matching `grim`'s own writer — an
always-present empty `[hooks]` table would move every existing fixture's
declaration hash).

## The `mcp` asymmetry — flagged, not fixed, as instructed

`mcp` is absent from `RawBundleSource` while being a first-class member
everywhere downstream: the resolver expands an `mcp` member, `effective_set`
lists it, `bundle_members_lock` filters it (with a comment recording the
shipped defect from when it did not), `prune` and `remove` treat it as one, and
the wire `BundleMember.kind` deserializes `"mcp"` fine. So a hand-pushed
bundle naming an `mcp` member resolves and installs today; grim just cannot
author one. Exactly the shape the hook gap had. Owner question — untouched.

I did adjust `mcp-spec.md`'s wording: its "not a bundle member" claim is still
true of the parser, but it now says the restriction is the **authoring
parser's**, not the wire format's, so nobody reads it as "the resolver rejects
mcp members".

## Things I found wrong in already-merged text

1. **`hook-spec.md:322-325`** — "A hook cannot be a bundle member." Accurate
   about the old parser; now false. Flipped, with the declares-never-arms
   contract and the member-own-source trust note. (You assigned me this one.)
2. **`bundle-spec.md:35`** — "any key outside this set and **the three member
   tables**". Corrected to four, with the `[hooks]` example.
3. **`bundle-spec.md:138`** — the pitfalls table listed "An `[mcp]` or
   `[hooks]` member table → hard parse error — **neither kind is bundleable**".
   Now `[mcp]` only, and it says `[hooks]` is accepted.
4. **`consume.md:416`** — "A bundle's member tables are `[skills]`, `[rules]`,
   and `[agents]` — **an MCP server and a hook are not bundleable.**" Corrected.
5. **`src/oci/bundle.rs:41-42`** — `BundleMember.kind`'s doc said "Only `skill`
   and `rule` are valid", which was already wrong for `agent` and `mcp` before
   hooks existed. Corrected to "every installable kind"; the module header said
   "listing the skill/rule members" and got the same treatment.
6. **`build.rs:518`** — the test named
   `read_bundle_members_covers_every_member_table` asserted only three kinds,
   so its name was a promise it did not keep. Now covers four.

Nothing in `.agents/plans/plan_hooks_artifact_kind.md` or
`.agents/wp-n-report.md` contradicted the code. WP-N's finding #2 ("hooks are
not bundle members") was true of the parser when written, which is precisely
the gap this package closes.

## Gates

```
cargo fmt                                        clean
cargo clippy --locked --all-targets -- -D warnings   clean
cargo test --bin grim                            2875 passed, 0 failed
task catalog:verify                              all 5 catalog packages build
task --force verify                              1056 acceptance tests passed
```

`.claude/tests/uv.lock` and `test/uv.lock` reverted; staged file-by-file, never
`git add -A`. `commit-verified` was stamped by the verify task itself, not by
hand.

## Files

- `src/config/project_config.rs` — `hooks` on `BundleSource`/`RawBundleSource`, `parse_member_map`, 3 tests
- `src/command/build.rs` — Hook member push, 2 tests
- `src/command/remove.rs` — the two fixes, 6 tests + 3 helpers
- `src/oci/bundle.rs` — doc corrections only, no behavior change
- `test/src/helpers.py` — `make_hook()`, `write_config(hooks=…)`
- `test/tests/test_bundle_hook_members.py` — new, 10 tests
- `catalog/skills/grim-authoring/references/{hook,bundle,mcp}-spec.md`, `catalog/skills/grim-usage/references/consume.md`

`grim schema` needed no change: `SchemaKind` is `Config | Publish | Lock | Mcp |
Hook` with **no `Bundle`** variant, and `RawBundleSource` derives only
`Deserialize`. The bundle source shape is not published as a schema at all, so
the new field flows into nothing. The `Lock` schema's `$defs.BundleMember` picks
`"hook"` up automatically via `ArtifactKind`'s `JsonSchema` derive.
