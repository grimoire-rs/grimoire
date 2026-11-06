# Pre-settings golden fixtures

Committed **data**, not code. These bytes were produced by a `grim` built from
commit **`47da12f`**, the tip of the install-fix work before the `settings`
artifact kind reached install. They exist so a *later* binary can be proved to
read, re-lock, and modify what a settings-free grim wrote, and to keep
reading the hook consent record that grim left behind.

## Never regenerate these from a later tree

**The commit these were built from is the entire value of the fixture.**
Regenerating with a binary that knows about settings turns the test that
consumes them from

> *the settings kind did not disturb a project that never declared one*

into

> *the current binary agrees with itself*

- **If a fixture mismatches, the binary is wrong, not the fixture.** A changed
  lock byte order, install-state field, declaration hash or consent-key
  spelling on a settings-free project is a breaking change.
- **If a mismatch is intended** (an approved additive change with its own
  migration story), add a *second* fixture directory for the new baseline
  commit and keep this one.
- `tools/generate.py` refuses to write into a directory containing a
  `README.md`, and needs `GOLDEN_GRIM` pointing at a binary built at `47da12f`.

## The declared surface

The project declares every artifact kind that existed at the baseline:

| Kind | Declaration |
|---|---|
| skill | `reg-skill` (registry), `local-skill` (path) |
| rule | `reg-rule` |
| agent | `reg-agent` |
| mcp | `reg-mcp` |
| hook (flag on) | `reg-hook`, armed by a consent record |
| bundle | `reg-bundle` (members `reg-skill`, `bundled-rule`), `local-bundle` (member `local-bundled-agent`) |

The global scope (`global-grimoire.toml`) declares direct artifacts only, so
the offline variants lock a bundle-free scope.

## Layout and roles

| Path | Role |
|---|---|
| `golden/project.grimoire.lock`, `golden/global.grimoire.lock` | **Assertion targets.** Locks after one lock+install pass per scope. |
| `golden/state.project.json`, `golden/state.global.json` | **Assertion targets.** Install state. |
| `golden/declaration_hash.json` | **Assertion target.** Both scopes' `declaration_hash` plus `declaration_hash_version`. |
| `golden/consent.project.json` | The consent record `grim hook allow` wrote, with `workspace` replaced by `<WORKSPACE>` and `consented_at` by `<NORMALIZED>`. A consumer fills both in for its own project directory. Its `hooks` holds hook keys only, in the spelling `<binding>@<registry>/<repository>`. |
| `golden/project.after-remove.grimoire.lock`, `golden/state.project.after-remove.json` | **Assertion targets.** The project after `grim remove bundle reg-bundle`. |
| `golden/tag-cache/`, `golden/tag-cache-no-manifests/` | A warm `$GRIM_HOME` tag cache, with and without the registry manifest blobs. Offline store-hit and store-miss cases. |
| `provenance/status.{project,global}.json` | `grim status --format json` as the baseline reported it. Compare `source` only. |
| `registry/pushed.json` | The six registry artifacts, recorded byte for byte. Replaying them reproduces every manifest digest in the locks. |
| `project/`, `global-grimoire.toml` | The declaration inputs. |
| `commands.txt` | The exact commands that produced the above. |

Registry host is **`localhost:5000`**, embedded in every `pinned =` value, every
consent key and both declaration hashes. A consumer without a registry there
skips rather than normalizing. Repos live under `grim-golden/pre-settings-47da12f/`.

## Exactly how these were produced

```sh
# 1. Build the baseline binary from 47da12f (submodules initialized).
cargo build --locked --bin grim            # -> target/debug/grim

# 2. An empty OCI registry on localhost:5000 (a throwaway zot was used).

# 3. Generate into a scratch directory, never into this one.
GOLDEN_GRIM=/path/to/that/grim python3 tools/generate.py /scratch/golden-out
```

Per scope there is exactly **one** lock+install pass, because a second pass
would make the mcp splice target pre-exist and the state would record
`"adopted": true`. The project's hook is armed by `grim hook allow` between
`lock` and `install`; no `--trust-hooks` is passed, so the record is what arms
it. `remove` runs last, after every capture.

Environment during generation: fresh empty `HOME` holding `.claude/`,
`.copilot/`, `.codex/`; fresh absolute `GRIM_HOME`; `XDG_CONFIG_HOME=$HOME/.config`;
`GRIM_INSECURE_REGISTRIES=localhost:5000`; `NO_COLOR=1`; `PATH=/usr/bin:/bin`;
everything else unset. `--client claude` is pinned.

## Things a consumer must know

- **`generated_at` is the only time-varying line in the locks.** Seed the golden
  lock before `grim lock` and it is preserved byte for byte, or blank the line.
- **`generated_by` carries the binary version** and moves at every release; mask it.
- **The project hook state embeds the workspace path** as `hooks/payload/<64 hex>/reg-hook`,
  a SHA-256 of the workspace directory. Replace that component on both sides.
- **The consent record's filename is the SHA-256 of the workspace path**, and
  its `workspace` field is compared byte for byte; write both for the
  directory under test.
- **The tag-cache registry directory is stored as `localhost_5000`**; a Unix
  consumer copies it to `tags/localhost:5000/`.
- **Offline cases lock the global scope.** Seed the golden global lock, copy
  one variant into `$GRIM_HOME`, run `grim lock --global --offline`. With
  neither variant present it exits 69.

## Determinism and self-sufficiency, measured

Two independent generations, 10 seconds apart, were diffed: only
`generated_at` (three lock files) and the workspace-key component varied; the
consent record was identical. `tools/replay.py` loaded the recorded bytes into
an empty registry (every replayed digest matched) and `tools/verify.py`
re-ran against the baseline binary:

```
IDENTICAL  project.grimoire.lock / global.grimoire.lock
IDENTICAL  state.project.json / state.global.json
IDENTICAL  declaration_hash[project] / [global]
IDENTICAL  project.after-remove.grimoire.lock / state.project.after-remove.json
IDENTICAL  global.grimoire.lock   # offline, golden/tag-cache/
IDENTICAL  global.grimoire.lock   # offline, golden/tag-cache-no-manifests/
FAILS AS EXPECTED (exit 69)       # offline, empty cache
```

## Tools

| File | Purpose |
|---|---|
| `tools/generate.py` | Produced everything here. Stdlib only; refuses to write into this directory. |
| `tools/replay.py` | Pushes `registry/pushed.json` into a registry verbatim, verifying each digest. |
| `tools/verify.py` | The check whose output is quoted above. `GOLDEN_SEED=0` selects the unseeded mode. |

None of these is a pytest test; `test/pyproject.toml` sets `testpaths = ["tests"]`.
