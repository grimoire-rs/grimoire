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
