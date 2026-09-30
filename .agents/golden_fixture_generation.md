# C-015 golden fixtures — generation record

**Verdict: C-015 is now satisfiable as written, and was verified to be so with the
baseline binary.** Three wording fixes are owed (below); none of them blocks WP-E's
Specify phase.

- **Fixture location:** `test/data/golden/pre_hooks_03e59b0/` (19 files, 192 KB).
  `.claude/rules/subsystem-tests.md` prescribes no fixture-data directory — it
  documents `test/{conftest.py,src,tests,recordings}` only — so this follows the
  path the task named rather than overriding a rule.
- **Nothing committed.** Rust source untouched, `test/` test code untouched. Only
  new files under `test/data/` plus this report.
- **Pre-existing dirt, not mine:** `test/uv.lock` shows as modified in the working
  tree (PyPI URLs rewritten to Vector's Artifactory mirror). I never ran `uv`;
  flagging it so it is not attributed to this work.

---

## 1. What was generated

A **hook-free** project declaring **all five existing kinds**, each registry-sourced
and — where the kind permits — also path-sourced:

| Kind | Registry-sourced | Path-sourced |
|---|---|---|
| skill | `reg-skill`, plus `bundled-skill` via the bundle | `local-skill` |
| rule | `reg-rule`, plus `bundled-rule` via the bundle | `local-rule` |
| agent | `reg-agent` | `local-agent` |
| mcp | `reg-mcp` | — `[mcp]` is `PathValues::Rejected` by design |
| bundle | `reg-bundle` (2 members) | — a local bundle's members must be absolute registry refs (`resolver.rs:655-663`) |

**Nothing was reduced.** All five kinds are covered, in both scopes. This mattered
more than it first looks: the hook kind inserts `[[hook]]` **between `[[mcp]]` and
`[[bundle]]`** in the lock and `"hooks"` **between `"bundles"` and `"mcp"`** in the
JCS declaration document, so the two kinds that *cannot* be produced offline —
`mcp` and `bundle` — are precisely the ones that bracket the insertion point. An
offline-only fixture (skill/rule/agent) would have missed the exact adjacency
C-015 exists to protect. A local `registry:2` was therefore started, the same way
`test/conftest.py` does.

### Assertion targets (`golden/`)

| File | Contents |
|---|---|
| `project.grimoire.lock` | project lock — contains `[[skill]] [[rule]] [[agent]] [[mcp]] [[bundle]]` + `[[bundle.member]]` |
| `global.grimoire.lock` | global lock |
| `state.project.json` | `<workspace>/.grimoire/state.json` |
| `state.global.json` | `$GRIM_HOME/state/global.json` |
| `declaration_hash.json` | both scopes' `declaration_hash` + `declaration_hash_version: 1` |

`declaration_hash` (project) = `sha256:191fd3b4f3511a51b98e59cb3ab551f04c8006b83ade13e678c79b4ef8886aa3`
`declaration_hash` (global)  = `sha256:0f3bc887790c9a61aa167ec86e34aa5b2334c5e323ad7ad326707268b3f846d3`

### Inputs (all committed, so the fixture is self-sufficient)

- `project/grimoire.toml` + `project/{skills,rules,agents}/…`
- `global-grimoire.toml`
- `registry/pushed.json` — the five registry artifacts recorded **byte-for-byte**
  (manifest bytes verbatim, layer as hex), so replaying them reproduces every
  manifest digest with no dependence on `grim`, on `test/src/registry.py`, or on
  this repo's packer.

### Explicitly *not* assertion targets (`provenance/`)

`status.project.json`, `status.global.json`, `context.project.json`. Kept as a
readable record; unfit for byte comparison because `context` embeds
`"version": "0.13.0"` (changes every release) and all three carry absolute paths
(rewritten to `<ENVROOT>`). Labelled as such in the README so nobody promotes
them into an assertion by accident.

---

## 2. Provenance

| | |
|---|---|
| Commit | `03e59b053de60173a866c783581a999ff04f4e12` (`03e59b0`) — no hook code exists |
| Worktree | `.agents/worktrees/golden-baseline`, clean, unmodified except build artifacts |
| Submodules | `external/docker_credential` `8e89cd0e`, `external/rust-oci-client` `7f3d0b6c` (initialized as instructed) |
| Binary | `grim 0.13.0`, debug |
| Build | `CARGO_TARGET_DIR=/home/mherwig/.cache/grim-golden-target CARGO_INCREMENTAL=0 cargo build --bin grim` — 24.9 s, exit 0 |
| Registry | `registry:2` on `localhost:5000`, repos under `grim-golden/pre-hooks-03e59b0/` |
| Generator | `test/data/golden/pre_hooks_03e59b0/tools/generate.py` (stdlib only; does not import `test/src/*`) |

**Disk:** `/mnt/wsl/share` was never touched by the build. `CARGO_TARGET_DIR` was
redirected to `/home/mherwig/.cache/` (`/dev/sde`, 530 GB free); the 1.3 GB target
dir lives there. `/mnt/wsl/share` stayed at 5.8 GB free throughout. `cargo clean`
was never run. **Exactly one build.**

### Exact command sequence

```sh
git -C .agents/worktrees/golden-baseline submodule update --init
cd .agents/worktrees/golden-baseline
CARGO_TARGET_DIR=/home/mherwig/.cache/grim-golden-target CARGO_INCREMENTAL=0 \
  cargo build --bin grim
cp -f /home/mherwig/.cache/grim-golden-target/debug/grim \
      /home/mherwig/.cache/grim-baseline-03e59b0

docker run -d --rm -p 5000:5000 --name grim-golden-registry registry:2

GOLDEN_GRIM=/home/mherwig/.cache/grim-baseline-03e59b0 \
  python3 tools/generate.py <scratch-dir>
```

and inside the generator, per scope, exactly one pass (also in `commands.txt`):

```
$ grim lock                                  # cwd=<project>
$ grim install --client claude               # cwd=<project>
$ grim lock --global                         # cwd=<GRIM_HOME>
$ grim install --global --client claude      # cwd=<GRIM_HOME>
```

### Environment pinned

`HOME` (fresh, containing only `.claude/ .copilot/ .codex/` so global client
detection is deterministic), `GRIM_HOME` (fresh, **absolute**),
`XDG_CONFIG_HOME=$HOME/.config`, `GRIM_INSECURE_REGISTRIES=localhost:5000`,
`NO_COLOR=1`, `PATH=/usr/bin:/bin`. **Everything else unset** — the ambient
environment is stripped. `--client claude` pinned explicitly, because client
auto-detection reads the ambient filesystem.

> Two things the plan predicted, hit for real during generation, both recorded in
> the README so a future maintainer does not re-learn them:
> 1. **A relative `GRIM_HOME` is resolved against the process CWD** — my first run
>    passed `./run1/_env/grim-home` and grim created nested
>    `run1/_env/grim-home/run1/_env/grim-home` directories. This is exactly the
>    `env::grim_home()` behaviour the plan records as finding **B1**, observed
>    incidentally on the 0.13.0 binary.
> 2. **Install outputs must not leak into the fixture *input*.** The first fixture
>    captured `project/` by copying the post-install tree, which dragged
>    `.mcp.json` along; on the next install the splice target pre-existed and grim
>    recorded `"adopted": true` instead of `false`. The fixture now writes its
>    inputs explicitly. Same reason there is exactly **one** lock+install pass.

---

## 3. Determinism — generated three times, diffed literally

`gen1`, `gen2`, `gen3` — independent clean environments. `gen3` was deliberately
separated in time (`sleep 5`) so a same-second collision could not hide a
timestamp difference (`gen1`/`gen2` did collide at `07:14:44Z`, which is why a
third run was needed to make the claim honestly).

```
$ diff -r gen1/golden gen2/golden        ; echo exit=$?
exit=0
$ diff -r gen1/registry gen2/registry    -> identical
$ diff -r gen1/project  gen2/project     -> identical

$ diff -r gen1/golden gen3/golden        ; echo exit=$?
diff -r gen1/golden/global.grimoire.lock gen3/golden/global.grimoire.lock
6c6
< generated_at = "2026-08-17T07:14:44Z"
---
> generated_at = "2026-08-17T07:15:01Z"
diff -r gen1/golden/project.grimoire.lock gen3/golden/project.grimoire.lock
6c6
< generated_at = "2026-08-17T07:14:44Z"
---
> generated_at = "2026-08-17T07:15:01Z"
exit=1
```

**`generated_at` is the only non-deterministic byte in the entire fixture** — one
line in each of the two locks. Everything else reproduced identically: both
install-state files, both declaration hashes, all five manifest digests, all three
path-source content hashes, every `content_hash`, the recorded registry bytes, and
the project inputs.

### Normalizations — exactly one, and it is optional

| Field | Why | Disposition |
|---|---|---|
| `generated_at` (both locks) | wall clock | **Not normalized in the committed data.** Committed with its real value (`2026-08-17T07:14:44Z`). Two consumption strategies, both executed and proven below — Strategy A needs **no normalization at all**. |
| absolute paths in `provenance/*.json` | environment | rewritten to `<ENVROOT>`; these files are **not** assertion targets |
| `"version": "0.13.0"` in `provenance/context.project.json` | release-coupled | not normalized; file excluded from assertion for this reason |

**No field had to be excluded from an assertion target.** Install state needed no
normalization whatsoever: it is entirely anchor-relative (`"anchor": "workspace"` /
`"claude-root"` / `"claude-user-dir"` + a relative path), carrying no absolute
path, no hostname and no timestamp.

**Strategy A — seeded, zero normalization (recommended).** Copy the golden lock in
*before* `grim lock`. `lock_io::save` preserves `generated_at` verbatim when every
artifact's resolved content is unchanged, so a hook-free project reproduces the
golden lock byte-for-byte *including the timestamp*. If content did change, the
timestamp is bumped **and** the content differs — the test fails, correctly.

**Strategy B — unseeded.** Blank the single `generated_at = "…"` line on both sides.

Both run against the **committed** bytes with the baseline binary:

```
$ GOLDEN_GRIM=<baseline> python3 tools/verify.py . /tmp/w-seeded
IDENTICAL  project.grimoire.lock
IDENTICAL  global.grimoire.lock
IDENTICAL  state.global.json
IDENTICAL  state.project.json
IDENTICAL  declaration_hash[project]
IDENTICAL  declaration_hash[global]

$ GOLDEN_SEED=0 GOLDEN_GRIM=<baseline> python3 tools/verify.py . /tmp/w-unseeded
   ... the same six lines, all IDENTICAL
```

### Self-sufficiency proven against a *fresh, empty* registry

The `registry:2` container was stopped and replaced with an empty one, the recorded
bytes replayed, and the comparison re-run:

```
$ python3 tools/replay.py .
replayed grim-golden/pre-hooks-03e59b0/reg-skill:1.0.0  -> sha256:9f9ab08b…c637
replayed grim-golden/pre-hooks-03e59b0/reg-rule:1.0.0   -> sha256:657ed756…b748
replayed grim-golden/pre-hooks-03e59b0/reg-agent:1.0.0  -> sha256:74d332f2…305a
replayed grim-golden/pre-hooks-03e59b0/reg-mcp:1.0.0    -> sha256:3499d7c5…6de1
replayed grim-golden/pre-hooks-03e59b0/reg-bundle:1.0.0 -> sha256:6d4bb9c0…4e4e
$ GOLDEN_GRIM=<baseline> python3 tools/verify.py . /tmp/w
   ... all six IDENTICAL
```

Every replayed digest matched the record, so the digests in the golden locks are a
function of the committed bytes alone.

---

## 4. The anti-regeneration guard

The README's most important section is **"⛔ Why you must not regenerate these from
a later tree"**, stating plainly that a refresh converts the test from *"the hook
kind did not disturb a hook-free project"* into *"the current binary agrees with
itself"*, deleting the only Principle 9 evidence **while leaving a green test
behind** — the worst failure mode, because nothing looks broken. It prescribes:
a mismatch means the binary is wrong; an intended change adds a *second* baseline
directory rather than overwriting this one.

Backed by a mechanical guard, tested:

```
$ python3 tools/generate.py <the committed fixture dir>
refusing to write into …/test/data/golden/pre_hooks_03e59b0: it looks like the
committed fixture. Generate into a scratch directory instead.
```

---

## 5. What I could not capture

- **Nothing was lost to offline constraints.** The registry was reachable (started
  locally), so no kind was dropped and no subset fallback was needed.
- **No JCS-document surface.** The CLI exposes no way to print the canonical
  declaration document, only its hash, so the *"`hooks` sorts between `bundles` and
  `mcp`"* half of C-015 stays a unit-test obligation on `config/hash.rs`. The
  fixture pins the hash, which is the observable consequence.
- **One client only.** `--client claude` is pinned for determinism, so the fixture
  does not cover codex/copilot/opencode renderers. C-015 does not ask for them.
- **Linux only.** No Windows path-rendering coverage.
- **I did not run the post-hooks binary against these fixtures.** That is WP-E's
  Specify job, and doing it would have meant `git submodule update --init` inside
  `.agents/worktrees/wp-e` — a real modification to another worker's active
  worktree. The fixtures are ready for it; the check is four `grim` invocations
  plus `tools/verify.py`.

---

## 6. Verdict on C-015, and the wording it still owes

**Satisfiable as written, and verified.** All three named artifacts
(`grimoire.lock`, `state/global.json`, `declaration_hash`) exist as committed data
produced by a `03e59b0` binary, reproduce byte-for-byte, and are reproducible from
the fixture alone. `DECLARATION_HASH_VERSION` is `1` in both scopes. The contract
does **not** need rewriting to be met.

Three clauses should still be corrected, because as phrased each invites a tester
to do the wrong thing:

1. **"committed before WP-E's stub lands" is now factually impossible** — WP-E's
   stub already exists on `hex/hooks-artifact-kind--wp-e`. The clause was a proxy
   for the property that actually matters, which is *provenance*, not ordering.
   Reword to **"generated by a binary built from `03e59b0`"**. The fixtures satisfy
   that; they cannot satisfy the ordering, and leaving the clause in makes a
   satisfied contract read as unsatisfiable.
2. **"byte-identical" should name the strategy.** It is literally true under
   Strategy A (seeded — grim's own shipped `generated_at` preservation) and true
   modulo one blanked line under Strategy B. A tester who silently picks B and
   normalizes has quietly weakened "byte-identical" without saying so. Naming
   Strategy A as the contract's method keeps the words honest.
3. **The registry precondition is contractual, not incidental.** Both golden locks
   embed `localhost:5000` in every `pinned = …`. The consuming test must **skip**
   when the session registry host differs — never normalize the host, since that
   rewrites the bytes under test. C-015 should say so.

One additive suggestion: C-015 names only `state/global.json`, but the fixture also
pins the **project** install state, which is where the mcp `entry` pointer and the
`adopted` flag live — the two state fields most exposed to a registration-shaped
kind. Worth folding into the contract, since the data is already there.
