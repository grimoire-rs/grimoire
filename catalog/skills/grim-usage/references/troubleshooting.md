# Troubleshooting

You loaded this file because a grim command failed and you need to read
the exit code, diagnose the cause, or get past an integrity gate.

Contents: [Exit Codes](#exit-codes) · [Exit 65](#exit-65-data-errors) ·
[Integrity Gates](#integrity-gates) ·
[Kind Inference](#the-kind-inference-gotcha) ·
[Offline Failures](#offline-failures) · [Auth Failures](#auth-failures)

## Exit Codes

grim's exit codes follow BSD `sysexits.h`, with grim-specific codes from
79 up. `case $?` on these values is the supported automation contract —
no stderr parsing needed:

| Code | Class | Typical triggers |
|---|---|---|
| 0 | Success | — |
| 1 | Failure | unclassified fall-through |
| 64 | Usage error | bad invocation; `grim init` when the config already exists; invalid `grim fetch` flag combinations; a release/publish tag colliding with the reserved `__grimoire` namespace |
| 65 | Data error | validation failures of any kind — see below |
| 69 | Unavailable | registry unreachable, resolve timeout |
| 74 | I/O error | filesystem read/write failure (non-permission) |
| 75 | Temporary failure | another grim process holds the lock; credential-helper timeout — retry |
| 77 | No permission | permission denied anywhere in the chain |
| 78 | Config error | malformed `grimoire.toml`/lock, no registry for `grim login`/`logout`, bundle conflict, unsupported client, credential helper missing |
| 79 | Not found | tag/manifest/blob 404, no config discovered, lock missing; a missing description companion on `grim fetch --description` |
| 80 | Auth error | registry authentication failed |
| 81 | Offline blocked | `--offline`/`GRIM_OFFLINE` blocked a network operation (deliberate policy, distinct from 69) — includes `fetch`/`describe` against an uncached reference, which is 81, not 79 |

Under `--format json`, a failure emits a `{"error": {code, exit,
message}}` document on stdout; some failures add a machine-readable
`reason` field: `stale-lock` (exit 65 — a partial `grim update <name>`
was refused; retry with a full `grim update`), `modified` (exit 65 — an
install refused a locally modified artifact; retry with `--force`),
`untracked-destination` (exit 65 — an install refused to clobber an
unrecorded pre-existing destination; retry with `--force`), `no-config`
(exit 79 — a project-scope command found no `grimoire.toml` walking up
from the working directory), and `locked` (exit 75 — another grim
process holds the config-file lock). New reasons are additive — treat an
unknown one as absent. A reason also carries an optional `retryable: true`
sibling field when a plain re-run is expected to succeed once the
transient condition clears — today only `locked` sets it. Full list: the
[JSON interface][json-interface] docs page.

## Exit 65: Data Errors

65 is the validation class — the artifact or input itself is wrong.
Common causes, roughly in order of frequency:

- **Invalid name.** Names are lowercase letters, digits, hyphens, and
  periods only; max 64 chars; no leading, trailing, or adjacent
  separators (`--`, `..`, `.-` all invalid). Applies to skill directory
  names, rule/agent file stems, and the frontmatter `name`.
- **Skill structure.** Missing `SKILL.md`; missing or unclosed `---`
  frontmatter fence; missing `name` or `description`; frontmatter
  `name` not equal to the directory name; description empty or over
  1024 chars.
- **Agent frontmatter.** Agents *require* frontmatter with `name`
  (== file stem) and `description`.
- **Catalog metadata.** `keywords` written as a list instead of a
  comma-separated string; `repository` not an `https://` URL.
- **Vendor metadata.** A known `<vendor>.<field>` key with a bad
  literal (e.g. a non-`"true"/"false"` boolean, a value outside a
  closed enum set).
- **Release tag errors.** Reference with no tag; invalid version
  string; exact-version tag already exists at a different digest
  (re-release with `--force` only if you mean to rewrite it).
- **Integrity mismatch** on installed content (see below).
- **Oversize blob.** A registry serving more bytes than its manifest
  declared for a layer aborts the download mid-transfer rather than
  buffering an unbounded body. Reachable from `grim fetch` (also gated by
  an 8 MiB pre-download check on the declared size) and from
  `grim install`/`grim update` (no separate flag — any locked artifact's
  download can hit this on a lying descriptor).
- **Git provenance unavailable.** Building or releasing with `--git`
  (opt-in commit provenance) from a path that is not a git repository,
  or with `git` missing from `PATH`, is a data error — the flag
  hard-fails when it cannot read provenance. Note too that `--git` makes
  a re-release from a different commit change the digest, refused without
  `--force`. Confirm with `grim release --help`.

Fix the named input and re-run `grim build` until it exits 0 before
trying `grim release` again.

## Integrity Gates

grim never silently overwrites or deletes work you did locally:

- `grim install` **refuses** to overwrite a locally modified artifact;
  re-run with `--force` to overwrite it deliberately.
- `grim install` also **refuses** to overwrite a destination it has no
  record of (a hand-authored same-named skill dir, rule file, or MCP
  config entry) — `--force` overwrites and records it. Identical
  content is adopted into the record instead of refused, so a lost
  state file with intact rendered files repairs itself on reinstall.
- `grim add` installs-on-add through the same gates and takes the same
  `--force`: re-running the *same* `grim add <ref> --force` overwrites a
  modified artifact (re-adding the same reference is an idempotent
  re-declare, so nothing else changes).
- `grim update` prunes artifacts that dropped out of the lock, but a
  locally modified orphan is **kept** and reported as `kept-modified`;
  `--force` prunes it anyway.

`grim status` shows which artifacts are `locally modified`. If a managed
file needs permanent local changes, copy it out of the managed location
instead of fighting the gate — managed files are owned by the lock.

## The Kind-Inference Gotcha

Kind is inferred from shape — and agents break the pattern:

- At `build`/`release`: a directory packs as a skill, `.md` as a rule,
  `.toml` as a bundle. A bare `.md` is **always a rule** by shape — an
  agent requires `--kind agent` explicitly. Forgetting it is not an
  error: the file silently publishes as a rule (grim warns when a rule
  carries both `name` and `description` — heed that warning). Likewise a
  `.toml` is **always a bundle** by shape — an MCP server descriptor
  requires `--kind mcp` (grim errors with a `--kind mcp` hint when the
  TOML carries a `[server]` table).
- At `add`: kind is read from the published manifest's kind metadata
  (the `com.grimoire.kind` annotation; legacy `artifactType` on older
  artifacts). A non-Grimoire image cannot be inferred — `add` errors
  and asks for `--kind`.

## Offline Failures

Exit 81 means offline mode itself blocked the operation — deliberate
policy, not an outage (that is 69). Either drop `--offline` / unset
`GRIM_OFFLINE`, or warm the cache online first (`grim lock`, then go
offline) — see [registries.md](registries.md). A floating tag that was
never resolved online cannot be resolved from the cache.

## Auth Failures

Exit 80 is the registry rejecting your credential. Things to know:

- `grim login` verifies the credential against the registry **before**
  storing it by default — a wrong password fails right at login with
  exit 80 and nothing stored (unreachable registry: 69; explicit verify
  request while offline: 81). A skipped verification (store-only mode,
  offline) surfaces a wrong password on the next pull or push instead.
  Re-login with a fresh token; confirm flags with `grim login --help`.
- Credentials are read from `$DOCKER_CONFIG/config.json` — a plain
  `docker login` works too; the store is shared.
- A configured credential helper that is not on `PATH` is exit 78, not
  80; so is refusing to store plaintext without
  `--allow-insecure-store`.
- Private registries return 404 (not 403) for unauthorized repos on
  some hosts — an unexpected 79 on a private reference can be an auth
  problem in disguise. Try `grim login` first.

## Further Reading

- [Command reference][commands] — per-command behavior, including
  `--force` semantics on install and update.
- [Authentication][auth] — credential resolution order and storage.
- [Configuration][config] — config/lock shape behind the 78-class
  errors.

[commands]: https://grimoire.rs/commands.html
[auth]: https://grimoire.rs/authentication.html
[config]: https://grimoire.rs/configuration.html
[json-interface]: https://grimoire.rs/json-interface.html
