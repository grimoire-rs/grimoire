---
paths:
  - src/install/settings_*.rs
---

# Install Layout: Settings

Where a settings piece lands per client. Index: [subsystem-file-structure.md](./subsystem-file-structure.md).

### Settings {#install-layout-settings}

A **settings** package (experimental, behind `[options.experimental] settings`)
writes typed values into the harness's **own** config file and owns no file
but the Codex rules file. Each value is a *piece* spliced through the MCP engines —
`json_splice` (JSON/JSONC) or `toml_splice` (Codex, Warp), picked by
`Vendor::settings_config_format` — so every byte outside the piece survives.
`settings_writer::apply` is the one write path: it re-reads the file just
before the splice and re-judges every piece; a mismatch without `--force`
leaves the file untouched and exits 75 (`SettingsRaced`, reason `locked`,
retryable), and a file that no longer parses exits 65 with no reason, even
under `--force` (`SettingsBecameUnparseable`: a rerun refuses it too, so it
is data, not a race). Config files go through
`atomic_write_through_symlink`. The merge class (object, set, map, scalar, own
file) is the vendor's `settings_fields()` row; the ownership table lives in
`docs/src/content/docs/settings.md#ownership` — do not restate it here.

| Client | Project | Global |
|--------|---------|--------|
| **Claude** | `.claude/settings.local.json` (the file hooks splice; never the shared `settings.json`) | `<claude_root>/settings.json` |
| **Codex** | none | `<codex_root>/config.toml`, plus the own file `rules/grim-<binding>.rules` |
| **Copilot** | none | `<copilot_root>/settings.json` |
| **Cursor** | none | `cli-config.json`: `$CURSOR_CONFIG_DIR` when set, else `~/.cursor` when the file is there, else `$XDG_CONFIG_HOME`\|`~/.config`/`cursor` (Linux only); a `required_by_client` row makes grim edit only an existing file (`client-config-absent`) |
| **OpenCode** | none | the MCP file: `$OPENCODE_CONFIG`, else XDG `opencode.json`/`.jsonc` |
| **Qoder** | none | `<qoder_root>/settings.json` |
| **Amp** | none | `<amp_root>/settings.jsonc` when it exists, else `settings.json` (JSONC) |
| **Antigravity** | none | `~/.gemini/antigravity-cli/settings.json` (`antigravity-cli-root`, the CLI's own settings dir; not `antigravity-root`, the `~/.gemini/config` root the App and the CLI share for every other kind) |
| **Droid** | none | `~/.factory/settings.json` |
| **Kilo** | none | `<kilo_config_root>/kilo.jsonc`, else `kilo.json` when only it exists (JSONC) |
| **Kiro** | none | `<kiro_root>/settings/cli.json` (strict JSON, flat dotted keys: each pointer is one member) |
| **Gemini** | none | `<gemini_root>/settings.json`, the file its MCP entry edits |
| **Warp** | none | `settings.toml` (TOML 1.1): `~/.warp` on macOS, `$XDG_CONFIG_HOME`\|`~/.config`/`warp-terminal` on Linux, none elsewhere (`no-applicable-settings`) |
| **everyone else** | declined — empty `settings_fields()` | declined |

**The own-file class** is `codex.rules` alone: one closed Starlark renderer
writes `rules/grim-<binding>.rules`, compared by byte hash and written by the
plain rename (a link there is replaced, not followed). The `grim-` prefix can
never name Codex's `default.rules`; the binding grammar is the traversal
control. Config files are written before own files.

**Records are machine-local (I1).** A project's settings records live only in
`$GRIM_HOME/settings/<workspace-key>.json` (`settings_store`): envelope
`{v, workspace, records}`, `deny_unknown_fields`, dir `0700`, file `0600`,
`<workspace-key>` = `hook_dispatch::workspace_key` verbatim. `.grimoire/state.json`
never holds the kind — a `git pull` can deliver that file, so a settings record
read from it is discarded with a warning, and the save splits the kind out, so
a project without the kind keeps `state.json` byte-identical. An unusable store
file (parse failure, newer `v`, foreign `workspace`) reads as absent, and the
next save keeps its bytes once at `<key>.json.unusable`; no records → the file
is removed; under a `GRIM_HOME` the hook guard refuses, no store is read or
written.
Global scope keeps its records inline in `$GRIM_HOME/state/global.json`.

**Load rule, every kind.** An output its record's kind may not address is
dropped with a warning (`drop_illegal_outputs`). Three settings cases are
**withheld** instead of dropped: an OpenCode output that fails only on its file
name (`$OPENCODE_CONFIG` moved), an output of a client with no settings file on
this host (Warp on Windows, in a `global.json` carried from another machine),
and every settings output of a global state loaded under a refused `GRIM_HOME`. The value stays in the file, the record
keeps the output through the next save, and a run that resolves the file again
reads it. The match compares the path below the anchor, never the anchor, so a
Cursor or moved-root output stays visible on one host. Accepted residual
(follow-ups PD-46): a Warp output carried between macOS and Linux stays visible
at the other platform's root, a file that platform's Warp does not read.

**Global root fence (T3).** At global scope `root_refusal` declines a
settings file before it is read: a non-absolute path is
`settings-root-relative`, and a path inside the fenced work tree is
`settings-root-in-workspace`, carrying `Inside`: its own spelling (`Spelled`),
a `..` component (`Climbs`, refused whenever a tree is fenced: a `..` after a
missing component resolves no prefix), a link on its path (`Linked`), or an
unreadable cwd (`Unplaced`: `Bound::Closed` refuses every global root). The fenced
tree is the **outermost** ancestor of the cwd holding a `.git` dir or a
regular `.git` file with a `gitdir:` line that does not hold `$HOME`, so a
submodule fences its superproject; `holds_git` opens only a regular file (a
FIFO or device would block the open). A tree that holds `$HOME` is exempt,
since every default root lies in it, and then `shown_file` prints the
link-resolved path, never `~`. The vendor variables that place the file are
environment a repository can set, which is why the fence exists. Before the
survey, `name_refused_surfaces` warns once per (client, cause) on every run.
No reader opens a fenced file: `recorded_refusal` judges each recorded output
by its recorded path before any open, for `is_intact`, `judge`, `status`
(`footprint`, `derive_settings_aware`), the TUI and `derive_badge`, selected
client or not, and the row's `arming` names every client it refuses.
`kept_modified` drops a fenced output, and a row whose record holds a refused
client's output reads `skipped`. A refused file
is withheld, never dropped: the row reads `not-armed`, exit 0, the pin and
record hold, and `outputs` keeps the output. The remedy follows `Inside` (a
link: run grim from outside the repository); `root_variables` names what each
`settings_config_path` reads, pinned by a child-process probe test. Other
kinds do not take the fence. Recorded edges (dotfiles false positive, another
repository, a repository-set `HOME`, no `.git`): `arch-threat-model.md` T3.

**Consent and hygiene.** Project scope applies only past the flag and the
workspace consent record, which holds `settings:<binding>@<registry>/<repository>`
keys beside the hook keys (same file, same write seams). `.claude/settings.local.json`
goes into the repository's **common** `info/exclude`
(`ensure_settings_local_excluded`), dropped only when neither hooks nor
settings use the file — hygiene, not a control.

**Named update.** `grim update <names>` passes `Reach::Named`: it arms only the
closure's packages, removes a leftover only inside the previous closure, and its
`--force` reaches both (`force_scope`). A pruned package's refused value gets
one refusal naming `grim uninstall --force settings <name>` in place of a rerun
(`refuse_dropped`): the lock is saved by then, so no named rerun prunes it.
Consent, the conflict preflight and the removal guard still read the whole lock.
A piece only leaving records hold that an armed package outside the closure
declares gets no verdict (`defer_beside`); `apply` reads that package's layer
when the preflight did not (one armed package). When such a layer cannot be read
and a leftover would be removed, the pass stops before any write with the read's
error, 81 offline (`stop_for_unread_beside`). Each kept-value warning's install
carries `--trust-workspace` when only the flag armed the declarer (`install_applying`).
