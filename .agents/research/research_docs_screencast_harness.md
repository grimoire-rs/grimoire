# Research: screencast recording harness

**Date:** 2026-09-06
**Run:** hex-plan xhigh, docs redesign (`.agents/plans/plan_docs_site_redesign.md`)
**Phase:** Discover, explorer recordings
**Consumers:** `.agents/specs/design_docs_site_redesign.md`, the plan.

# Recording harness discovery

## 1. How a cast is produced today

- **Entry point**: `task demo` (`test/taskfile.yml:52-70`) — builds grim then runs
  `uv run pytest recordings/ -v`. Outside pytest's `testpaths = ["tests"]`
  (`test/pyproject.toml:7`), so `task test`/`task verify` never collects it —
  explicit-only, not run in CI (`.github/workflows/**` has zero references).
- **Tool**: a bespoke ~230-line recorder (`test/recordings/cast_recorder.py:91-237`),
  not asciinema-rec. Spawns `/bin/bash --norc --noprofile` via `pexpect`
  (`cast_recorder.py:136-147`), disables echo, sets a sentinel `PS1`, then
  `run_command()` types each char with a simulated typing delay (0.04s) and
  captures real-time output timestamped against wall-clock elapsed time
  (`cast_recorder.py:158-227`). Emits asciicast v2 JSON directly
  (`cast_recorder.py:57-70`) — no library dependency for the format itself.
- **Driver test**: `test/recordings/test_record_demo.py:64-99`. Terminal size
  180x24 (`cast_recorder.py:113`, sized for grim's widest table row — see
  comment). Binary: `grim_binary` session fixture (built `grim`, PATH-prepended
  so the recording shows `grim init` not an absolute path, line 87). Sandboxed
  via `GrimRunner` (`test/src/runner.py:36-77`): isolated `GRIM_HOME`, `HOME`/
  `USERPROFILE`, `XDG_CONFIG_HOME`, minimal env passthrough — the acceptance
  suite's normal isolation, reused as-is.
- **Registry**: the **real, public** `ghcr.io/grimoire-rs/skills/grim-usage`
  (`test_record_demo.py:14-29,42`), not the local `registry:2` fixture the rest
  of the suite uses — verified anonymously pullable via GHCR token probe
  (comment, lines 16-20). Requires network; would fail offline/in an air-gapped CI.
- **Determinism**: typing timestamps are deterministic (fixed delay); output
  timestamps are wall-clock (network-dependent) — not reproducible byte-for-byte
  across runs, hence the comment that a cold `grim add` ran 13.6s on the
  recording host once (`docs/theme/index.hbs:531-534`). No digests/timestamps
  baked into visible text since a short flat `tempfile.TemporaryDirectory`
  (not pytest's deep `tmp_path`) keeps the printed path short and stable
  (`test_record_demo.py:65-70`). Colors: grim emits no ANSI (confirmed by
  manual PTY probe, `cast_recorder.py:10-14`), so no color capture needed.
- **Runtime**: not measured in-repo; gated by real network round-trips to GHCR
  (`grim add`), so multi-second, variable.
- **Review**: `docs/src/demo.cast` is **committed** (4.2KB, 129 lines) and
  reviewed via `git diff docs/src/demo.cast` after re-running (task summary,
  `test/taskfile.yml:60-63`; also documented in `index.hbs:42-45`). A structural
  regression guard, `assert_tables_column_aligned` (`cast_recorder.py:251-292`),
  fails the test if any table's columns misalign — guards against reintroducing
  post-hoc string substitution.

## 2. What's needed for 11 use-case casts

- **Per-cast script**: today it's one hardcoded Python test
  (`test_record_demo.py`) with a fixed command sequence. Scaling to 11 would
  need each cast as its own `test_record_*` function (or a small
  data-driven loop) — no existing script/YAML format for command lists exists
  yet; `CastRecorder.run_command()` is the only building block.
- **TUI recording: not supported today.** The harness only drives a bash shell
  and captures line-buffered command output via `pexpect.spawn` + sentinel
  prompt matching (`cast_recorder.py:134-227`). It never sends raw keystrokes
  mid-screen or reads a redrawing TUI frame — `grim tui`/`browse-tui` would
  need new key-injection + full-screen-diff capture logic; nothing here does
  that. This is the biggest capability gap for the `browse-tui` screencast.
- **Global-scope sandbox**: exists and is reusable as-is —
  `GrimRunner`'s isolated `HOME`/`USERPROFILE`/`XDG_CONFIG_HOME`
  (`test/src/runner.py:44-64`) already fakes `~/.claude`, etc. for any cast
  needing global-scope installs.
- **Local-registry fixture for casts that must not hit the public registry**:
  exists — the acceptance suite's session-scoped `registry` fixture
  (`.claude/rules/subsystem-tests.md:31`, `test/conftest.py`) starts a real
  `registry:2` container, plus `make_artifact`/`make_bundle`/`write_config`
  helpers (`subsystem-tests.md:52-58`) to push fixture skills/rules/bundles.
  `GRIM_INSECURE_REGISTRIES` is auto-set by `GrimRunner` when the registry
  host differs from the built-in HTTP allowlist (`runner.py:73-77`). Casts like
  `own-index`/`registries`/`team-ci` should use this instead of the public
  GHCR ref the current demo uses.
- **Idle-time trimming**: not done at record time — trimming happens at
  **playback** via the player's `idleTimeLimit: 1` option (`index.hbs:536`),
  not baked into `demo.cast` itself. New casts should rely on the same
  player option rather than re-implementing trimming in the recorder.

## 3. Astro embedding

Current site is mdBook, not Astro (`docs/theme/index.hbs:47` — mdBook renders
this template). Embedding pattern to port: a container `<div id="demo-player">`
plus a script block calling
`AsciinemaPlayer.create('<path>.cast', containerEl, { loop, idleTimeLimit, preload })`
(`index.hbs:519-538`) after loading `asciinema-player.min.js` (vendored, v3.17.0,
Apache-2.0, integrity-checked against npm's sha512 before commit — comment,
`index.hbs:37-40`) and `asciinema-player.css`. Astro would need the same two
vendored files copied in and a client-side script (`<script>` or a small
island) making the same `.create()` call per page.

## 4. Risks

- **Network in CI**: `task demo` is never run in CI today; if the 11-cast
  workflow adds a CI step, the public-GHCR-dependent casts introduce real
  network flakiness — prefer the local `registry:2` fixture for all but the
  one cast that's specifically about the real public index.
- **Flakiness**: wall-clock-timestamped output capture means re-recording
  the same cast twice yields different (but functionally equivalent) timing —
  acceptable for a committed, manually-reviewed asset, risky if asserted byte-exact.
- **Cast size**: current cast is 4.2KB/129 lines; 11 of them stay small (KB-scale
  text, no video) — no LFS need. `.gitattributes` only routes binary image
  formats (jpg/png/gif) to LFS, not text.
- **TUI gap** is the standalone blocker for `browse-tui`, `inspect` if those
  render as full-screen TUI rather than scrollable CLI output — needs new
  harness code, not a reuse of `cast_recorder.py`.
