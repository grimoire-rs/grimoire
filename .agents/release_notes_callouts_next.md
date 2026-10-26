# Release Notes Callouts — Next Release

`task release:prepare` regenerates `CHANGELOG.md` from Conventional
Commits via git-cliff, so any hand-written nuance gets overwritten on
the next run. This file is the durable staging area: paste these
bullets into the changelog (or release notes) during the human-review
step of `task release:prepare`, before tagging.

- `grim login` now verifies the credential against the registry by
  default; `--no-verify` restores store-only behavior. New failure
  exits: 80 (rejected), 69 (registry unreachable), 81 (explicit
  `--verify` while offline). Nothing is stored on failure. CI scripts
  relying on unconditional exit 0 need `--no-verify` or explicit
  handling of the new exit codes.
- First `grim update` after upgrading reaps stale, unmodified outputs
  of clients previously dropped from an explicitly-set
  `[options].clients` (locally modified outputs are preserved unless
  `--force`). Autodetected client sets are never reaped.
- The `codex.` metadata-key prefix is now reserved as a tool namespace;
  previously-plain `codex.*` keys become vendor-specific (see
  `docs/src/vendor-metadata.md`).
- `grim install` / `grim add` JSON `target` field is `null` when every
  selected client declines the artifact kind (e.g. rules with only
  Codex selected); consumers with a strict non-nullable string schema
  must widen it to accept `null`.

- Experimental hooks now run through a native `grim-hook` launcher that
  `grim install` writes to `$GRIM_HOME/hooks/grim-hook`, replacing the
  `bin/grim-hook` shell script and the `grim hook run` command (both removed).
  Re-run `grim install` in each workspace that uses hooks: until you do, that
  workspace's hooks do not fire. The old registration exits 0, so nothing is
  blocked meanwhile. See `docs/src/content/docs/hooks-troubleshooting.md#after-upgrade`.
- A failure inside grim's own hook dispatch now surfaces to the client instead
  of being swallowed as exit 0. The launcher exits 1, 64, 65, 74 or 79 (never
  2) and each client applies its own policy: Copilot denies a failing
  `preToolUse`, Claude Code and Codex show an error and proceed. A deleted
  launcher or dispatch table now shows until the next `grim install`. Benign
  states (flag off, revoked, unknown root, no match) still exit 0. See
  `docs/src/content/docs/hooks.md#launcher-exit`.
- Codex users approve grim's gatekeeper hooks once more in `/hooks`: the
  registered command text changed, and Codex keys its approval on that text.
  `grim install` names the affected hooks.
- A hook bound as `grim-hook` or `grim-hook.exe` is now refused with cause
  `binding-name-reserved`; both names were legal before. Rename the binding.
  `grim hook list` also gains a `launcher` object (`path`, `sha256`, `state`)
  and `launcher-unwritable`, `launcher-path-occupied`, `launcher-path-unquotable`
  and `grim-home-shared` joined the cause list.
- `GRIM_LOG` on the launcher is no longer a tracing filter: any value
  containing `debug` or `trace` turns on its debug lines, and nothing else
  changes the output.
- A concurrent `grim install` now meets the `dispatch.json.lock` for longer,
  because the lock is held across writing the launcher and the table, and
  reports `dispatch-lock-held` (transient) when it loses.
