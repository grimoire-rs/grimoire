# Pre-dependencies golden fixtures

Committed **data**, not code. These bytes were produced by a `grim` built from
**`cdeedcaa63198eb55a1581555d33465f50a1e351`** (`cdeedcaa`, tag `v0.15.1`), the
last release before declared artifact dependencies. They exist so a *later*
binary can be proved to read, re-lock, and modify what 0.15.1 wrote.

## Never regenerate these from a later tree

**The commit these were built from is the entire value of the fixture.** The
files are a *record of the past*, not expected output. Regenerating them with a
binary that knows about dependencies turns the test that consumes them from

> *dependencies did not disturb a dependency-free project's on-disk contracts*

into

> *the current binary agrees with itself*

A refresh does not fix a stale fixture. It silently deletes the only evidence
that the Principle 9 stabilization freeze held for this feature, and leaves a
green test behind.

- **If a fixture mismatches, the binary is wrong, not the fixture.** A changed
  lock byte order, a changed state field, or a changed declaration hash on a
  dependency-free project is a breaking change.
- **If a mismatch is intended** (an approved additive change with its own
  migration story), add a *second* fixture directory for the new baseline
  commit and keep this one.
- `tools/generate.py` refuses to write into a directory containing a
  `README.md`, so it cannot be aimed at this directory by accident, and it
  needs `GOLDEN_GRIM` pointing at a 0.15.1 binary.

## The declared surface

The project declares every shape the upgrade has to keep reading:

| Shape | Declaration |
|---|---|
| Direct skill, registry | `reg-skill` |
| Direct skill, path | `local-skill` |
| Direct mcp | `reg-mcp` |
| Direct hook (flag on) | `reg-hook` |
| Registry bundle | `reg-bundle` (members `reg-skill`, `bundled-rule`) |
| Member shared by two provenances | `reg-skill`: declared directly and named by `reg-bundle` at the same identifier |
| Single-provenance member | `bundled-rule` (registry bundle), `local-bundled-agent` (local bundle) |
| Local bundle | `local-bundle` (`project/bundles/local-bundle.toml`) |

The global scope (`global-grimoire.toml`) declares direct artifacts only
(skill, rule, agent, mcp, hook). That is deliberate: 0.15.1 locks a registry
bundle online only (its bundle manifest fetch refuses offline, exit 69), so
the offline variants below can only exercise a bundle-free scope.

## Layout and roles

| Path | Role |
|---|---|
| `golden/project.grimoire.lock`, `golden/global.grimoire.lock` | **Assertion targets.** Locks after one lock+install pass per scope. |
| `golden/state.project.json`, `golden/state.global.json` | **Assertion targets.** Install state. |
| `golden/declaration_hash.json` | **Assertion target.** Both scopes' `declaration_hash` plus `declaration_hash_version`. |
| `golden/project.after-remove.grimoire.lock`, `golden/state.project.after-remove.json` | **Assertion targets.** The project after `grim remove bundle reg-bundle`: exactly what 0.15.1 dropped. |
| `golden/tag-cache/` | A warm `$GRIM_HOME` tag cache (`tags/`) **plus** the registry manifest blobs the store held (`blobs/`). Offline store-hit case. |
| `golden/tag-cache-no-manifests/` | The same `tags/`, no `blobs/`. Offline store-miss case. |
| `provenance/status.{project,global}.json` | `grim status --format json` as 0.15.1 reported it. Absolute paths are rewritten to `<ENVROOT>`. Compare `source` only. |
| `registry/pushed.json` | The six registry artifacts, recorded byte for byte (manifest verbatim, layer as hex). Replaying them reproduces every manifest digest in the locks. |
| `registry/release_baseline.json` | The manifest bytes 0.15.1 produced by releasing one dependency-free skill (`source_files`) from a directory outside any git tree, so the manifest carries no revision or created annotation. A later grim releasing the same source to the same `reference` must produce the same bytes. |
| `project/`, `global-grimoire.toml` | The declaration inputs. |
| `commands.txt` | The exact 0.15.1 commands that produced the above. |

Registry host is **`localhost:5000`**, embedded in every `pinned =` value and in
both declaration hashes. A consumer without a registry there skips rather than
normalizing: rewriting the host would rewrite the bytes under test. Repos live
under `grim-golden/pre-dependencies-cdeedcaa/`.

## Exactly how these were produced

```sh
# 1. A detached worktree at the release commit, submodules initialized.
git worktree add --detach .agents/worktrees/v0151 cdeedcaa
git -C .agents/worktrees/v0151 submodule update --init --recursive

# 2. Release build, target dir inside that worktree.
cd .agents/worktrees/v0151
CARGO_TARGET_DIR=$PWD/target cargo build --release        # -> grim 0.15.1

# 3. An empty OCI registry on localhost:5000 (a throwaway zot was used).

# 4. Generate into a scratch directory, never into this one.
GOLDEN_GRIM=$PWD/target/release/grim python3 tools/generate.py /scratch/golden-out
```

Per scope there is exactly **one** lock+install pass, because a second pass
would make the mcp splice target pre-exist and 0.15.1 would record
`"adopted": true` instead of `false`. The project install passes
`--trust-hooks` (a per-run answer that writes no consent record); the
`remove` runs last, after every capture.

Environment during generation: fresh empty `HOME` holding `.claude/`,
`.copilot/`, `.codex/`; fresh absolute `GRIM_HOME`; `XDG_CONFIG_HOME=$HOME/.config`;
`GRIM_INSECURE_REGISTRIES=localhost:5000`; `NO_COLOR=1`; `PATH=/usr/bin:/bin`;
everything else unset. `--client claude` is pinned because client detection
reads the ambient filesystem.

## Things a consumer must know

- **`generated_at` is the only time-varying line in the locks.** Seed the
  golden lock into the project before `grim lock` and it is preserved
  byte for byte, or blank the line on both sides.
- **The project hook state embeds the workspace path.** The hook payload path
  in `state.project*.json` and `provenance/status.project.json` is
  `hooks/payload/<64 hex>/reg-hook`, a SHA-256 of the workspace directory.
  Replace that 64-hex component with a placeholder on both sides before
  comparing. Nothing else in the state varies.
- **`project.after-remove.grimoire.lock` starts with a bare `rule = []`
  line** before `[metadata]`. That is what 0.15.1 wrote when the last rule
  left the lock; keep it. `grim remove` touches no install state, so
  `state.project.after-remove.json` equals `state.project.json`.
- **The tag-cache registry directory is stored as `localhost_5000`**, because
  a `host:port` directory name cannot be checked out on Windows (grim itself
  writes `host_port` there). A Unix consumer copies it to `tags/localhost:5000/`
  before use.
- **Offline cases lock the global scope.** Seed the golden global lock, copy
  one variant into `$GRIM_HOME`, run `grim lock --global --offline`. With the
  other variant absent both reproduce the golden lock, and with neither
  present 0.15.1 exits 69. `tools/verify.py` runs all three.

## Determinism and self-sufficiency, measured

Three independent generations were diffed: only `generated_at` (three lock
files) and the workspace-key component above varied. The fixture does not
depend on the registry state it was generated against: the registry was
replaced by an empty one, `tools/replay.py` loaded the recorded bytes (every
replayed digest matched), and `tools/verify.py` re-ran against the 0.15.1
binary, seeded and unseeded:

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
| `tools/generate.py` | Produced everything here. Stdlib only; does not import `test/src/*`, so the bytes cannot move when the harness moves. Refuses to write into this directory. |
| `tools/replay.py` | Pushes `registry/pushed.json` into a registry verbatim, verifying each replayed digest. |
| `tools/verify.py` | The check whose output is quoted above. `GOLDEN_SEED=0` selects the unseeded mode. |

None of these is a pytest test; `test/pyproject.toml` sets
`testpaths = ["tests"]`.
