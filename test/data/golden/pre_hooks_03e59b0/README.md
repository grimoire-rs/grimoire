# Pre-hooks golden fixtures — contract C-015

Committed **data**, not code. These bytes were produced by a `grim` built from
**`03e59b053de60173a866c783581a999ff04f4e12`** (`03e59b0`, `main`, the last
commit before any `ArtifactKind::Hook` work), and they exist so a *later*
binary can be proved byte-compatible with it.

Contract, from `.agents/plans/plan_hooks_artifact_kind.md` › **C-015**:

> A hook-free project's `grimoire.lock`, `state/global.json` and
> `declaration_hash` are byte-identical to golden fixtures generated at
> `03e59b0` and committed before WP-E's stub lands — asserting
> current-binary-equals-current-binary is vacuous. `DECLARATION_HASH_VERSION`
> stays `1`.

## ⛔ Why you must not regenerate these from a later tree

**The commit these were built from is the entire value of the fixture.** They
are not "expected output" — they are a *record of the past*. Regenerating them
with any binary that knows about hooks turns the test that consumes them from

> *the hook kind did not disturb a hook-free project's on-disk contracts*

into

> *the current binary agrees with itself*

which is exactly the vacuity C-015 was written to forbid. The plan calls this
out by name. A refresh does not "fix a stale fixture"; it **silently deletes the
only evidence that the Principle 9 stabilization freeze held for this feature**,
and it does so while leaving a green test behind — the worst possible failure
mode, because nothing looks broken.

So:

- **If a fixture mismatches, the binary is wrong, not the fixture.** Investigate
  the diff. A changed lock byte order, a changed state field, or a changed
  declaration hash on a hook-free project is a Principle 9 breaking change.
- **If a mismatch is genuinely intended** (an approved, documented additive
  change with its own migration story), do not regenerate: add a *second*
  fixture directory for the new baseline commit and keep this one, so the
  history of what changed when stays readable.
- `tools/generate.py` refuses to write into a directory containing a
  `README.md`, specifically so it cannot be aimed at this directory by accident.

## What is asserted, and what is not

| Path | Role |
|---|---|
| `golden/project.grimoire.lock` | **Assertion target.** Project-scope lock. |
| `golden/global.grimoire.lock` | **Assertion target.** Global-scope lock. |
| `golden/state.project.json` | **Assertion target.** Project install state (`<workspace>/.grimoire/state.json`). |
| `golden/state.global.json` | **Assertion target.** Global install state (`$GRIM_HOME/state/global.json`). |
| `golden/declaration_hash.json` | **Assertion target.** Both scopes' `declaration_hash` plus `declaration_hash_version`. |
| `provenance/status.{project,global}.json` | **Not an assertion target** — absolute paths are rewritten to `<ENVROOT>`. Kept as a human-readable record of what the baseline reported. |
| `provenance/context.project.json` | **Not an assertion target** — it embeds `"version": "0.13.0"`, which changes at every release, and absolute paths. Provenance only. |

Why the lock matters most: the hook kind inserts a `[[hook]]` array **between
`[[mcp]]` and `[[bundle]]`** in the lock, and `"hooks"` **between `"bundles"` and
`"mcp"`** in the JCS declaration document. Both golden locks therefore contain
`[[mcp]]` *and* `[[bundle]]` sections — the adjacency the contract is about is
present in the data, not merely implied.

## The declared surface

A hook-free project declaring **all five existing kinds**, each in both a
registry-sourced and (where the kind supports it) a path-sourced form:

| Kind | Registry-sourced | Path-sourced |
|---|---|---|
| skill | `reg-skill` (+ `bundled-skill` via the bundle) | `local-skill` |
| rule | `reg-rule` (+ `bundled-rule` via the bundle) | `local-rule` |
| agent | `reg-agent` | `local-agent` |
| mcp | `reg-mcp` | — (`[mcp]` rejects path values by design) |
| bundle | `reg-bundle` (two members) | — (a local bundle's members must be absolute registry refs) |

Inputs, all committed:

- `project/grimoire.toml` + `project/{skills,rules,agents}/…` — the project declaration and its path sources.
- `global-grimoire.toml` — the global-scope declaration (`$GRIM_HOME/grimoire.toml`).
- `registry/pushed.json` — the five registry artifacts recorded **byte-for-byte**
  (manifest bytes verbatim, layer as hex). Replaying them reproduces every
  manifest digest in the golden locks exactly, with no dependence on `grim`, on
  `test/src/registry.py`, or on this repo's packer.

## Exactly how these were produced

```sh
# 1. A worktree at the pre-hooks commit, submodules initialized.
git worktree add .agents/worktrees/golden-baseline 03e59b0
git -C .agents/worktrees/golden-baseline submodule update --init
#    external/docker_credential  8e89cd0e4fb5a7d74fa25f12024eafb90cd7a88b
#    external/rust-oci-client    7f3d0b6c8041bb9902e412dcb452aee749d2031e

# 2. Debug build, target dir OUTSIDE the shared volume (which was at 97%).
cd .agents/worktrees/golden-baseline
CARGO_TARGET_DIR=/some/roomy/disk/target CARGO_INCREMENTAL=0 cargo build --bin grim
#    -> grim 0.13.0

# 3. An empty OCI registry on the port the acceptance suite uses.
docker run -d --rm -p 5000:5000 --name grim-golden-registry registry:2

# 4. Generate into a scratch directory (never into this one).
GOLDEN_GRIM=/path/to/baseline/grim python3 tools/generate.py /tmp/golden-out
```

`tools/generate.py` then, in one pass per scope:

```
$ grim lock                                  # cwd=<project>
$ grim install --client claude               # cwd=<project>
$ grim lock --global                         # cwd=<GRIM_HOME>
$ grim install --global --client claude      # cwd=<GRIM_HOME>
```

(also in `commands.txt`.) Exactly **one** lock+install pass per scope, because
that is what a consuming test does: a second pass would make the mcp splice
target pre-exist, and grim would then record `"adopted": true` instead of
`false` — a state file describing an adoption, not a clean install.

### Environment pinned during generation

| Variable | Value |
|---|---|
| `HOME` | a fresh empty dir, containing only `.claude/`, `.copilot/`, `.codex/` (so global-scope client detection is deterministic) |
| `GRIM_HOME` | a fresh empty dir, **absolute** |
| `XDG_CONFIG_HOME` | `$HOME/.config` |
| `GRIM_INSECURE_REGISTRIES` | `localhost:5000` |
| `NO_COLOR` | `1` |
| `PATH` | `/usr/bin:/bin` |
| everything else | **unset** — the ambient environment is stripped |
| registry | `localhost:5000`, plain HTTP, repos under `grim-golden/pre-hooks-03e59b0/` |
| `--client` | `claude`, pinned explicitly — client auto-detection reads the ambient filesystem, which is exactly the environment dependence a golden fixture must not carry |

`GRIM_HOME` **must be absolute.** During development a relative value made grim
resolve it against the process CWD and create nested `.../grim-home/...`
directories — the same `env::grim_home()` behaviour the plan records as finding
**B1**.

## Determinism — measured, not assumed

Three independent generations into clean environments (`gen1`, `gen2`, `gen3`;
`gen3` deliberately separated in time so a same-second collision could not hide
a timestamp difference), diffed recursively:

```
diff -r gen1/golden gen2/golden                       -> no differences
diff -r gen1/registry gen2/registry                   -> no differences
diff -r gen1/project  gen2/project                    -> no differences
diff -r gen1/golden gen3/golden
  project.grimoire.lock:6  generated_at = "2026-08-17T07:14:44Z"
                        -> generated_at = "2026-08-17T07:15:01Z"
  global.grimoire.lock:6   (the same single line)
```

**`generated_at` is the only non-deterministic byte in the whole fixture.** Both
install-state files, both declaration hashes, all five manifest digests, both
path-source content hashes and every `content_hash` reproduced identically.

Nothing else needed normalizing: install state is entirely anchor-relative
(`"anchor": "workspace"` / `"claude-root"` / `"claude-user-dir"` plus a relative
path), so it carries no absolute path, no hostname and no timestamp.

### Handling `generated_at` — two proven consumption strategies

**Strategy A — seeded, zero normalization (recommended).** Copy the golden lock
into the project *before* running `grim lock`. `lock_io::save` preserves
`generated_at` verbatim when the resolved content of every artifact is unchanged
(`lock_io.rs` module docs), so a hook-free project reproduces the golden lock
**byte-for-byte including the timestamp**. If content did change, the timestamp
is bumped *and* the content differs — the test fails, correctly.

**Strategy B — unseeded, one normalized line.** Start with no lock and blank the
single `generated_at = "…"` line on both sides before comparing. Nothing else is
normalized.

Both were executed against these committed bytes with the baseline binary:

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

### Self-sufficiency: proven against a *fresh, empty* registry

The fixture does not depend on the registry state it was generated against. The
`registry:2` container was destroyed and replaced with an empty one, the recorded
bytes replayed, and the whole comparison re-run:

```sh
docker run -d --rm -p 5000:5000 --name <fresh> registry:2
python3 tools/replay.py .            # every replayed digest matched the record
GOLDEN_GRIM=<baseline> python3 tools/verify.py . /tmp/w
   ... all six IDENTICAL
```

## Consuming these from `test/`

`.claude/rules/subsystem-tests.md` prescribes no fixture-data location, so this
directory follows the plan's suggested path. A consuming acceptance test needs:

1. A reachable registry at **`localhost:5000`** — the host string is embedded in
   every `pinned = …` value in both golden locks. That is the acceptance suite's
   own default (`test/conftest.py` starts `registry:2` there when nothing
   answers). **If the session registry host differs, skip rather than
   normalize:** rewriting the host would also rewrite the bytes under test.
2. `tools/replay.py` to load the recorded artifacts (idempotent — re-pushing
   identical bytes is a no-op at the registry).
3. A fresh `GRIM_HOME` and `HOME` per the table above, `--client claude` pinned.
4. The four `grim` invocations in `commands.txt`.
5. Byte comparison against `golden/`, using Strategy A or B.

## Tools

| File | Purpose |
|---|---|
| `tools/generate.py` | Produced everything here. Stdlib only; deliberately **does not** import `test/src/*` so the fixture bytes cannot move when the harness moves. Refuses to write into this directory. |
| `tools/replay.py` | Pushes `registry/pushed.json` back into a registry, verbatim, verifying each replayed digest against the record. |
| `tools/verify.py` | The determinism/identity checker whose output is quoted above. `GOLDEN_SEED=0` selects Strategy B. |

None of these is a pytest test; `test/pyproject.toml` sets
`testpaths = ["tests"]`, so nothing here is collected.
