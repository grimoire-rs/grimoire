# Pre-mods golden fixtures

Committed **data**, not code. These bytes were written by the **grim 0.16.0
release binary** (tag commit `1972e3b9`, `grimoire-x86_64-unknown-linux-gnu.tar.gz`,
SHA-256 `e55f7d40271418f6fe64fa4ef6d557a1a622d4fc240885d8687bff7a3ba8644f`),
the last release before the `mod` artifact kind. A later binary is proved to read,
re-lock and modify what a mod-free grim wrote. No build was needed: the release
binary generated them.

## Never regenerate these from a later binary

The binary is the whole value of the fixture. Regenerating from a grim that knows
mods turns the consuming test from "the mod kind did not disturb a project that
never declared one" into "the current binary agrees with itself". A mismatch means
the binary is wrong, not the fixture; an intended change gets a second fixture
directory for its own baseline.

`tools/generate.py` refuses to write into a directory containing a `README.md` and
needs `GOLDEN_GRIM` naming the 0.16.0 binary. `tools/replay.py` re-uploads the
recorded registry bytes.

## The declared surface

One project and one global scope declaring every pre-mod kind: skill, rule, agent,
mcp, hook, settings, and (project only) a registry bundle and a path bundle, plus a
path skill. Registry refs are pinned to `localhost:5000` under
`grim-golden/pre-mods-1972e3b9/`.

- `golden/`: assertion targets (both locks, both install states, the lock and state
  after `remove bundle reg-bundle`, the declaration hashes, the 0.16.0 consent
  record, and two warm tag caches for the offline cases).
- `registry/pushed.json`: every pushed manifest and layer, byte for byte.
- `provenance/`: `status` output, kept as a record and never compared (absolute
  paths, version strings).
- `commands.txt`: the exact command sequence, one lock-and-install pass per scope.

The consumer is `test/tests/test_golden_pre_mods.py`. It installs with
`--trust-workspace`, because the consent record this binary wrote lives where a
newer grim no longer reads it.
