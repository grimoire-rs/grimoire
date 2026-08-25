# Handover: a rule's support directory auto-loads as global rules in Claude Code

**Audience:** grim maintainers.
**Found in:** ocx (`ocx-sh/ocx`, 2026-08-22), consuming `ocx-sh/lore`'s
`rust-quality` and `rust-cargo` rules. Measured with `/context`, not inferred.
**Status:** confirmed defect, reproducible. No grim branch, no ADR. The
contract below is a proposal.

## TL;DR

`materialize_rule` (`src/install/client_target.rs:428`) copies a rule's
sibling support directory to `<rules_dir>/<name>/` **verbatim, for every
client** — `:471`, *"so the index's relative links resolve."*

For Claude that puts the depth files inside `.claude/rules/`, and Claude Code
discovers rules there **recursively**, treating any file without `paths:`
frontmatter as an unconditional global rule. So the support tree — material
the index is supposed to *route to* — loads into every session instead.

Measured on ocx: **164.5k tokens of always-on context, 145k of it (88%) the
two support trees.** 19 files, ~343 KB. `rules/rust-quality/` alone is 18
files and 329 KB. The two index rules that own them are correctly scoped to
`**/*.rs` and were, correctly, not loaded at all.

Ask: when a rule with a support directory installs to Claude, register that
directory in `claudeMdExcludes`. Unregister on uninstall.

## Why this is grim's bug and not lore's

Lore is authored correctly on both sides:

- `rules/rust-quality.md` carries `paths: ["**/*.rs"]` and a
  "Where the Depth Is" routing table.
- The depth files under `rules/rust-quality/` carry no frontmatter, which is
  right — **no path glob can express "only when the index sends you."** That
  is precisely what a support directory means, and `publish.toml` says so:
  *"`rust-quality` is the index rule; its support directory carries the depth
  files it routes to."*

There is no authoring change that fixes this. Adding `paths: ["**/*.rs"]` to
each depth file would move the 145k from *every* session to *every Rust
session* — the same cost, differently scheduled. Omitting `paths` is what
already happens. Nothing in between exists.

Nor can lore fix it downstream: the only surface that suppresses the load is
the consumer's own settings file, and grim is the thing that writes into a
consumer's tree.

## The Claude Code semantics, verbatim

From `code.claude.com/docs/en/memory`:

- *"All `.md` files are discovered recursively, so you can organize rules
  into subdirectories like `frontend/` or `backend/`."*
- *"Rules without `paths` frontmatter are loaded at launch with the same
  priority as `.claude/CLAUDE.md`."*
- `claudeMdExcludes`: *"lets you skip specific files by path or glob
  pattern… Patterns are matched against absolute file paths using glob
  syntax. You can configure `claudeMdExcludes` at any settings layer: user,
  project, local, or managed policy. Arrays merge across layers."*

The docs' own example excludes a rules directory:
`"/home/user/monorepo/other-team/.claude/rules/**"`.

Exclusion suppresses **auto-load only**. The files stay on disk and stay
readable with the Read tool, so the index's routing keeps working and its
relative links keep resolving. That is the whole point: the depth arrives
when the index sends the agent for it, which is the behaviour the artifact
was written for.

## Proposed contract

A structural clone of the OpenCode precedent, pointed the other way.
`src/install/opencode_config.rs` already implements *"the reversible
config-registration pattern from the hooks ADR"* — a managed entry added when
the first rule installs, removed when the last uninstalls — because OpenCode
has no per-file scoping and needs an entry to make rules load. Claude has
per-file scoping but over-applies it to support trees, and needs an entry to
make part of them **not** load. Same seam, opposite sign.

### New module: `src/install/claude_config.rs`

Mirrors `opencode_config.rs`. Manages one array key.

### Trigger

Per rule, not per rules-directory — only rules that ship a support directory
are excluded; index files must keep loading. One managed element each:

```
**/.claude/rules/<name>/**
```

The `**/` prefix keeps a project-scope entry valid across git worktrees of
the same repository, which is how the reporting project is laid out (four
fixed worktrees). Use an absolute glob at global scope, matching how
`opencode_config` roots its global glob at `$GRIM_HOME`.

### Config path resolution

- **project** → `<workspace>/.claude/settings.json`
- **global** → `~/.claude/settings.json`, honoring `CLAUDE_CONFIG_DIR` —
  `vendor_claude.rs:238` already resolves this root via
  `user_config_dir(env_dir("CLAUDE_CONFIG_DIR"), home_dir())`.

Committed `settings.json`, not `settings.local.json`: the exclusion is a
property of the artifact, identical for every consumer, not a machine
preference. Matches what `opencode_config` does at project scope.

### Machinery — all of it already exists

| Need | Existing |
|---|---|
| Trait hook, defaults to no-op | `Vendor::sync_config` — `src/install/vendor.rs:377` |
| Call-site precedent | `vendor_opencode.rs:302` → `opencode_config::sync_for_state` |
| Add one array element, span-preserving | `json_splice::upsert_array_element` — `json_splice.rs:184` |
| Remove it on uninstall | `json_splice::remove_array_element` — `json_splice.rs:230` |
| Parse / never-clobber-on-unparseable | `json_config::parse_object`, `sanitize_jsonc` |

`claudeMdExcludes` is a top-level array of strings, so `upsert_array_element`
/ `remove_array_element` apply directly — no new splice primitive.
`ClaudeVendor` overrides `sync_config`; the default no-op covers every other
vendor unchanged.

Keep the existing edit discipline: a `settings.json` that does not parse is
never rewritten, the sync fails instead. Claude's settings file tolerates
`//`-prefixed string entries inside arrays as pseudo-comments in the wild
(the reporting project's `permissions.deny` uses them) — they are ordinary
JSON strings, so the splice engine is unaffected, but do not "tidy" them.

### Determinism and drift

The managed element is derived from the artifact name alone, so regenerating
at the same pinned identifier yields the same entry — consistent with the
byte-identical-output rule in `client_target.rs`'s module doc. Uninstall and
`prune` must remove it; check `uninstall.rs`, `prune.rs`,
`expected_outputs.rs` and `install_state.rs` for the same coverage
`opencode_config` gets, and `path_anchor.rs` for the global-scope path test.

## Rejected alternative

**Relocate the support directory out of `rules/` for Claude** — e.g.
`.claude/lore/<name>/`. Correct-looking, and self-evident from the tree with
no hidden settings. But it breaks the "copied verbatim, no transform, all
clients" invariant at `client_target.rs:471` and forces relative-link
rewriting inside the index render for one vendor. Much larger diff, same
outcome. Revisit only if `claudeMdExcludes` proves unreliable.

## Related

- [grimoire-rs/grimoire#100](https://github.com/grimoire-rs/grimoire/issues/100)
  "Rules have no effect in OpenCode" — same family: per-client rule-loading
  semantics grim does not model. Worth fixing with one shared mental model
  rather than twice.
- No open issue covers the Claude side; 60 open checked on 2026-08-22.

## Interim workaround in the consuming project

ocx hand-wrote the exclusion into `.claude/settings.json`:

```json
"claudeMdExcludes": [
  "**/.claude/rules/rust-quality/**",
  "**/.claude/rules/rust-cargo/**"
]
```

Same glob spelling proposed above, so grim's upsert will find the element
present and leave it alone rather than adding a duplicate. If grim picks a
different spelling, that project gets two equivalent entries — harmless,
but a reason to keep the spelling.

## Out of scope

- Whether other clients over-load support trees the same way. OpenCode,
  Copilot, Cursor and Kiro each have their own rule-discovery rules; this
  handover only establishes the Claude case. Worth an audit pass, not a
  blocker.
- Any change to lore. It is correct as authored.
- Any grim frontmatter field to mark a support directory as
  "non-auto-loading". Unnecessary — the presence of a support directory is
  already the signal, and it is *always* true that it should not auto-load.
