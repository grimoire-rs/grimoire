# TODO

All items from the 2026-06-11 sweep are addressed (see
`.claude/artifacts/plan_todo_overnight.md` for decisions and commits).

## Open

### Nested Errors

In a project without gimoire.toml the same error is nested three times.
Reads very akward, maybe just the case if no grimoire.toml is found.
Maybe subject to the error reporting as is, and other failures produce same kind.

### TUI Init

Running the TUI without a grimoire.toml in local or global mode should prompt
before starting if the ./grimoire.toml or ~/.grimoire.toml is missing respectively.
Should ask for initialization for a specfic repository.
The default value of the prompt should be the configured GRIM_DEFAULT_REGSITRY, if none set, let it empty.
On cancel close the TUI.

### Search inquivelance

Search results via TUI and grim search are different.
grim search should yield the same results as TUI search.
TUI search works fine, but grim search is missing results.

### Snapshot default registry

On init, snapshot the default registry GRIM_DEFAULT_REGISTRY into the toml config default_registry.
ATM this is only set if --registry is explicilty set.

## Follow-ups (deferred from review, warn/suggest tier)

- Search: multi-term queries whose terms only match summary/description/
  keywords can still miss repos beyond the 500-repo browse window (the
  longest-term prefilter is name-scoped). Truncation is now visible in CLI
  (stderr warning) and TUI (legend hint); a pagination/multi-fetch rework
  would close the gap fully.
- Search JSON report: add a machine-readable `truncated` field (currently
  stderr-only) so scripts can detect incomplete results.
- TUI: background task panics are reaped but deliberately swallowed
  (raw-mode terminal, no stderr); consider a status-line error tally.
- TUI: string truncation in `fit()` counts chars, not terminal display
  width (pre-existing; matters for wide glyphs).
- TUI: selected-clients line degrades to detection when config has invalid
  client names while install errors hard — acceptable as best-effort
  display, revisit if confusing.
- TUI: synchronous lock/install-state reads run on the event loop each
  drain/schedule pass — fine at current sizes, move off-loop if it grows.
- TUI: bundle rows get no floating-tag "outdated" re-check (the lock
  records member pins but no bundle digest, so there is no baseline to
  compare the registry's bundle tag against). Member rows still re-check
  individually; recording the bundle digest in lock provenance would
  close the gap.
