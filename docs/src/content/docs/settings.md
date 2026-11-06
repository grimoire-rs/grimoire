---
title: Settings reference
description: The settings document format, how grim merges each value into a harness's own file, the refusal error, and the fields each harness takes. Experimental pre-1.0.
---
<!-- doc_type: reference -->

**Experimental pre-1.0**. See [Stability][stability-unstable].

The `settings` kind ships in grim 0.16.0 behind the
`[options.experimental] settings` flag. Its format, merge rules and report
fields may change in any minor release.

A settings package carries typed values for a harness's own configuration,
such as a permission rule or a default model. Each value lands in the file the
harness reads, and grim reads that file first. A value grim did not write is
never replaced without `--force`.

This page covers the document format, the merge rules, the refusal error, the
fields each harness takes, and their limits.

## Turn the kind on {#enable}

The kind is off by default. Turn it on for one project, or add `--global` for
global scope:

<!-- doc-norun: changes the reader's own grim configuration -->
```bash
grim config set options.experimental.settings true
```
<!-- /doc-norun -->

Declaring and locking a settings package need no flag. With the flag off,
`grim status` reads `gated` with cause `feature-flag-off`, and `grim install`
writes and removes no settings. `grim install`, `grim update` and `grim add`
without `--no-install` then warn once for each locked settings package. The
warning names the command that sets the flag for the scope that is off.
Clearing the flag leaves every written value in place.

No environment variable turns the kind on, and the flag is not consent. The
key is listed in the [configuration reference][config-experimental].

## Consent {#consent}

At project scope a settings package applies only after the workspace
consents. A repository's `grimoire.toml` is written by whoever committed it,
so a declaration alone never changes a harness's settings. Global scope is
always consented and writes no record.

Consent shares the workspace record that hooks use, under keys of the form
`settings:<binding>@<registry>/<repository>`. `grim hook allow` records
consent for the hooks and settings the workspace declares. `grim hook revoke`
removes it and leaves every written value in place.

`--trust-workspace` and `--no-trust-workspace` answer for one run of `add`,
`install` or `update`, and write no record. Their aliases are `--trust-hooks`
and `--no-trust-hooks`. Neither flag turns the kind on. [Workspace
consent][hooks-consent] and [Per-run answers][hooks-trust-flags] describe the
record and the flags under their hook names.

A package that consent withholds writes nothing, and the run exits `0`. Its
row in the `install`, `update` or `add` report reads `skipped`, and grim warns
`settings package '<name>' not applied: <reason>` once for it. The reason
depends on the answer:

| Answer | Reason |
|---|---|
| No record, no terminal | ``this workspace has not been consented for settings and there is no terminal to ask on; run `grim consent allow` or pass --trust-workspace`` |
| Prompt answered no | ``settings consent for this workspace was declined; run `grim consent allow` or pass --trust-workspace to apply them`` |
| A record that no longer covers the package | ``this workspace declares settings it was not consented for; re-run `grim consent allow` to review them`` |
| `--no-trust-workspace` | `--no-trust-workspace (alias --no-trust-hooks) was passed, so nothing applies on this invocation` |

A version bump of a consented package asks nothing again, and a new version
can change what an agent may do. The lock diff and the change list each run
reports are the evidence of that change.

## Document format {#format}

A settings document is one TOML file. Descriptive keys sit at the top level,
and each harness gets a table named after its grim client name. Save this
document as `settings/team-defaults.toml`:

<!-- doc: settings-reference -->
```toml
description = "Team defaults for Claude Code and Codex"

[claude]
model = "sonnet"

[claude.permissions]
deny = ["Bash(rm -rf:*)"]

[codex]
approval_policy = "on-request"

[[codex.rules]]
pattern = ["rm"]
decision = "forbidden"
justification = "Deleting files needs a person."
```

Validate and pack it without pushing. `grim build` needs `--kind settings` for
a settings document, and exits `78` without it.

<!-- doc: settings-reference -->
```bash-run
grim build settings/team-defaults.toml --kind settings
```

The report names the kind, the name, the path, the layer digest and the
status `built`.

### Top-level keys {#format-keys}

`description` is required. `summary`, `keywords`, `license`, `repository`,
`deprecated`, `replaced-by`, `authors`, `vendor`, `homepage` and
`documentation` are optional. A `repository` must be an `https` URL.
`requires` declares [dependencies][artifacts-requires] and is kept out of the
settings layer.

Every other top-level table is a harness table: `claude`, `codex`, `copilot`,
`cursor`, `opencode`, `qoder`, `antigravity`, `amp`, `droid`, `gemini`, `kilo`,
`kiro` or `warp`. A table for any other name is refused at
build time with exit `65`. A consumer skips such a table with a warning.
For a name this grim does not know, the warning reads
`settings package '<name>' carries a table for '<table>', a client this grim does not know; it is ignored, and a newer grim may apply it`.
For a harness this grim knows that takes no settings, such as `zed`, it reads
`settings package '<name>' carries a table for '<table>', which takes no settings from this grim; it is ignored, and a newer grim may apply it`.

### The layer {#format-layer}

grim publishes the document as one JSON layer of media type
`application/vnd.grimoire.settings.v1+json`. The layer is pretty-printed with
sorted keys and holds at most 64 KiB. `grim schema --kind settings` prints the
document's JSON Schema.

### Values {#format-values}

Each value is a piece with a pointer, its path inside the harness table.
`deny` under `[claude.permissions]` has the pointer `/permissions/deny`. A
value must match the type its field takes in the [field table][fields].

A pointer segment that holds a dot is one quoted key. [Kiro][kiro]'s
`/chat.disableWrap` is written as below. A bare `chat.disableWrap` is a
`chat` table to TOML, so grim refuses it.

<!-- doc-norun: a fragment of a settings document, built only as a whole -->
```toml
[kiro]
"chat.disableWrap" = true
```
<!-- /doc-norun -->

A map entry is a table of its own. A [Claude Code][claude] marketplace entry
names its source:

<!-- doc-norun: a fragment of a settings document, built only as a whole -->
```toml
[claude.extraKnownMarketplaces.acme]
source = { source = "github", repo = "acme/plugins" }
```
<!-- /doc-norun -->

A Codex rule entry takes `pattern`, a non-empty list of strings, and
`decision`, one of `allow`, `prompt` or `forbidden`. `justification` is an
optional string.

### Validation errors {#validation}

Messages are lowercase and never name the value. At build time each one reads
`<path>: invalid tool metadata: <message>` and exits `65`.

| Message | Cause |
|---|---|
| `{vendor} has no setting at {pointer}` | The harness has no field at that pointer. At install time this exits `65` with reason `unsupported-artifact-feature`. Both carry the [unknown-key `hint`][json-hint]. A bare [Kiro][kiro] dotted key adds `; kiro keys are dotted names, so quote the whole key in TOML, as in "chat.defaultModel" = …`, naming the key it matched. |
| `unknown vendor table {vendor}` | A top-level table names no harness grim knows. It carries the [unknown-key `hint`][json-hint]. |
| `{vendor} takes no settings from grim, so a package cannot carry a {vendor} table` | A top-level table names a harness grim knows that takes no settings, such as `zed`. |
| `... must be {expected}` | The value has the wrong type. |
| `... must be one of [...]` | The value is not in the field's list. |
| `setting {pointer} is null` | The value is null. |
| `setting {pointer} is a datetime` | TOML datetimes are refused. |
| `setting {pointer} is a float` | Floats are refused. |
| `setting {pointer} is out of range` | The integer does not fit. |
| `setting {pointer} holds an expansion token, which the harness replaces with an environment variable or a file when it reads the file; write the value literally` | An OpenCode or Kilo key or value holds `{env:` or `{file:`, or a Gemini value holds `$NAME` or `${NAME}`: a `$` before a letter, digit, `_` or `{`. Each harness expands these when it reads the file. |
| `settings {first} and {second} are exclusive` | Codex `default_permissions` sits beside `sandbox_mode` or `sandbox_workspace_write`. |
| `... holds a credential in a URL` | A URL carries a user name or password. |
| `rule {index} holds a control character` | A Codex rule string holds a control character. |
| `settings layer is {actual} bytes, the limit is {limit}` | The layer is over 64 KiB. |
| `malformed settings document: {detail}` | The document does not parse, or an OpenCode pattern table is not in sorted byte order. |

## Merge rules {#merge}

grim reads the harness's file, judges every piece, and writes only the pieces
that change. A key that no package and no record names is never compared and
never changed. Each field has one merge class:

| Class | One piece is | How it merges |
|---|---|---|
| Object | a table of fields | grim recurses into it. It creates a missing table and removes one it emptied, unless the harness requires it. |
| Set | one element of a list of strings | grim adds its elements and removes only the ones it wrote. Other elements stay. |
| Map | one entry of a table, by key | Each entry is compared whole. |
| Scalar | one value | Replaced whole. Ordered lists and fixed-shape tables are scalars too. |
| Own file | a whole file | grim writes or deletes the file whole and compares its bytes by hash. Only the [Codex rules file][codex-rules-file] uses this class. |

Config files are written before own files.

### Ownership {#ownership}

The file decides, and a record never authorises an overwrite. grim records the
hash of each value it writes. On the next run it compares the file with what
the lock wants and with that record. The first matching row applies:

| The file | The lock | What grim does | Listed as |
|---|---|---|---|
| cannot be parsed | wants a value there | Refuses the run, exit `65`. `--force` does not override it. | nothing |
| cannot be parsed | dropped the value | Drops its record and leaves the value. | a warning, and `abandoned_entries` on `update` |
| lacks the value | dropped it | Drops its record. | nothing |
| holds a value grim adopted | dropped it | Leaves the value in place. | a warning, and `abandoned_entries` on `update` |
| holds the recorded value | wants another | Writes the new value. | `changed` |
| holds the recorded value | dropped it | Removes the value. | `removed` |
| holds the wanted value | wants it | Writes nothing. With no record, grim adopts the value. | nothing |
| lacks the value | wants it | Writes it. | `added` |
| holds a value of another type in the way | wants a value there | Refuses. `--force` replaces it. | `overwritten` |
| holds a value of another type in the way | dropped the value | Refuses. `--force` drops the record and leaves it. | nothing |
| holds a value grim did not write | wants another | Refuses. `--force` replaces it. | `overwritten` |
| holds a value grim did not write | dropped it | Refuses. `--force` removes it. | `removed` |

A refusal covers every settings value in the run. grim judges every piece
before it writes any, so a refused run writes no settings. A run that fails
part way through converges on the next run.

An adopted value stays the user's. grim never removes it when the lock drops
it, and warns `<path> <pointer>: left in place, grim no longer tracks it`.
`grim update` also names the value in its report's `abandoned_entries`.
`grim install` has no such list, so the warning is its only notice.

### Uninstall {#uninstall}

`grim uninstall` applies the same rows to the package it removes, and its
`--force` covers settings pieces too. It never removes an adopted value that
still matches its record. That value stays even under `--force`, and is named
in `abandoned_entries`. An adopted value that changed refuses, and `--force`
removes it.

## Refusals and `--force` {#force}

A refused run exits `65` with the message
`settings {packages}: {n} settings value(s) were set or changed outside grim; rerun with --force to change them`.
`{packages}` names every package that holds a refused piece, such as
`'base', 'team'`. The [error document][json-error] sets `forceable` to
`true`. Its `reason` is `modified` when any refused piece changed after grim
wrote it, and `untracked-destination` otherwise. A `pieces` array names every
refused piece:

<!-- doc-norun: an error document grim prints, not a command -->
```json
{
  "error": {
    "code": "data",
    "exit": 65,
    "message": "settings 'team-defaults': 1 settings value(s) were set or changed outside grim; rerun with --force to change them",
    "reason": "modified",
    "forceable": true,
    "pieces": [
      {
        "package": "team-defaults",
        "client": "claude",
        "file": ".claude/settings.local.json",
        "pointer": "/model",
        "element": null,
        "marked": "overwrite",
        "state": "modified",
        "wanted": "sonnet",
        "current": "opus"
      }
    ]
  }
}
```
<!-- /doc-norun -->

| Field | Meaning |
|---|---|
| `package` | The package binding that declared the piece. |
| `client` | grim's name for the harness. |
| `file` | The settings file: workspace-relative at project scope, `~/` under the home directory, absolute otherwise. `null` on a conflict. When a repository around the command holds the home directory, a global file is shown absolute with its links resolved, as [Config roots inside a repository][root-fence] explains. |
| `pointer` | The value's pointer, or the pointer of a value of another type in the way. |
| `element` | The list element of a set piece, else `null`. |
| `marked` | `overwrite`, or `conflict` for a [conflict][conflicts]. |
| `state` | `modified` when the value changed after grim wrote it, `untracked-destination` when grim never wrote one. `null` on a conflict. |
| `wanted` | The value the package wants. |
| `current` | The value the file holds, or `null`. |

grim screens `current` before it shows it. A credential in a URL is redacted,
control characters are escaped, and the text is cut at 200 characters after
escaping. Plain output prints one warning per refused piece, such as
`overwrite claude .claude/settings.local.json /model (team-defaults): modified`.

`grim install --force` writes every refused piece. The overwrite cannot be
undone: grim keeps no backup of a value it replaces. `current` is the only copy
grim shows, and it stops at 200 characters. Save a value you want to keep
before you rerun with `--force`.

### Conflicts {#conflicts}

Two packages conflict when they want different values for one pointer, or when
one pointer lies inside the other. A set element and an own file never
conflict on value. The Codex exclusion between `default_permissions` and
`sandbox_mode` holds across packages too.

A conflict exits `78` with
`settings conflict across packages: {n} value(s) cannot all apply; change or remove one of {packages}, since no --force overrides a conflict`.
Its rows in `pieces` carry `marked` set to `conflict`, a `null` `file` and
`state`, and the `wanted` value. Plain output prints one warning per row, such
as `conflict claude /model (base)`.

With the flag on and settings in the lock, `grim add` checks for a conflict
before it saves the lock. In the same case `grim lock` exits `78` on a
conflict among the packages it would apply. A damaged state file then makes
`grim lock` exit `74`.

### Unparseable files {#unparseable}

A settings file grim cannot parse stops any run that would write into it, with
exit `65` and the message
`{client}: settings file '{path}' cannot be parsed, and grim never replaces it, so repair its syntax and rerun`.
`--force` does not override it.

grim reads each file again just before it writes. When a value it judged has
changed since the plan and `--force` is not given, grim leaves the file
untouched and the run exits `75` with
`settings file '{path}' changed during the run, so grim left it untouched; rerun the command`.
The [error document][json-error] carries reason `locked` and `retryable`
`true`. A file that changed and no longer parses exits `65`, under `--force`
too, with
`settings file '{path}' changed during the run and no longer parses; grim never replaces such a file, so repair it and rerun`,
and carries no `reason`.

## The change list {#changes}

Every run that changes a value reports the change, never the value. Each
`settings_changes` item holds `package`, `client`, `file`, `key`, `element`
and `action`. `action` is `added`, `changed`, `removed` or `overwritten`.

| Command | Where the list sits |
|---|---|
| `grim install` | `settings_changes` beside `items` |
| `grim update` | `settings_changes` beside `items` and `pieces` |
| `grim add` | `settings_changes` on the added row |
| `grim uninstall` | `settings_changes` beside `abandoned_entries` |

Plain output prints one warning per change, such as
`added claude .claude/settings.local.json /model (team-defaults)`. After any
change grim also warns
`a running client may need a restart to read its new settings`.

An `abandoned_entries` item holds `path`, `pointer` and `element`. On
`grim install`, `grim add` and the install pass of `grim update`, plain output
also warns `<path> <pointer>: left in place, grim no longer tracks it`.
`grim uninstall` and the client reap of `grim update` print no such line. They
name the value in `abandoned_entries` only.

## Where each value lands {#files}

| Harness | Project | Global |
|---|---|---|
| [Claude Code][claude] | `.claude/settings.local.json` | `settings.json` in `$CLAUDE_CONFIG_DIR`, else `~/.claude` |
| [Codex][codex] | none | `config.toml` and `rules/grim-<binding>.rules` in `$CODEX_HOME`, else `~/.codex` |
| [Copilot][copilot] | none | `settings.json` in `$COPILOT_HOME`, else `~/.copilot` |
| [Cursor][cursor] | none | `cli-config.json` in `$CURSOR_CONFIG_DIR`, else in `~/.cursor` when it exists there, else on Linux in `cursor` in the XDG config directory |
| [OpenCode][opencode] | none | `$OPENCODE_CONFIG`, else `opencode/opencode.jsonc` in the XDG config directory when it exists, else `opencode/opencode.json` there |
| [Qoder][qoder] | none | `settings.json` in `$QODER_CONFIG_DIR`, else `~/.qoder` |
| [Amp][amp] | none | `amp/settings.jsonc` in the XDG config directory when it exists, else `amp/settings.json` there |
| [Antigravity][antigravity] | none | `settings.json` in `~/.gemini/antigravity-cli`, the CLI's own directory |
| [Droid][droid] | none | `settings.json` in `~/.factory` |
| [Kilo][kilo] | none | `kilo/kilo.jsonc` in the XDG config directory, else `kilo/kilo.json` there when only it exists |
| [Kiro][kiro] | none | `settings/cli.json` in `$KIRO_HOME`, else `~/.kiro` |
| [Gemini][gemini] | none | `settings.json` in `$GEMINI_CLI_HOME/.gemini`, else `~/.gemini`, the file grim's MCP entry edits |
| [Warp][warp] | none | `settings.toml` in `~/.warp` on macOS, and in `warp-terminal` in the XDG config directory on Linux; none on Windows |

Only [Claude Code][claude] takes settings at project scope. Every other
harness gives a package the decline `no-applicable-settings` in a project.

[Warp][warp] gives the same decline on Windows and on every platform but
macOS and Linux, since grim has no root for its file there. Only the Stable
build's file is written. A Preview build reads its own directory, as the
[file locations page][warp-files] lists.

### Declines {#declines}

When a harness cannot take a package here, grim writes nothing for it and
warns `settings package '<name>': <client> <decline> (<cause>); <remedy>`,
such as
`settings package 'team': cursor writes only into an existing /home/me/.cursor/cli-config.json (client-config-absent); start cursor once so it creates that file, then run grim install --global`.
A global file is named by its absolute path.

A file grim refuses to reach is named differently. For the causes
`not-registered`, `settings-root-relative` and `settings-root-in-workspace`,
grim warns once per harness and cause on every run, as
`settings left unchanged: <client> <decline> (<cause>); <remedy>`.
The row's `arming` array in `grim status` names the same cause, and its
`message` carries the decline and the remedy.

| Cause | Meaning | Fix |
|---|---|---|
| `no-applicable-settings` | Nothing in the package applies to this harness at this scope. | Declare the package at global scope with `grim --global add`, or select a harness that takes settings with `--client` or `[options].clients`. |
| `client-config-absent` | The harness writes only into a file that exists, and it does not. | Start the harness once so it creates the file, then run `grim install --global`. |
| `not-registered` | A link on the file's path dangles or leads outside the workspace, or grim could not read its settings records. | Replace the link with a regular file, or make the records file readable. |
| `settings-root-relative` | At global scope, the variable that places the file names a relative path. | Set the variable to an absolute path, or unset it, then run `grim install --global`. |
| `settings-root-in-workspace` | At global scope, the file lies inside the repository the command runs in, by its own path or through a link, or grim cannot place it outside. | The step depends on what put the file inside. [Config roots inside a repository][root-fence] lists each case. |
| `local-file-tracked` | [git][git] tracks `.claude/settings.local.json`, so values grim wrote there can reach a commit. | Run `git rm --cached .claude/settings.local.json`, then `grim install`. |

`local-file-tracked` is a note on one harness and never moves the row's
`state`. `client-config-absent` covers [Cursor][cursor], whose
`cli-config.json` grim never creates. It also covers any field the harness
requires. grim never removes a required key: a list keeps `[]`, and a single
value is left in place.

At project scope grim refuses to write through a link on a settings file's
path when the link dangles or leads outside the workspace. The
harness is declined there with cause `not-registered`, and the row reads
`not-armed`. A settings records file grim cannot read gives the same cause,
with the message
`settings left unchanged: their records at <path> could not be read; make that path a file your user can read, then re-run the command`.

### Config roots inside a repository {#root-fence}

At global scope the variables in the [Files][files] table place each
harness's file. A repository can set the environment of whoever runs grim in
it, through files such as `.envrc` or `.mise.toml`. So grim refuses a global
settings file whose config directory is a relative path, with cause
`settings-root-relative`. It also refuses a file inside the repository the
command runs in, with cause `settings-root-in-workspace`:

<!-- doc-norun: a warning grim prints, not a command -->
```text
settings left unchanged: kiro cannot take kiro-rel/settings/cli.json, whose config directory is a relative path (settings-root-relative); set KIRO_HOME to an absolute path, or unset it, then run grim install --global
```
<!-- /doc-norun -->

The repository is the outermost directory, from the working directory up, that
holds a `.git` directory or a regular `.git` file naming one. A repository
that holds the home directory does not count, since every default root lies in
it. So a nested clone or a submodule is fenced together with its
superproject. A `.git` that is neither a directory nor a regular file marks
nothing.

A file counts as inside in four cases, and the warning names the step for its
case:

| Case | Decline | Step |
|---|---|---|
| The file's own path lies in the repository. | `cannot take <file>, which lies inside the repository this command runs in` | Set the variable to a path outside the repository, or unset it, then run `grim install --global`. Or run that command from outside the repository. |
| A link on the file's path leads into the repository. | `cannot take <file>, which links into the repository this command runs in` | Run `grim install --global` from outside the repository. |
| The path holds a `..` component. | ``cannot take <file>, whose `..` component keeps grim from placing it outside the repository this command runs in`` | Set the variable to a path without `..`, or unset it, then run `grim install --global`. Or run that command from outside the repository. |
| The working directory cannot be read. | `cannot take <file>: the directory this command runs in cannot be read, so grim cannot tell whether that file lies inside its repository` | Run `grim install --global` from a directory you can read. |

The `..` case applies only while the command runs inside a repository that
counts. An unreadable working directory refuses every global file, since grim
cannot tell where the repository lies.

A refused harness writes nothing, its `grim status` row reads `not-armed`, and
the run exits `0`. No command reads a file the fence refuses, whether the
harness is selected or only a record still names it. A value grim wrote there
before stays in place, its record and pin are kept, and `grim status` still
lists it in `outputs`. The `grim install` row of a package whose record holds
such a value reads `skipped`.

Every run names each refused harness once per cause, whether or not the
package declares a value for it. A run with nothing to write names it too. The
warning starts `settings left unchanged:`. The check covers the `settings`
kind only.

A config root that is, or links into, a dotfiles repository is refused while
grim runs inside that repository. Run the command from another directory, or
unset the variable.

When a repository around the command holds the home directory, grim never
shows a global file as `~/`. Change lines and `pieces` then name it by its
absolute path, with links resolved, so a write into that repository shows
where it lands.

Two known edges fall outside the fence. grim reads the home directory from
`HOME` (`USERPROFILE` on Windows). A repository can set that variable like the
others, and the home exemption follows it. A checkout with no `.git`, such as
an unpacked archive or a repository of another version control system, is no
repository to the fence. In both cases grim follows the variables as they are
set.

In a project, the [Claude Code][claude] field `/permissions/defaultMode`
takes only `default`, `acceptEdits` and `plan`, since the harness ignores the
other modes there. Any other value is skipped with the warning
`settings package '<package>': claude /permissions/defaultMode does not take this value in a project; skipped`.

### The project local file {#local-file}

At project scope grim writes [Claude Code][claude] values to
`.claude/settings.local.json`, the per-developer file. It never writes the
shared `.claude/settings.json`.

grim appends `.claude/settings.local.json` to `info/exclude` in the
repository's common [git][git] directory, so linked worktrees share the rule.
The rule only keeps `git status` clean. grim drops it once neither settings
nor hooks use the file. [Claude Code][claude] also adds
`**/.claude/settings.local.json` to the global [git][git] excludes on its first
write, as its [settings docs][claude-settings] say.

## The Codex rules file {#codex-rules}

A `[[codex.rules]]` entry is not a `config.toml` key. grim renders the entries
to a Starlark file it owns, `rules/grim-<binding>.rules` under the
[Codex][codex] root. In reports its key is `/rules`. Each entry becomes one
`prefix_rule` call:

<!-- doc-norun: Starlark that grim writes for Codex, not a command -->
```python
prefix_rule(
    pattern = ["rm"],
    decision = "forbidden",
    justification = "Deleting files needs a person.",
)
```
<!-- /doc-norun -->

- grim touches only files with the `grim-` prefix, and never `default.rules`.
- `rules = []` places nothing.
- [Codex][codex] reads rules at startup, so restart it after a change, as the
  [Codex rules docs][codex-rules] say.
- A malformed `.rules` file beside grim's also stops grim's rules in a check.
  With codex-cli 0.166.0, `codex execpolicy check` over both files exits
  non-zero with `failed to parse policy` and prints no decision. How a
  [Codex][codex] session treats a bad file is not verified.
- The `rules/` directory stays after grim removes its last file.

`--force` at a rules file path replaces only an empty directory or a link to a
directory. A non-empty directory there, or a file at the `rules` path itself,
is a forceable refusal with exit `65`. Under `--force` the run stops before
any write with exit `74`, naming the path and
`--force replaces only an empty directory`.

## Per-harness fields {#fields}

Each row is one field grim takes. `both` means project and global scope.

| Harness | Pointer | Value | Merge | Scope |
|---|---|---|---|---|
| claude | `/model` | string | scalar | both |
| claude | `/effortLevel` | `low`, `medium`, `high`, `xhigh` | scalar | both |
| claude | `/outputStyle` | string | scalar | both |
| claude | `/language` | string | scalar | both |
| claude | `/spinnerTipsEnabled` | boolean | scalar | both |
| claude | `/alwaysThinkingEnabled` | boolean | scalar | both |
| claude | `/includeGitInstructions` | boolean | scalar | both |
| claude | `/permissions/allow` | list of strings | set | both |
| claude | `/permissions/deny` | list of strings | set | both |
| claude | `/permissions/ask` | list of strings | set | both |
| claude | `/permissions/defaultMode` | `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`, `manual`. A project takes the first three. | scalar | both |
| claude | `/sandbox/enabled` | boolean | scalar | both |
| claude | `/extraKnownMarketplaces/<name>` | table with `source` | map | both |
| claude | `/theme` | `auto`, `dark`, `light`, `dark-daltonized`, `light-daltonized`, `dark-ansi`, `light-ansi` | scalar | both |
| claude | `/editorMode` | `normal`, `vim` | scalar | both |
| claude | `/tui` | `default`, `fullscreen` | scalar | both |
| claude | `/attribution/commit` | string | scalar | both |
| claude | `/attribution/pr` | string | scalar | both |
| claude | `/attribution/sessionUrl` | boolean | scalar | both |
| claude | `/sandbox/filesystem/allowWrite` | list of strings | set | both |
| claude | `/sandbox/filesystem/denyWrite` | list of strings | set | both |
| claude | `/sandbox/filesystem/denyRead` | list of strings | set | both |
| claude | `/sandbox/filesystem/allowRead` | list of strings | set | both |
| codex | `/model` | string | scalar | global |
| codex | `/model_reasoning_effort` | string | scalar | global |
| codex | `/approval_policy` | `on-request`, `never` | scalar | global |
| codex | `/sandbox_mode` | `read-only`, `workspace-write`, `danger-full-access` | scalar | global |
| codex | `/sandbox_workspace_write/network_access` | boolean | scalar | global |
| codex | `/sandbox_workspace_write/writable_roots` | list of strings | set | global |
| codex | `/sandbox_workspace_write/exclude_slash_tmp` | boolean | scalar | global |
| codex | `/sandbox_workspace_write/exclude_tmpdir_env_var` | boolean | scalar | global |
| codex | `/default_permissions` | string | scalar | global |
| codex | `/permissions/<name>` | table with `description`, `extends`, `filesystem`, `network`, `workspace_roots` | map | global |
| codex | `/tui/alternate_screen` | `auto`, `always`, `never` | scalar | global |
| codex | `/tui/animations` | boolean | scalar | global |
| codex | `/tui/auto_recap` | boolean | scalar | global |
| codex | `/tui/raw_output_mode` | boolean | scalar | global |
| codex | `/history/persistence` | `save-all`, `none` | scalar | global |
| codex | `/hide_agent_reasoning` | boolean | scalar | global |
| codex | `/marketplaces/<name>` | table with `source`, `source_type`, `ref`, `sparse_paths` | map | global |
| codex | `rules` | list of tables with `pattern`, `decision`, `justification` | own file | global |
| codex | `/file_opener` | `vscode`, `vscode-insiders`, `windsurf`, `cursor`, `none` | scalar | global |
| codex | `/tui/notification_method` | `auto`, `osc9`, `bel` | scalar | global |
| codex | `/tui/notification_condition` | `unfocused`, `always` | scalar | global |
| codex | `/web_search` | `disabled`, `cached`, `indexed`, `live` | scalar | global |
| copilot | `/model` | string | scalar | global |
| copilot | `/effortLevel` | `low`, `medium`, `high`, `xhigh` | scalar | global |
| copilot | `/theme` | `default`, `github`, `dim`, `high-contrast`, `colorblind` | scalar | global |
| copilot | `/defaultMode` | `interactive`, `plan`, `autopilot` | scalar | global |
| copilot | `/defaultPermissionMode` | `manual`, `assisted`, `allow-all` | scalar | global |
| copilot | `/permissions/disableBypassPermissionsMode` | `disable` | scalar | global |
| copilot | `/allowedUrls` | list of strings | set | global |
| copilot | `/deniedUrls` | list of strings | set | global |
| copilot | `/disabledMcpServers` | list of strings | set | global |
| copilot | `/disabledSkills` | list of strings | set | global |
| copilot | `/askUser` | boolean | scalar | global |
| copilot | `/includeCoAuthoredBy` | boolean | scalar | global |
| copilot | `/respectGitignore` | boolean | scalar | global |
| copilot | `/renderMarkdown` | boolean | scalar | global |
| copilot | `/stream` | boolean | scalar | global |
| copilot | `/transcriptView` | `default`, `concise` | scalar | global |
| copilot | `/mergeStrategy` | `rebase`, `merge` | scalar | global |
| copilot | `/sandbox/enabled` | boolean | scalar | global |
| copilot | `/extraKnownMarketplaces/<name>` | table with `source`, `autoUpdate` | map | global |
| copilot | `/editorMode` | `normal`, `vim` | scalar | global |
| copilot | `/autoTier` | `efficiency`, `balance`, `intelligence`. A tier the Copilot server offers stays valid in your file. | scalar | global |
| copilot | `/banner` | `always`, `once`, `never` | scalar | global |
| copilot | `/keepAlive` | `on`, `off`, `busy` | scalar | global |
| copilot | `/worktreeBaseRef` | `head`, `defaultBranch` | scalar | global |
| copilot | `/autoUpdatesChannel` | `stable`, `prerelease` | scalar | global |
| copilot | `/logLevel` | `none`, `error`, `warning`, `info`, `debug`, `all`, `default` | scalar | global |
| copilot | `/stayInAutopilot` | boolean | scalar | global |
| copilot | `/continueOnAutoMode` | boolean | scalar | global |
| copilot | `/showReasoning` | boolean | scalar | global |
| copilot | `/experimental` | boolean | scalar | global |
| copilot | `/toolSearch` | boolean | scalar | global |
| copilot | `/connectors` | boolean | scalar | global |
| copilot | `/customAgents/defaultLocalOnly` | boolean | scalar | global |
| copilot | `/autoUpdate` | boolean | scalar | global |
| copilot | `/beep` | boolean | scalar | global |
| copilot | `/beepOnSchedule` | boolean | scalar | global |
| copilot | `/compactPaste` | boolean | scalar | global |
| copilot | `/copyOnSelect` | boolean | scalar | global |
| copilot | `/inlineImages` | boolean | scalar | global |
| copilot | `/mouse` | boolean | scalar | global |
| copilot | `/pinnedPrompts` | boolean | scalar | global |
| copilot | `/renderHexColors` | boolean | scalar | global |
| copilot | `/screenReader` | boolean | scalar | global |
| copilot | `/scrollbar` | boolean | scalar | global |
| copilot | `/shellShortcut` | boolean | scalar | global |
| copilot | `/showTimestamps` | boolean | scalar | global |
| copilot | `/showTipsOnStartup` | boolean | scalar | global |
| copilot | `/sidebar` | boolean | scalar | global |
| copilot | `/streamerMode` | boolean | scalar | global |
| copilot | `/terminalNotifications` | boolean | scalar | global |
| copilot | `/terminalProgress` | boolean | scalar | global |
| copilot | `/updateTerminalTitle` | boolean | scalar | global |
| copilot | `/taskbarPresence` | boolean. [Copilot][copilot] reads it on Windows only. | scalar | global |
| copilot | `/ide/autoConnect` | boolean | scalar | global |
| copilot | `/ide/openDiffOnEdit` | boolean | scalar | global |
| copilot | `/dynamicRetrieval/skills` | boolean | scalar | global |
| copilot | `/tabs/enabled` | boolean | scalar | global |
| copilot | `/subagents/agents/rubber-duck/autoInvoke` | boolean | scalar | global |
| copilot | `/subagents/disabledSubagents` | list of strings | set | global |
| copilot | `/enabledMcpServers` | list of strings | set | global |
| copilot | `/sandbox/allowBypass` | boolean | scalar | global |
| cursor | `/permissions/allow` | list of strings, required by the harness | set | global |
| cursor | `/permissions/deny` | list of strings, required by the harness | set | global |
| cursor | `/approvalMode` | `allowlist`, `auto-review`, `unrestricted` | scalar | global |
| cursor | `/sandbox/readBoundary` | `system`, `workspace` | scalar | global |
| cursor | `/editor/vimMode` | boolean, required by the harness | scalar | global |
| cursor | `/notifications` | boolean | scalar | global |
| cursor | `/hints` | boolean | scalar | global |
| cursor | `/rewind` | boolean | scalar | global |
| cursor | `/suggestNextPrompt` | boolean | scalar | global |
| cursor | `/maxMode` | boolean | scalar | global |
| cursor | `/display/showLineNumbers` | boolean | scalar | global |
| cursor | `/display/showThinkingBlocks` | boolean | scalar | global |
| cursor | `/display/showStatusIndicators` | boolean | scalar | global |
| cursor | `/display/showStatusLineRunningTime` | boolean | scalar | global |
| cursor | `/attribution/attributeCommitsToAgent` | boolean | scalar | global |
| cursor | `/attribution/attributePRsToAgent` | boolean | scalar | global |
| opencode | `/model` | string | scalar | global |
| opencode | `/small_model` | string | scalar | global |
| opencode | `/default_agent` | string | scalar | global |
| opencode | `/autoupdate` | boolean or `notify` | scalar | global |
| opencode | `/snapshot` | boolean | scalar | global |
| opencode | `/compaction/auto` | boolean | scalar | global |
| opencode | `/compaction/prune` | boolean | scalar | global |
| opencode | `/permission` | `allow`, `ask`, `deny` | scalar | global |
| opencode | `/permission/<tool>` | `allow`, `ask` or `deny`, or a table of patterns to those actions | map | global |
| opencode | `/share` | `manual`, `disabled`. Turning on `auto` is left to the user. | scalar | global |
| qoder | `/model/name` | string | scalar | global |
| qoder | `/general/defaultPermissionMode` | `default`, `accept_edits`, `plan`, `auto`, `bypass_permissions`, `dont_ask` | scalar | global |
| qoder | `/general/vimMode` | boolean | scalar | global |
| qoder | `/general/enableAutoUpdate` | boolean | scalar | global |
| qoder | `/general/enableNotifications` | boolean | scalar | global |
| qoder | `/output/format` | `text`, `json` | scalar | global |
| qoder | `/ui/theme` | string | scalar | global |
| qoder | `/ui/hideTips` | boolean | scalar | global |
| qoder | `/ui/hideBanner` | boolean | scalar | global |
| qoder | `/ui/showLineNumbers` | boolean | scalar | global |
| qoder | `/ui/compactToolOutput` | boolean | scalar | global |
| qoder | `/permissions/allow` | list of strings | set | global |
| qoder | `/permissions/ask` | list of strings | set | global |
| qoder | `/permissions/deny` | list of strings | set | global |
| qoder | `/security/disableYoloMode` | boolean | scalar | global |
| qoder | `/model/reasoningEffort` | `disabled`, `off`, `none`, `low`, `medium`, `high`, `xhigh`, `max` | scalar | global |
| antigravity | `/agentMode` | `accept-edits`, `auto`, `plan` | scalar | global |
| antigravity | `/toolPermission` | `request-review`, `proceed-in-sandbox`, `always-proceed`, `strict` | scalar | global |
| antigravity | `/artifactReviewPolicy` | `asks-for-review`, `agent-decides`, `always-proceed` | scalar | global |
| antigravity | `/allowNonWorkspaceAccess` | boolean | scalar | global |
| antigravity | `/enableTerminalSandbox` | boolean | scalar | global |
| antigravity | `/permissions/allow` | list of strings | set | global |
| antigravity | `/permissions/ask` | list of strings | set | global |
| antigravity | `/permissions/deny` | list of strings | set | global |
| antigravity | `/model` | string | scalar | global |
| antigravity | `/verbosity` | `high`, `medium`, `low` | scalar | global |
| antigravity | `/queuedMessages` | `queue`, `send-immediately` | scalar | global |
| antigravity | `/notifications` | boolean | scalar | global |
| antigravity | `/colorScheme` | `terminal`, `light`, `solarized light`, `colorblind-friendly light`, `dark`, `solarized dark`, `colorblind-friendly dark`, `tokyo night` | scalar | global |
| antigravity | `/altScreenMode` | `default`, `always`, `never` | scalar | global |
| antigravity | `/editorMode` | `default`, `vim` | scalar | global |
| antigravity | `/vimInsertFirst` | boolean | scalar | global |
| antigravity | `/runningLightSpeed` | `fast`, `medium`, `slow`, `off` | scalar | global |
| antigravity | `/pickerGrouping` | `none`, `grouped` | scalar | global |
| antigravity | `/showTips` | boolean | scalar | global |
| antigravity | `/showFeedbackSurvey` | boolean | scalar | global |
| antigravity | `/hideActiveItems` | boolean | scalar | global |
| antigravity | `/copyOnSelect` | boolean | scalar | global |
| antigravity | `/clearScrollbackOnResize` | boolean | scalar | global |
| antigravity | `/useTerminalCursor` | boolean | scalar | global |
| antigravity | `/disableSlashCommands` | boolean | scalar | global |
| amp | `/amp.showCosts` | boolean | scalar | global |
| amp | `/amp.notifications.enabled` | boolean | scalar | global |
| amp | `/amp.terminal.copyOnSelect` | boolean | scalar | global |
| amp | `/amp.terminal.detailsExpandedByDefault` | boolean | scalar | global |
| amp | `/amp.thread.autoArchiveOnQuit` | boolean | scalar | global |
| amp | `/amp.updates.mode` | `auto`, `warn`, `disabled` | scalar | global |
| amp | `/amp.git.commit.ampThread.enabled` | boolean | scalar | global |
| amp | `/amp.git.commit.coauthor.enabled` | boolean | scalar | global |
| droid | `/sessionDefaultSettings/autonomyLevel` | `off`, `low`, `medium`, `high` | scalar | global |
| droid | `/sessionDefaultSettings/interactionMode` | `auto`, `spec`, `agi` | scalar | global |
| droid | `/sessionDefaultSettings/model` | string | scalar | global |
| droid | `/sessionDefaultSettings/reasoningEffort` | `none`, `dynamic`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` | scalar | global |
| droid | `/outputStyle` | string | scalar | global |
| droid | `/diffMode` | `github`, `unified` | scalar | global |
| droid | `/soundFocusMode` | `always`, `focused`, `unfocused` | scalar | global |
| droid | `/completionSoundFocusMode` | `always`, `focused`, `unfocused` | scalar | global |
| droid | `/awaitingInputSoundFocusMode` | `always`, `focused`, `unfocused` | scalar | global |
| droid | `/toolResultDisplay` | `expanded`, `compact` | scalar | global |
| droid | `/logoAnimation` | `once`, `always`, `off` | scalar | global |
| droid | `/subagentAutonomyLevel` | `inherit`, `off`, `low`, `medium`, `high` | scalar | global |
| droid | `/includeCoAuthoredByDroid` | boolean | scalar | global |
| droid | `/enableDroidShield` | boolean | scalar | global |
| droid | `/ideAutoConnect` | boolean | scalar | global |
| droid | `/showThinkingInMainView` | boolean | scalar | global |
| droid | `/streamAssistantResponses` | boolean | scalar | global |
| droid | `/vimMode` | boolean | scalar | global |
| droid | `/blockOnMcpLoad` | boolean | scalar | global |
| droid | `/showTokenUsageIndicator` | boolean | scalar | global |
| droid | `/overrideTerminalColors` | boolean | scalar | global |
| droid | `/nerdFont` | boolean | scalar | global |
| droid | `/disabledSkills` | list of strings | set | global |
| droid | `/sandbox/enabled` | boolean | scalar | global |
| droid | `/sandbox/mode` | `per-command`, `whole-process` | scalar | global |
| droid | `/sandbox/filesystem/denyRead` | list of strings | set | global |
| droid | `/sandbox/filesystem/denyWrite` | list of strings | set | global |
| droid | `/sandbox/filesystem/allowWrite` | list of strings | set | global |
| droid | `/sandbox/network/allowedDomains` | list of strings | set | global |
| droid | `/extraKnownMarketplaces/<name>` | table with `source` | map | global |
| kilo | `/model` | string | scalar | global |
| kilo | `/small_model` | string | scalar | global |
| kilo | `/default_agent` | string | scalar | global |
| kilo | `/reasoning_display` | `expanded`, `preview`, `headline` | scalar | global |
| kilo | `/terminal_command_display` | `expanded`, `collapsed` | scalar | global |
| kilo | `/code_edit_display` | `expanded`, `collapsed` | scalar | global |
| kilo | `/web_search` | boolean | scalar | global |
| kilo | `/hide_prompt_training_models` | boolean | scalar | global |
| kilo | `/privacy_mode` | boolean | scalar | global |
| kiro | `/chat.defaultModel` | string | scalar | global |
| kiro | `/chat.defaultAgent` | string | scalar | global |
| kiro | `/chat.outputStyle` | string | scalar | global |
| kiro | `/chat.agentEngine` | `v3`, `v2` | scalar | global |
| kiro | `/acp.agentEngine` | `v3`, `v2` | scalar | global |
| kiro | `/chat.notificationMethod` | `auto`, `bel`, `osc9` | scalar | global |
| kiro | `/chat.historyMode` | `session`, `global` | scalar | global |
| kiro | `/chat.screenReader` | `auto`, `on`, `off` | scalar | global |
| kiro | `/chat.workflowLaunch` | `when-asked`, `large-tasks`, `delegate-all` | scalar | global |
| kiro | `/chat.sessionDashboard.scope` | `current`, `all` | scalar | global |
| kiro | `/chat.disableGranularTrust` | boolean | scalar | global |
| kiro | `/chat.enableNotifications` | boolean | scalar | global |
| kiro | `/chat.greeting.enabled` | boolean | scalar | global |
| kiro | `/chat.disableWrap` | boolean | scalar | global |
| kiro | `/chat.disableAutoCompaction` | boolean | scalar | global |
| kiro | `/chat.enablePromptHints` | boolean | scalar | global |
| kiro | `/chat.autoExpandToolOutput` | boolean | scalar | global |
| kiro | `/chat.sessionDashboard.indexResponses` | boolean | scalar | global |
| kiro | `/chat.allowAnimations` | boolean | scalar | global |
| kiro | `/chat.allowAsciiArt` | boolean | scalar | global |
| kiro | `/chat.allowIcons` | boolean | scalar | global |
| kiro | `/chat.preserveScrollback` | boolean | scalar | global |
| kiro | `/chat.terminalTitle` | boolean | scalar | global |
| kiro | `/chat.showThinking` | boolean | scalar | global |
| kiro | `/chat.enableKnowledge` | boolean | scalar | global |
| kiro | `/chat.enableCodeIntelligence` | boolean | scalar | global |
| kiro | `/chat.enableThinking` | boolean | scalar | global |
| kiro | `/chat.enableWorkflows` | boolean | scalar | global |
| kiro | `/toolSearch.enabled` | boolean | scalar | global |
| kiro | `/chat.enableAutoAgentUpgrade` | boolean | scalar | global |
| kiro | `/app.disableAutoupdates` | boolean | scalar | global |
| kiro | `/app.warnOnElevatedPrivileges` | boolean | scalar | global |
| kiro | `/knowledge.defaultIncludePatterns` | list of strings | set | global |
| kiro | `/knowledge.defaultExcludePatterns` | list of strings | set | global |
| gemini | `/general/defaultApprovalMode` | `default`, `auto_edit`, `plan` | scalar | global |
| gemini | `/general/notificationMethod` | `auto`, `osc9`, `osc777`, `bell` | scalar | global |
| gemini | `/general/preferredEditor` | `vscode`, `vscodium`, `windsurf`, `cursor`, `zed`, `antigravity`, `sublimetext`, `lapce`, `nova`, `bbedit`, `vim`, `neovim`, `emacs`, `hx`, `emacsclient`, `micro` | scalar | global |
| gemini | `/general/vimMode` | boolean | scalar | global |
| gemini | `/general/enableNotifications` | boolean | scalar | global |
| gemini | `/general/checkpointing/enabled` | boolean | scalar | global |
| gemini | `/general/plan/enabled` | boolean | scalar | global |
| gemini | `/general/enableAutoUpdate` | boolean | scalar | global |
| gemini | `/general/enableAutoUpdateNotification` | boolean | scalar | global |
| gemini | `/model/name` | string | scalar | global |
| gemini | `/output/format` | `text`, `json` | scalar | global |
| gemini | `/ui/inlineThinkingMode` | `off`, `full` | scalar | global |
| gemini | `/ui/loadingPhrases` | `tips`, `witty`, `all`, `off` | scalar | global |
| gemini | `/ui/errorVerbosity` | `low`, `full` | scalar | global |
| gemini | `/ui/hideTips` | boolean | scalar | global |
| gemini | `/ui/hideBanner` | boolean | scalar | global |
| gemini | `/ui/showLineNumbers` | boolean | scalar | global |
| gemini | `/ui/compactToolOutput` | boolean | scalar | global |
| gemini | `/tools/allowed` | list of strings | set | global |
| gemini | `/tools/confirmationRequired` | list of strings | set | global |
| gemini | `/tools/sandboxAllowedPaths` | list of strings | set | global |
| gemini | `/tools/exclude` | list of strings | set | global |
| gemini | `/tools/sandboxNetworkAccess` | boolean | scalar | global |
| gemini | `/tools/shell/backgroundCompletionBehavior` | `silent`, `inject`, `notify` | scalar | global |
| gemini | `/security/toolSandboxing` | boolean | scalar | global |
| gemini | `/security/disableYoloMode` | boolean | scalar | global |
| gemini | `/security/disableAlwaysAllow` | boolean | scalar | global |
| gemini | `/security/enablePermanentToolApproval` | boolean | scalar | global |
| gemini | `/security/autoAddToPolicyByDefault` | boolean | scalar | global |
| gemini | `/security/blockGitExtensions` | boolean | scalar | global |
| gemini | `/skills/disabled` | list of strings | set | global |
| gemini | `/context/fileFiltering/respectGitIgnore` | boolean | scalar | global |
| gemini | `/context/fileFiltering/respectGeminiIgnore` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/thinking_display_mode` | `show_and_collapse`, `always_show`, `never_show` | scalar | global |
| warp | `/agents/warp_agent/other/open_conversation_layout_preference` | `new_tab`, `split_pane` | scalar | global |
| warp | `/agents/warp_agent/other/long_running_command_submission_mode` | `send_immediately`, `queue_until_command_completes` | scalar | global |
| warp | `/agents/warp_agent/other/orchestration_message_display_mode` | `show_and_collapse`, `always_show`, `always_collapse` | scalar | global |
| warp | `/agents/warp_agent/other/default_prompt_submission_mode` | `interrupt`, `queue` | scalar | global |
| warp | `/general/default_session_mode` | `terminal`, `agent`, `cloud_agent`, `docker_sandbox` | scalar | global |
| warp | `/agents/warp_agent/other/auto_approve_bypasses_command_denylist` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/cloud_agent_computer_use_enabled` | boolean | scalar | global |
| warp | `/agents/mcp_servers/file_based_mcp_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/agent_attribution_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/show_agent_notifications` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/show_conversation_history` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/should_show_oz_updates_in_zero_state` | boolean | scalar | global |
| warp | `/agents/warp_agent/other/should_render_use_agent_toolbar_for_user_commands` | boolean | scalar | global |
| warp | `/agents/warp_agent/is_any_ai_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/code_suggestions_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/intelligent_autosuggestions_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/agent_mode_query_suggestions_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/git_operations_autogen_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/rule_suggestions_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/active_ai/shared_block_title_generation_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/input/ai_auto_detection_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/input/nld_in_terminal_enabled` | boolean | scalar | global |
| warp | `/agents/warp_agent/input/show_model_selectors_in_prompt` | boolean | scalar | global |
| warp | `/agents/warp_agent/input/show_agent_tips` | boolean | scalar | global |
| warp | `/agents/warp_agent/input/include_agent_commands_in_history` | boolean | scalar | global |
| warp | `/agents/knowledge/rules_enabled` | boolean | scalar | global |
| warp | `/agents/knowledge/warp_drive_context_enabled` | boolean | scalar | global |

`<name>` is any key. In `/permission/<tool>`, `<tool>` is one of `read`,
`edit`, `glob`, `grep`, `bash`, `task`, `todowrite`, `skill`, `lsp`,
`question`, `webfetch`, `websearch`, `external_directory` or `doom_loop`.
[OpenCode][opencode] applies the last pattern that matches, so grim compares a
pattern table whole and needs its keys in sorted byte order.

## Limits {#limits}

### Delivered, not enforced {#delivered}

grim delivers a value into the file a harness reads, and does not enforce it.
The harness, the user or another tool can change the file. A source with
higher precedence can override it. A control that must hold belongs in a
harness's managed settings, where the harness offers them.

A value the harness dropped from its file returns at the next `grim install`.
A value someone changed reads as modified and refuses until `--force`.

grim checks each value against its own field list, not the harness's list. A
harness reports a value it rejects through its own check, such as
`claude doctor` in [Claude Code][claude]. The values a check's error text lists
can differ from the values it accepts. In version 2.1.318, `claude doctor`
accepts `manual` for `/permissions/defaultMode`, although its error text for an
invalid mode leaves `manual` out.

Some harnesses change or ignore a delivered value on their own:

- [Antigravity][antigravity] rewrites its file at startup and drops every key
  set to its default, as its [settings docs][antigravity-settings] say. Such a
  value reads `missing` until the next `grim install`.
- [Kilo][kilo]: clearing a setting in its Settings panel removes it from every
  config file. The value reads `missing` until the next `grim install`.
- [Kiro][kiro] checks no value, so grim's check is the only one. A value it
  does not take stays in the file, and the harness ignores it or uses its
  default.
- [Warp][warp] shows a banner for a value it does not recognize and falls back
  to the default, as its [settings file docs][warp-settings] say.
- [Droid][droid]: each model accepts its own subset of
  `/sessionDefaultSettings/reasoningEffort`, so a value grim delivers can be
  refused for one model.
- [Droid][droid] creates `~/.factory/settings.json` with defaults on its first
  run, as its [settings docs][droid-settings] say. They do not say which
  values the new file holds. A default that differs from a package's value is
  one grim did not write, so the install refuses until `--force`.
- [Antigravity][antigravity] does not offer `/agentMode` `auto` to business
  or enterprise accounts, or without its terminal sandbox. The CLI then starts
  in its default mode, as its [settings schema][antigravity-schema] says.
- [Claude Code][claude] `/effortLevel`: at global scope grim writes the user
  `settings.json`, where Opus 5.5 and later models ignore the key. Opus 5,
  Fable 5.1 and earlier models still apply it there. In a project's local file
  it applies to every model, as its [settings reference][claude-effort] says.
- [Claude Code][claude] `/sandbox/enabled` in a project: while an
  administrator requires the sandbox, a `false` grim writes there is ignored
  when the user's `~/.claude/settings.json` sets `true`. Its [settings
  reference][claude-sandbox-enabled] and [sandboxing docs][claude-sandboxing]
  list the case.
- [Claude Code][claude] `/permissions/defaultMode` in a project: a
  conversation the VS Code extension starts ignores the value grim writes
  there. It reads only user, managed and `--settings` values, as its
  [settings reference][claude-default-mode] says.
- [Claude Code][claude] `/extraKnownMarketplaces/<name>` in a project: the
  harness honors the entry only after the user accepts the workspace trust
  dialog for that folder. Before that it ignores the entry without a message,
  in a `-p` run too, as its [settings reference][claude-marketplaces] says.

### Shadowing {#shadowing}

- [Cursor][cursor]: a project `.cursor/cli.json` with a `permissions` block
  replaces the global block grim writes.
- [Qoder][qoder]: its own pages disagree on how lists from different files
  merge. Treat a global list as one a project file can replace.
- [Claude Code][claude] in a linked worktree: grim writes the worktree's own
  `.claude/settings.local.json`. The harness also applies the main checkout's
  local file. A single value there shadows the worktree's, and permission lists
  from both files apply.
- [Copilot][copilot] reads `.claude/settings.json` and
  `.claude/settings.local.json` for a shared subset, including
  `extraKnownMarketplaces` and `hooks`, as its [config directory
  docs][copilot-config-dir] say. A [Claude Code][claude] project value can
  therefore reach [Copilot][copilot]. Marketplaces merge, the repository file
  overrides the user file, and `autoUpdate` is ignored.
- [Claude Code][claude] sandbox paths: while an administrator requires the
  sandbox, the harness ignores `sandbox.filesystem.allowWrite` entries in a
  project's local file. It also ignores an `allowRead` entry there under a
  path that managed, `--settings` or user settings deny reading. Its
  [sandboxing docs][claude-sandboxing] list both cases. A relative path
  resolves against the project root in a project file. In the user file it
  resolves against `~/.claude`, so one entry names a different path at each
  scope.
- [Antigravity][antigravity]: its pages disagree on whether project rules
  combine with the global `permissions` lists or take precedence. Treat a
  global list as one a project rule can override.
- [Amp][amp]: a workspace `.amp/settings.json` or `.amp/settings.jsonc`
  overrides the global file. Its docs do not say which global file wins when
  both exist, and grim edits `settings.jsonc` then.
- [Droid][droid]: an organization value wins for a hard control, and a
  project value wins for a session default. `~/.factory/settings.local.json`
  merges on top of the file grim writes, as its [hierarchical settings
  docs][droid-hierarchy] say. The sandbox `allowWrite` and `allowedDomains`
  lists accumulate across levels by the general array rule alone.
- [Kilo][kilo]: a project `kilo.jsonc` or `.kilo/kilo.jsonc` overrides the
  global file. Kilo also deep-merges a `kilo.json`, `opencode.json`,
  `opencode.jsonc` or `config.json` beside it, in an order its docs do not
  state.
- [Kiro][kiro]: a workspace `.kiro/settings/cli.json` overrides a global
  value for each key it sets, except a global-only key. Its [settings
  reference][kiro-settings] names `chat.agentEngine` and `chat.terminalTitle`
  among those. A workspace
  `knowledge.defaultIncludePatterns` or `knowledge.defaultExcludePatterns`
  list replaces the global list whole.
- [OpenCode][opencode]: in version 1.19.5 the harness also merges a
  `config.json` beside `opencode.json`, beneath both spellings of that file.
  Its [config docs][opencode-config] do not name it, and grim never reads it.
  A value grim writes therefore outranks the user's value there, with no
  refusal.
- [Gemini][gemini]: a project `.gemini/settings.json` and the system settings
  file override the global file, in the order its [configuration
  reference][gemini-config] gives. A project list replaces `tools.allowed`,
  `tools.confirmationRequired` and `tools.sandboxAllowedPaths` whole.

### Reload {#reload}

Each harness reloads its settings on its own terms:

| Harness | When a change takes effect |
|---|---|
| [Claude Code][claude] | Most edits apply without a restart, permissions included. `model` and `effortLevel` are read at session start. See its [settings docs][claude-settings]. |
| [Codex][codex] | Rules are read at startup, so restart. No primary source says when `config.toml` is read, so assume a restart. |
| [Copilot][copilot] | Most settings apply right away and a few need a restart, as its [settings docs][copilot-settings] say. |
| [Cursor][cursor] | No primary source says, so assume a restart. Its [configuration docs][cursor-config] note that the CLI manages some fields and may overwrite them. |
| [OpenCode][opencode] | Its [config docs][opencode-config] do not say, so assume a restart. |
| [Qoder][qoder] | Its [settings reference][qoder-settings] lists no restart for `model.name`, the `general` and `ui` keys, and the permission lists. `security.disableYoloMode` needs one. `output.format` is not listed, so assume a restart. |
| [Warp][warp] | Warp reloads the file as soon as it changes. Its Settings panel and its bundled `modify-settings` skill write the same file, so a value changed there reads as modified. See its [settings file docs][warp-settings]. |

The pages read for [Amp][amp], [Antigravity][antigravity], [Droid][droid],
[Gemini][gemini], [Kilo][kilo] and [Kiro][kiro] do not say when a change takes
effect, so assume a restart.

### Moved config roots {#moved-roots}

- When a shell `CLAUDE_CONFIG_DIR` moves, grim cannot name the old root. It
  warns that the file lacks a value grim recorded, and that a copy may remain
  at an old root grim cannot name.
- When a `CLAUDE_CONFIG_DIR` set in the `env` of [Claude Code][claude]'s own
  settings moves, grim names the copy at the old root. Install warns that the
  copy is left in place, and uninstall lists it in `abandoned_entries`.
- When `OPENCODE_CONFIG` points elsewhere, a recorded output sits in a file
  the run does not resolve. The run sets it aside with a warning and holds the
  package's pin. `grim status` reads `outdated`, and each run fetches the
  layer again.

## Status {#status}

A settings row on `grim status` reads `gated` or `not-armed` while a gate
holds. Its `arming` array names each harness's cause, and [Hook
arming][json-hook-arming] lists every cause and their order. A lock pin with
an empty record behind it reads `outdated`.

## Declare and publish {#publish}

A consumer declares a package in a `[settings]` table of `grimoire.toml`, or
with `grim add --kind settings <ref>`:

<!-- doc-norun: names a registry the reader does not own -->
```toml
[settings]
team-defaults = "ghcr.io/acme/team-defaults:1"
```
<!-- /doc-norun -->

A publisher lists the package under `[settings.<name>]` in `publish.toml`,
with its source at `settings/<name>.toml`. `grim release` takes
`--kind settings` for a single document. [Publishing][publishing] covers the
manifest.

## See also {#see-also}

- [Hooks reference][hooks] covers workspace consent, which settings share.
- [Hook arming][json-hook-arming] lists every `cause` token.
- [Error document][json-error] shows the shape that `pieces` extends.
- [MCP server artifacts][mcp-servers] covers another kind that writes into a
  client's own config.

<!-- internal -->
[stability-unstable]: ./stability.md#unstable
[config-experimental]: ./configuration.md#options-experimental
[hooks-consent]: ./hooks.md#consent
[hooks-trust-flags]: ./hooks.md#trust-flags
[artifacts-requires]: ./artifacts.md#requires
[fields]: #fields
[files]: #files
[root-fence]: #root-fence
[codex-rules-file]: #codex-rules
[conflicts]: #conflicts
[json-error]: ./json-interface.md#error-document
[json-hint]: ./json-interface.md#error-hint
[json-hook-arming]: ./json-interface.md#hook-arming
[publishing]: ./publishing.md
[hooks]: ./hooks.md
[mcp-servers]: ./mcp-servers.md

<!-- external -->
[claude]: https://code.claude.com
[codex]: https://developers.openai.com/codex
[copilot]: https://github.com/features/copilot
[cursor]: https://cursor.com
[opencode]: https://opencode.ai
[qoder]: https://qoder.com
[amp]: https://ampcode.com
[antigravity]: https://antigravity.google
[droid]: https://factory.ai
[kilo]: https://kilo.ai
[kiro]: https://kiro.dev
[gemini]: https://geminicli.com
[warp]: https://warp.dev
[git]: https://git-scm.com
[claude-settings]: https://code.claude.com/docs/en/settings
[claude-sandboxing]: https://code.claude.com/docs/en/sandboxing#repository-settings-under-an-admin-required-sandbox
[claude-effort]: https://code.claude.com/docs/en/settings-reference#effortlevel
[claude-sandbox-enabled]: https://code.claude.com/docs/en/settings-reference#sandbox-enabled
[claude-default-mode]: https://code.claude.com/docs/en/settings-reference#permissions-defaultmode
[claude-marketplaces]: https://code.claude.com/docs/en/settings-reference#extraknownmarketplaces
[codex-rules]: https://learn.chatgpt.com/docs/agent-configuration/rules
[copilot-settings]: https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/change-settings
[copilot-config-dir]: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference
[cursor-config]: https://cursor.com/docs/cli/reference/configuration
[opencode-config]: https://opencode.ai/docs/config/
[qoder-settings]: https://docs.qoder.com/cli/settings-reference
[antigravity-settings]: https://antigravity.google/docs/settings
[droid-hierarchy]: https://docs.factory.com/enterprise/hierarchical-settings-and-org-control
[droid-settings]: https://docs.factory.com/droid-cli/settings
[antigravity-schema]: https://antigravity.google/schemas/v1/settings.json
[kiro-settings]: https://kiro.dev/docs/reference/settings
[gemini-config]: https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md
[warp-settings]: https://docs.warp.dev/terminal/settings
[warp-files]: https://docs.warp.dev/terminal/settings/file-locations
