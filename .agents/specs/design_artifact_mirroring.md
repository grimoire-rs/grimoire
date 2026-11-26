# Design: Artifact mirroring

**Status:** Proposed · **Date:** 2026-11-24 · **Decision record:** `.agents/adr/adr_artifact_mirroring.md` (Dn below refers to it) · **Source:** `.agents/discussions/artifact-mirroring.md`

Contracts here are written so a tester can write failing tests from them without reading code. "Frozen" and "Unstable" refer to `docs/src/content/docs/stability.md` after amendment 3 of the ADR.

## 1. Components and boundaries

```
 operator CI / shell
   │  grim mirror sync            (D2: unknown name → PATH grim-mirror, argv "sync")
   ▼
 grim ── exec ──► grim-mirror  (crates/grim-mirror, no dependency on the grimoire crate)
                    │  git (hardened, D12)  ──► upstream repos (https:// or file://)
                    │  $GRIM_BIN convert …   (D9, Unstable)       ┐
                    │  $GRIM_BIN describe … --format json         ├─ grim CLI + JSON only
                    │  $GRIM_BIN publish --format json [--dry-run]┘
                    ▼
                  OCI registry  ({registry}/{upstream}/{kind}/{name}, D16)
```

| Component | Owns | Never does |
|---|---|---|
| grim, dispatch (`src/main.rs` parse-error path) | Finding and running `grim-<name>` | Telemetry, context init, reading `GRIM_BIN` |
| grim, `publish.toml` `mirror` table (`src/command/publish.rs`, `src/oci/annotations.rs`) | Validating the table, emitting annotations, precedence | Licence or SPDX validation, git |
| grim, `grim convert` (`src/command/convert.rs`, inverse beside `src/install/vendor.rs`) | Claude → general form for one unit | Network, writing outside `--out` |
| `grim-mirror` | Fetch, enumerate, kind and licence gates, versioning, staging, driving grim, the run report | Running upstream code, checkout, any registry write except through `grim publish` |

## 2. Data flow of one run

1. Parse argv; load `mirror.toml` (79 absent, 78 invalid). `GRIM_OFFLINE` truthy → 81 before any work.
2. Resolve grim (`GRIM_BIN` if absolute, else `grim` on `PATH`); capability probe (ADR D13): `grim schema --kind publish` must define a `mirror` property on the package entry, and with any `dialect = "claude"` entry `grim convert --help` must exit 0; else 69 naming the missing capability.
3. For each selected `[[upstream]]`, in file order: clone or update the repository in the work dir (§ 5.1). A failed fetch marks the upstream `failed`; its units are not reported individually; the run continues.
4. Enumerate units at the candidate commit(s) (§ 5.2, § 5.3). Apply kind gate (§ 5.4), name gate, staging gate (§ 5.5), licence gate (§ 5.6).
5. Per unit, read what the target repository holds (§ 5.7 probes), then **reconcile** the published versions in the window against their recorded revision (`unchanged` / `refused`) and **select** the new versions to publish (§ 5.7).
6. Stage selected and reconciled units at their content commit (§ 5.5), convert when `dialect = "claude"` (§ 3.4), write one `publish.toml` per group (§ 5.8), run `grim publish` (selected) or `grim publish --dry-run` (reconciled, output-drift check, § 5.8).
7. Bundles last, from members that are `published` or verified `unchanged` (§ 5.9). `retire` entries take § 5.10 instead of 5–6.
8. Print the report (§ 6.2); exit per § 6.3.

## 3. grim-side contracts

### 3.1 Plugin dispatch (Frozen)

**Trigger.** Only when clap's parse fails with `ErrorKind::InvalidSubcommand`. Then grim scans `argv[1..]` (`std::env::args_os`, kept as `OsString` end to end; comparisons are ASCII on `OsStr`, never a lossy conversion) left to right:

- A token `--` ends the scan: no dispatch.
- A known global flag is consumed: `--offline`, `--locked`, `--global`, `-g` take no value; `--format`, `--color`, `--progress`, `--log-level`, `--config`, `--registry` take the next token unless written `--flag=value`.
- Any other token starting with `-` ends the scan: no dispatch.
- The first other token is the **candidate name**. It must equal clap's reported invalid subcommand, else no dispatch.

**Name grammar.** `^[a-z0-9][a-z0-9-]{0,62}$`. Anything else: no dispatch.

**Lookup.** Binary name `grim-<name>`, on Windows `grim-<name>.exe`. Walk `PATH` (`std::env::split_paths`) in order; skip an empty or non-absolute entry; the first entry holding a regular file of that name wins; on Unix it must have any execute bit, else lookup continues. No `PATHEXT`, no other extension.

**Not found.** clap's original error is printed exactly as today and grim exits 64. No JSON document under `--format json`, as today.

**Found.**
- If a global flag other than `--offline`/`--locked` appeared before the name, grim prints to stderr `grim: '<flag>' before a plugin name is not supported; write it after the name: grim <name> <flag>` (first such flag, as typed without its value) and exits 64. The plugin does not run.
- Child argv: the tokens after the name, unchanged `OsString`s. The program is the **absolute path** the lookup found, never a bare name (so no platform search runs and a `grim-<name>.bat` is never reached).
- Child env: grim's environment, plus the keys one dedicated function returns (`crate::env::plugin_child_env`, the only code that lands `GRIM_*` keys on a child env, per `subsystem-cli.md` § Configuration Forwarding): `GRIM_BIN=<path of the running grim>`, `GRIM_OFFLINE=1` when `--offline` was given, `GRIM_LOCKED=1` when `--locked` was given. Nothing removed.
- Unix: the process is replaced (`exec`). Exit status is the child's. If `exec` fails: `EACCES` → 77, any other error → 69, with `grim: cannot run <path>: <error>` on stderr.
- Windows: spawn, wait, exit with the child's full exit code (the `u32` passed through unchanged, an NTSTATUS such as `0xC0000005` included). While waiting grim ignores Ctrl-C and Ctrl-Break, so the console delivers them to the child alone. Spawn failure: as Unix.
- No telemetry event, no tracing output, no lock or state file touched.

**Unchanged:** `grim --help`, completions and `grim help <x>` list and know built-ins only; every built-in and alias wins because clap matched it.

### 3.2 `GRIM_BIN` (Frozen)

Set by grim on a plugin child only (§ 3.1). Value: `std::env::current_exe()` made absolute; on Unix canonicalised, on Windows never in `\\?\` verbatim form (the `dunce` simplification grim already depends on). grim never reads it. Env-var table row: "Set by grim for a plugin it dispatches: the path of the grim that dispatched it. Read by plugins, never by grim."

### 3.3 `publish.toml` per-entry `mirror` table (Frozen)

Placement: `[<kind>.<name>.mirror]` under any kind table (`skills`, `rules`, `agents`, `bundles`, `mcp`, `hooks`, `settings`, `mods`), `deny_unknown_fields`, optional.

| Key | Required | Grammar | Emits |
|---|---|---|---|
| `source` | yes | absolute URL, scheme `https` or `file`, no userinfo, no fragment, ≤ 2048 bytes | `com.grimoire.mirror.source`; when `https`, also `org.opencontainers.image.source` |
| `commit` | yes | `^[0-9a-f]{40}$` | `org.opencontainers.image.revision` |
| `path` | yes | `.` or `/`-separated segments, no empty, `.` or `..` segment, no leading `/`, no `\`, ≤ 1024 bytes | `com.grimoire.mirror.path` |
| `license` | yes | non-empty, ≤ 256 bytes, no control characters (grim does not parse SPDX) | `org.opencontainers.image.licenses` |
| `license-source` | yes | `detected` \| `declared` | `com.grimoire.mirror.license-source` |
| `deprecated` | no | same rule as the authored `deprecated` field | `com.grimoire.deprecated` |
| `replaced-by` | no | same rule as the authored `replaced-by` field | `com.grimoire.replaced-by` |

- **Precedence:** with the table present, each value it sets wins over every authored tier and every derived value for that one annotation (frontmatter, flag, per-entry and top-level `[metadata]`, git-derived). Annotations the table does not set follow the existing chain. A `file://` `source` does not touch `org.opencontainers.image.source`.
- **Errors:** an unknown key, a missing required key or a value outside its grammar fails the whole manifest before any push, exit 65, with the message naming `<kind>.<name>.mirror.<key>`; an unknown key reuses the existing unknown-field parse error and its exit.
- **Unchanged:** a manifest without the table produces byte-identical manifests and reports. The publish report gains no field. `grim schema --kind publish` gains the table (generated from the struct; an additive schema change).
- **Read-back:** `grim describe --format json` shows the new keys in `annotations` and the commit in `revision`, licence in `license`, https source in `repository`; no new describe field.

### 3.4 `grim convert` (Unstable)

```
grim convert <path> --from claude --out <dir> [--kind skill|rule|agent]
```

- `<path>`: a skill directory (holds `SKILL.md`) or a `.md` file. Kind inference as `grim build`: directory with `SKILL.md` → skill; `.md` → rule unless `--kind agent`. A kind mismatch, `--from` other than `claude`, or `--kind` outside the three → 64.
- `--out <dir>`: absent or empty directory, else 65 with no `reason` (convert has no `--force`, so no `forceable` either). Output: skill → `<dir>/<name>/` (all files copied, `SKILL.md` rewritten); agent → `<dir>/<name>.md`; rule → `<dir>/<stem>.md`. `<name>` is the frontmatter `name` (skill, agent), which must pass grim's name grammar, else 65.
- Global flags: `--format` honoured; no network, no config read, no scope.
- Additive CLI change: `grim --help` and every completion script gain the `convert` entry; no existing entry changes.

**Inverse rules** (registry = `CLAUDE_SKILL_FIELDS` / `CLAUDE_AGENT_FIELDS` / Claude rule registry, empty):

| Input top-level key | Action | Report |
|---|---|---|
| Common: skill `name description license compatibility allowed-tools metadata`; agent `name description model tools license metadata`; rule `paths license metadata` | kept as authored. `license` stays on agents and rules too: their frontmatter keeps unknown keys verbatim (`AgentFrontmatter::extra`, `RuleFrontmatter::extra`), the mirror's licence lookup reads it (§ 5.6), and dropping a licence statement from a file is the wrong default; the published licence annotation comes from the `mirror` table regardless | — |
| Equal to a row's `native` (e.g. `when_to_use`, `disallowedTools`), not common | moved to `metadata."claude.<field>"` | `moved {field: <native>, to: "claude.<field>"}` |
| Same, but `metadata."claude.<field>"` already present | top-level key removed, metadata value kept | `dropped {field, reason: "shadowed"}` |
| Same, value of the wrong YAML type (table below) | removed | `dropped {field, reason: "unrepresentable"}` |
| Anything else (`hooks`, `mcpServers`, `version`, `homepage`, …) | removed | `dropped {field, reason: "unknown-field"}` |

| `FieldType` | Accepted YAML value | Stored string |
|---|---|---|
| `Bool` | boolean | `true` / `false` |
| `String` | string | unchanged |
| `Enum(set)` | string in set | unchanged |
| `Integer` | integer | base-10 |
| `Float` | finite number | Rust `f64` shortest round-trip `Display` |
| `CommaList` | string, or sequence of non-empty strings none containing `,` | string unchanged; sequence joined with `, ` |

Invariant (unit-tested for every registry row): for each **moved** key (never a `shadowed` or dropped one), lifting the converted file for Claude at install yields the same native key with a semantically equal value under the row's `FieldType`: equal booleans, integers and floats, equal strings, and for `CommaList` the same ordered list of trimmed items whether the input was a string or a sequence (a sequence input lifts back as a sequence; a string input whose items are re-split is equal by items).

Frontmatter re-serialisation: keys keep input order, moved keys are removed from the top level and appended to `metadata` in input order; the body after the closing `---` is copied byte for byte; no frontmatter in a rule → file copied unchanged. **Body tokens:** each occurrence of `${CLAUDE_PLUGIN_ROOT}`, `$CLAUDE_PLUGIN_ROOT`, `${CLAUDE_PROJECT_DIR}`, `$CLAUDE_PROJECT_DIR`, `$ARGUMENTS` in any `.md` file of the unit is a warning `{file, line, token}` (1-based line, `file` relative to the unit).

**JSON report** (single object, always-present-null):

```json
{"kind": "skill", "name": "pdf", "from": "claude",
 "input": "/abs/skills/pdf", "output": "/abs/out/pdf",
 "moved":   [{"field": "user-invocable", "to": "claude.user-invocable"}],
 "dropped": [{"field": "hooks", "reason": "unknown-field"}],
 "warnings":[{"file": "SKILL.md", "line": 12, "token": "${CLAUDE_PLUGIN_ROOT}"}]}
```

Plain: a first line `converted <kind> <name> -> <output>`, then one table `Field | Action | Detail` (moved, dropped, warning rows). A kind mismatch says which fix applies, e.g. `<path> is a skill directory (holds SKILL.md); drop --kind or pass --kind skill`. Exit: 0 (dropped fields and warnings included), 64 usage, 65 frontmatter unparseable / missing or invalid `name` / occupied `--out` / symlink or special file in a skill directory, 74 I/O, 79 `<path>` absent.

## 4. `mirror.toml` grammar (Unstable)

```toml
schema = 1                                  # required, integer 1

[mirror]
registry = "ghcr.io/acme/mirror"            # required: host[:port][/path]
licenses = ["MIT", "Apache-2.0", "CC0-1.0"] # optional: SPDX ids; default every OSI-approved id
backfill = 1                                # optional, 1..=100, default 1

[[upstream]]
name    = "anthropic-skills"                # required, ^[a-z0-9][a-z0-9-]{0,62}$, unique
repo    = "https://github.com/anthropics/skills"  # required, https:// or file://, no userinfo, no #fragment
ref     = "main"                            # optional branch or tag; default remote HEAD
commit  = "0123…"                           # optional 40-hex pin; excludes ref
dialect = "agentskills"                     # optional: agentskills (default) | claude
version = "auto"                            # optional: auto (default) | snapshot
tag-pattern = '^v?(\d+\.\d+\.\d+)$'         # optional regex, first group = strict X.Y.Z
backfill = 3                                # optional, overrides [mirror].backfill
license = "MIT"                             # optional SPDX expression, declared fallback
unit-licenses = { "skills/pdf" = "Apache-2.0" }  # optional, path → expression
skills  = ["skills/*"]                      # optional globs (globset, `/`-anchored at repo root)
agents  = ["agents/*.md"]
rules   = ["rules/*.md"]
exclude = ["skills/internal-*"]             # optional, applies to every list
marketplace = ".claude-plugin/marketplace.json"  # optional
plugins = ["python-*", "backend-api"]       # required with marketplace: name globs
retire  = { reason = "moved upstream", replaced-by = "ghcr.io/acme/pdf:1",
            units = ["skills/old-tool"] }   # optional; units: target-repository suffixes of units gone upstream (§ 5.10)

[[upstream.bundle]]
name    = "anthropic-skills"                # required, name grammar, unique in the entry
members = ["skills/*"]                      # required globs over unit paths of this entry
```

Validation → exit 78, message `<file>: <key path>: <problem>`, where `<file>` is the path given to `--file` (`mirror.toml` for the default `./mirror.toml`): unknown key anywhere; `schema` ≠ 1; duplicate `upstream.name` or bundle name; `ref` with `commit` (a `commit` pin is valid in both modes, § 5.7); a `tag-pattern` that does not compile or has no capture group; `marketplace` without `plugins` or the reverse; an entry with no kind list and no marketplace; a `licenses` id the SPDX list lacks or `LicenseRef-*`; an unparseable `license` expression; `backfill` out of range; a path in `unit-licenses`, a `retire.units` suffix, a glob or `marketplace` that is absolute, contains a `..` segment or a backslash; a `repo` not matching the grammar above (`http://`, `ssh://`, `git@…`, userinfo on `https://` or `file://`, and a `#` fragment are refused here).

## 5. Mirror pipeline

### 5.1 Fetch

Work dir: `--work-dir <dir>` (kept, clones reused) or a fresh temp dir removed at exit. Clone dir: `<work>/repos/<sha256(url)[:16]>`. New: `git clone --filter=blob:none --no-checkout --no-local <url> <dir>`. Existing: `git fetch --prune --force origin '+refs/heads/*:refs/remotes/origin/*' '+refs/tags/*:refs/tags/*'`. Both then run `git remote set-head origin --auto`. Every call uses the hardened command builder (ADR D12) with deadlines: clone and fetch 600 s, other git calls 60 s. Tree listings use `-z`.

**Candidate commit:** `commit` if set (fetched by `git fetch origin <sha>` when absent; failure → upstream `failed`, `commit-unreachable`); else `ref` resolved as `refs/remotes/origin/<ref>`, else `refs/tags/<ref>^{commit}`, else upstream `failed` `ref-unknown` (never a bare rev-parse of `<ref>`); else `refs/remotes/origin/HEAD`.

**Path containment.** Every upstream-derived path (unit path, marketplace source string, `git-subdir.path`, component path in an entry or `plugin.json`, `metadata.pluginRoot`) must be `/`-separated, relative, free of `..` segments, backslashes and NUL, and stay inside the repository (component paths: inside the plugin root) after joining. Failure → that unit or plugin `refused` `unsafe-path`. A plugin name used as a path segment must pass grim's name grammar, else `refused` `invalid-name`.

### 5.2 Enumeration from kind lists

Paths come from `git ls-tree -r -z --name-only <candidate>`. A skill unit is a directory matched by a `skills` glob that holds `SKILL.md`; an agent or rule unit is a `.md` file matched by `agents` / `rules`. `exclude` removes matches. A matched directory with no `SKILL.md` is a staging check (§ 5.5), not silently dropped. Unit name: skill directory leaf, rule file stem, agent frontmatter `name`.

### 5.3 Enumeration from a marketplace

Rules follow Claude Code ≥ 2.1.239 as documented on 2026-11-24 (`research_artifact_mirroring_plan_review.md` findings 1–4; re-verify with the watchlist row). `marketplace` is read at the candidate commit as JSON; the **marketplace root** is the directory holding the `.claude-plugin/` that holds the file. `plugins[]` entries whose `name` matches a `plugins` glob are selected.

| `source` | Repository and plugin root |
|---|---|
| string, bare name (no `/`, no leading `./`) | this repository; root = marketplace root / `metadata.pluginRoot` (when set) / name |
| string, `.`, `./x` or `x/y` | this repository; root = marketplace root / string (`metadata.pluginRoot` not applied) |
| `{source: "github", repo: "<owner>/<repo>"}` | `https://github.com/<owner>/<repo>`; root `.` |
| `{source: "url", url}` | `url` when it matches the § 4 `repo` grammar and is `https://` (`file://` only when the marketplace upstream's own `repo` is `file://`, ED-8); root `.` |
| `{source: "git-subdir", url, path}` | `url` (or `<owner>/<repo>` shorthand → `https://github.com/<owner>/<repo>`); root `path` |
| `url`/`git-subdir` with `git@…`, `http://`, `ssh://`; `npm`, `archive`, `command`, anything else | not fetched; one `skipped` row, `source-unsupported` |

A URL from marketplace content that is not `https://` (and not that `file://` exception) is one `skipped` row, `source-unsupported`. **One broken plugin (ED-9, ED-12):** a repository that answers it does not exist, or lacks the pinned commit or ref, is one `refused` row (kind `bundle`, the plugin name, `upstream-unavailable`); any other fetch or read failure of it is one `failed` row of kind `bundle` carrying the git error's infra code; an unreadable or oversize manifest or entry, or a declared gitlink agent (one `agent` row), is `refused` `unsupported-entry`. Other plugins continue and the upstream stays `ok`. **External members:** a member from another repository records that repository as `com.grimoire.mirror.source`, its commit as revision and its commit date as `created`; an unreadable commit time is a `failed` row (exit 74), never epoch 0. A plugin bundle records the repository its commit belongs to (ED-13): the plugin's own repository and its root there when that commit is a member's, else the marketplace URL.

Pin on an object source: `sha` wins over `ref`; neither → that repository's HEAD. An external repository is fetched as in § 5.1 and inherits the entry's `version`, `tag-pattern`, `dialect` and licences.

**Manifest and `strict`.** `P` = `<root>/.claude-plugin/plugin.json` when present; `E` = the marketplace entry. `strict` defaults to `true`: with `P`, `P` declares the components and `E`'s `skills`, `agents`, `commands`, `hooks`, `mcpServers` are appended; without `P`, `E` is the manifest. `strict: false` with `P` present and `E` declaring any component is a Claude load conflict: one `skipped` row for the plugin, `plugin-conflict`, and no bundle.

**Components, per key of the effective manifest:**
- `skills` **adds** to the default `skills/` scan, except that an entry whose source is the marketplace root (`.` or `./`) and lists `skills` **replaces** the default scan. A listed path holding `SKILL.md` is one skill, else each direct child directory holding one is. A plugin root that itself holds `SKILL.md` is one skill.
- `agents` **replaces** the default `agents/*.md` when declared; only `.md` files count (a listed directory gets warning `agents-dir-ignored` and yields nothing).
- `commands` replaces the default `commands/` when declared; present either way → one `skipped` row (`kind-unsupported`, kind `command`; policy, deferred, ADR D8).
- `hooks/hooks.json` or `hooks` (`kind-unsupported`, `hook`), `.mcp.json` or `mcpServers` (`kind-unsupported`, `mcp`), `settings.json` (`kind-unsupported`, `settings`), `outputStyles`, `lspServers`, `monitors`, `bin/` (`component-unsupported`): one `skipped` row each.

Each selected plugin is also one bundle named after the plugin (§ 5.9).

### 5.4 Kind gate

Mirrored: skill, rule, agent, bundle. Refused kinds appear only as `skipped` rows (§ 5.3); a kind list cannot name them.

### 5.5 Staging

A unit is staged at its **content commit** (§ 5.7: the tag's commit in tag mode, the unit's own revision in snapshot mode), which is also where its licence is read (§ 5.6). Entries come from `git ls-tree -r -z <content commit> -- <unit path>`:

| Condition | Outcome |
|---|---|
| A mode `120000` (symlink) or `160000` (submodule) entry | `refused` `unsupported-entry` |
| Mode `100755` | written as an executable regular file (`0o755` on Unix) in staging; grim's packer writes every layer entry as `0o644` (`src/skill/skill_package.rs`), so the published layer equals a native `grim publish` of the same tree, and an executable bit reaches installs only if grim ever packs modes |
| A blob starting `version https://git-lfs.github.com/spec/v1` | `refused` `unsupported-build-step` |
| Skill directory without `SKILL.md` holding a file matching `*.src.md`, `*.tmpl`, `*.template`, `*.j2`, `*.hbs`, `*.njk` | `refused` `unsupported-build-step` |
| Skill directory without `SKILL.md` otherwise | `refused` `unsupported-build-step` (message: no `SKILL.md`) |
| Unit name fails grim's name grammar | `refused` `invalid-name` |
| Two units of one kind with one name in one entry (one plugin, in marketplace mode) | both `refused` `name-collision` |
| An entry named `.claude-plugin` (the name fold of `adr_mod_artifact_kind.md` § Implementation (D15 fix)) at the unit root | dropped from staging; warning `plugin-manifest-dropped` |

Blob sizes are read with `git cat-file --batch-check` one object at a time under the 600 s blob deadline (ED-11), so the first blob over a size cap stops every later transfer. In a blobless clone that call downloads the blob: the one over-cap blob arrives whole, bounded by the deadline and git's low-speed abort, plus up to 64 MiB of accepted blobs per unit. Blobs are written with `git cat-file --batch` into `<work>/stage/<run>/<group>/<kind-subdir>/…`; no checkout, no filter. With `dialect = "claude"` each staged unit is passed through `grim convert --from claude --format json` into the group dir; exit 65 → `refused` `convert-failed` with grim's message; the report's `dropped` and `warnings` become unit warnings.

**Licence notice carriage.** When the level that decided (§ 5.6) is the plugin or repository and its evidence is a file (a manifest field carries nothing), that level's file travels with the unit: a skill directory without a licence file of its own gets a copy as `LICENSE`; a single-file agent or rule, which cannot carry a second file, gets it in the package README companion `publish.toml` already supports (`<name>.README.md` beside the file, whose text names the upstream, the licence id and the full licence text). Residual, stated in the mirror reference: an installed agent or rule file holds no licence text; the notice is in its repository's description companion.

### 5.6 Licence

**Evidence per level** is one of absent, recognised, unrecognised. Levels in order: unit (skill directory: `SKILL.md` `license` plus licence files in it; agent or rule: frontmatter `license` only) → plugin root (`plugin.json` `license` plus licence files) → repository root (`package.json` `license` plus licence files). Licence files: names matching `LICEN[CS]E*` or `COPYING*`, case-insensitive, in that level's directory only. A plugin's own licence files decide at plugin level even without a `plugin.json`.

- A **field** parsed by `spdx` in lax mode (accepts `GPL-3.0`, `GPL-2.0+`) and canonicalised is recognised; a field that does not parse (`Complete terms in LICENSE.txt`) is no evidence.
- A **file** with an `SPDX-License-Identifier:` line is recognised by that value (lax), or unrecognised if it does not parse. Otherwise the text matcher decides: one match → recognised; none → **unrecognised**. A match on a GPL, LGPL or AGPL text alone is **unrecognised** (the text cannot say `-only` or `-or-later`) unless a field or SPDX line at the same level names the id, which the text then corroborates.
- A level is unrecognised if any of its licence files is; else recognised if any source is; else absent.

**Decision.** The first non-absent level decides. Recognised: all recognised sources must be equal as canonical expressions, else `refused` `license-conflict`. Unrecognised: `refused` `license-unrecognised`, unless `unit-licenses[<unit path>]` declares the unit, which is then recorded `declared` (an entry-level `license` does not override an unrecognised file). All levels absent: `unit-licenses[path]`, else the entry `license`, recorded `declared`; else `refused` `license-missing`. A declaration beside recognised evidence is ignored with warning `declaration-ignored`. An expression the allowlist does not admit (ADR D11) → `refused` `license-not-allowed`.

**Text matcher.** Normalise: in the leading header block only (lines before the first line that also occurs in the template, at most 10 non-empty lines), drop lines containing `copyright`, `©`, `(c)` or `all rights reserved`, and a title line naming the licence; lowercase; replace each non-alphanumeric run with one space; split into words. A copyright or title line is dropped only when it has at most 14 words and none of the clause words (`not no non noncommercial commercial only except prohibit never without cannot nor train training ai unless` and the like; ED-10); a kept header line counts as foreign text. A template matches when it covers ≥ 0.97 of its own word bigrams and the file has at most 2 bigrams the template lacks (templates under 300 bigrams: MIT, MIT-0, BSD-2, BSD-3, ISC, 0BSD, Unlicense) or `max(4, ⌈0.02 × template bigrams⌉)` (longer ones); any foreign bigram holding a clause word refuses on every template. Residual: a restriction worded without a clause word on a long template still matches. Exactly one matching template → its id; two or more → unrecognised. Templates: MIT, MIT-0, Apache-2.0 (with and without the appendix), BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, MPL-2.0, Unlicense, CC0-1.0, and the GPL-2.0, GPL-3.0, LGPL-2.1, LGPL-3.0, AGPL-3.0 texts (corroboration only, above). A golden corpus under `crates/grim-mirror/tests/licenses/` pins the outcomes, including variants that must **not** match: MIT plus a non-commercial clause, BSD-3 against BSD-2, MIT against MIT-0, a dual-licence file holding two texts.

### 5.7 Versions, reconciliation and selection

**Definitions.**
- *Snapshot-shaped:* a version `X.Y.Z` whose `Z` has exactly 14 decimal digits.
- *Eligible tags:* `git tag --merged <candidate> -l` filtered to names matching `tag-pattern` whose first group is strict `X.Y.Z` (no leading zeros) and not snapshot-shaped. Every other matching name, prerelease captures included, is ignored; one upstream warning `tags-ignored` gives the count and up to five names. Two eligible tags capturing one version with different peeled commits → that version is `refused` `version-collision` for every unit, naming both tags.
- *Mode:* `version = "snapshot"` forces snapshot mode; `auto` is tag mode when the eligible set is non-empty, else snapshot.

**Target probes** (per unit target repository, cached per run by `(repository, version)`): `grim describe <repo>:latest --format json`. Exit 0 → the published tag list is its `tags`. Exit 79 → no listing; each version the steps below consider is probed with `grim describe <repo>:<version>` (79 = not published). A published version's recorded revision is `annotations["org.opencontainers.image.revision"]` of its exact-version describe. Other exits: § 7.1. Per unit per run at most `2 + backfill` describe calls when `latest` resolves; without it, one exact probe per considered version (the eligible tags newest first until the first published one is found, plus the backfill window), still cached per run.

**Tag mode.** Watermark `W` = the highest published strict-semver version that is not snapshot-shaped (none → no watermark). *Selection*: eligible versions above `W` (all eligible versions when there is none), newest `backfill` of them, published oldest first. *Reconciliation*: `W` when an eligible tag captures it, plus every published version among the newest `backfill` eligible versions; each one's recorded revision is compared with its tag's peeled commit: equal → `unchanged`; different or no recorded revision → `refused` `upstream-moved`. `W` with no eligible tag → upstream warning `watermark-tag-gone`. Content commit = the tag's peeled commit; a unit absent at it has no row for that version. Residual: a moved tag below `W` and outside the window is not re-checked; `backfill` is the operator's knob for that cost.

**Snapshot mode.** Content commit (= revision) = `git log -1 --format=%H%x00%ct <candidate> -- <unit path>`; base `M.m` = the highest eligible-tag version merged into that commit (`git tag --merged <revision>`), else `0.0`; version = `M.m.<YYYYMMDDHHMMSS of %ct in UTC>`. Units of one repository may sit on different bases, each from its own revision. Published at that version: recorded revision equal → `unchanged`, else `refused` `version-collision`. Not published → selected.

`0.0.<ts>` sorts below every release `≥ 0.1.0` but above any `0.0.Z` release. Because the watermark ignores snapshot-shaped versions, an upstream's first tag (even `v0.0.1`) is still selected after a snapshot phase; floats then stay on the higher snapshot until a release above it (`0.1.0` or later), as the cascade never lowers a float.

### 5.8 Publish groups

Group key = (upstream, plugin or none, version, content commit). Per group, `publish.toml`:

```toml
registry = "ghcr.io"                      # host of [mirror].registry
[skills.pdf]
version = "0.0.20261120101500"
path = "skills/pdf"
repository = "acme/mirror/anthropic-skills/skills/pdf"   # D16 template minus host
[skills.pdf.mirror]
source = "https://github.com/anthropics/skills"
commit = "<40-hex content commit>"
path = "skills/pdf"
license = "Apache-2.0"
license-source = "detected"
```

Run: `$GRIM_BIN publish --format json [--dry-run]` in the group dir, env `SOURCE_DATE_EPOCH=<%ct of the group's content commit>` and `GIT_CEILING_DIRECTORIES=<parent of the stage root>`, deadline 600 s. Item `Pushed` → `published`; `DryRun` → `would-publish`; `Skipped` → re-read with `grim describe` of that exact version: equal revision → `unchanged` (another writer published the same commit), else `refused` (`upstream-moved` in tag mode, `version-collision` in snapshot). Every other outcome: § 7.1.

**One writer per target.** The mirror assumes a single writer per `[mirror] registry` prefix; the docs example sets a CI `concurrency` group. A second writer is detected only by the `Skipped` re-read above.

**Output drift.** For every reconciled `unchanged` unit the mirror stages it at its content commit and runs the group with `--dry-run`; the item's `digest` (the digest a real push of the same bytes reports, `publishing.md` § compression) is compared with the existing tag's describe `digest`. Different → the row stays `unchanged` with warning `output-drift`. The mirror never republishes a version; a new version arrives with the next upstream commit or tag, or through `retire`. Causes: a converter change, a mirror-file change, or a grim release that packs or annotates differently (grim writes no grim-version annotation, `src/oci/annotations.rs`, so a grim upgrade shows as drift only when it changes packing or the annotation set, once per unit).

### 5.9 Bundles

Members: this entry's units whose path matches a `members` glob (declared bundle), or the plugin's admitted units (plugin bundle), external `git-subdir` members included. Only members that are `published` or `unchanged` this run are referenced; `refused` and `skipped` members are left out and reported with `bundle` set; a `failed` member makes the bundle `failed` (message names the member). No member left → `refused` `empty-bundle`. The bundle source `bundles/<name>.toml` (grimoire.toml-shaped) lists members as **absolute** references `{registry}/{…}/{name}:<exact version>`.

**Version.** Tag mode: the tag's version, content commit the tag's commit. Snapshot mode: the newest `%ct` among every member's content commit (any repository) and the last commit touching the bundle's own pathspecs in its repository (plugin root plus the marketplace file; for a declared bundle each `members` glob as `:(glob)<pattern>`); the commit holding that time is the content commit; base `M.m` = the highest base among members; version `M.m.<ts>`.

**Existing version.** A bundle version already published is reconciled by dry-run digest (§ 5.8): equal → `unchanged`; different → `refused` `bundle-drift` (its resolved member set differs from the published one; a new version arrives with the next member or bundle commit).

**Check mode.** A bundle with any member that is only `would-publish` does not run grim (that member is not in the registry); it is reported `would-publish` from the plan with `members` filled. A bundle whose members are all `unchanged` takes the dry-run path above.

`mirror.path` = plugin root, or `.` for a declared bundle. `license` = each member's canonical expression in parentheses, deduplicated, sorted, joined with ` AND ` (`(MIT) AND (Apache-2.0 OR MIT)`); `license-source = declared` if any member's is, else `detected`. Bundles publish after every member group of their entry.

### 5.10 Retire

Units of a `retire` entry: every enumerable unit, plus each `retire.units` item, written as a target-repository suffix (`skills/old-tool`, `<plugin>/agents/reviewer`) for units no longer present upstream. For each: the highest strict-semver published version `X.Y.Z` (snapshot-shaped included); its recorded revision and `com.grimoire.mirror.path`; re-stage from them **with the current converter and licence lookup**, then publish `X.Y.(Z+1)` with `mirror.deprecated = <reason>` and `mirror.replaced-by` when set. Because that is a new version, its bytes may differ from `X.Y.Z` (converter or packing changes); this is stated in the reference, not refused. Upstream or commit unreachable → `refused` `upstream-unavailable`. A highest version that already carries `com.grimoire.deprecated` → `unchanged`. Takedown by deletion is a separate runbook (`mirroring.md`): the operator deletes the registry repositories with registry tooling; the mirror deletes nothing.

## 6. `grim-mirror` CLI (Unstable)

### 6.1 Invocation

```
grim-mirror [--file <path>] [--upstream <name>]... [--work-dir <dir>] [--fail-on refused|infra] [--format plain|json] <check|sync>
```

Every option is global: it works before or after `check` or `sync`.

```
grim-mirror --version | --help
```

`--file` default `./mirror.toml`; `--upstream` must name an entry (else 64); `check` = plan and report every unit, publishing nothing (`grim publish --dry-run`); `sync` = publish the planned units to the registry (these are the `--help` strings). `--fail-on` (§ 6.3) defaults to `refused` for `check` and `infra` for `sync`. Env read: `GRIM_BIN`, `GRIM_OFFLINE` (truthy `1|true|yes|on` → 81). Diagnostics on stderr; the report on stdout.

### 6.2 Report (`--format json`)

```json
{"schema": 1, "mode": "sync", "grim": "0.17.0", "grim_mirror": "0.17.0",
 "upstreams": [{"name": "anthropic-skills", "repo": "https://github.com/anthropics/skills",
                "commit": "<40-hex candidate>", "status": "ok", "error": null, "warnings": []}],
 "items": [{"upstream": "anthropic-skills", "plugin": null, "bundle": null,
            "kind": "skill", "name": "pdf", "path": "skills/pdf",
            "ref": "ghcr.io/acme/mirror/anthropic-skills/skills/pdf:0.0.20261120101500",
            "version": "0.0.20261120101500", "revision": "<40-hex>",
            "status": "published", "reason": null, "message": null,
            "license": {"expression": "Apache-2.0", "source": "detected"},
            "members": null, "warnings": [{"code": "body-token", "message": "SKILL.md:12 ${CLAUDE_PLUGIN_ROOT}"}]}],
 "summary": {"published": 1, "would_publish": 0, "unchanged": 0, "refused": 0, "skipped": 0, "failed": 0}}
```

Every key always present (null when not applicable). `grim` is the probed `grim --version`, `grim_mirror` this binary's version, so a report names both tools that produced it. `kind` ∈ `skill rule agent bundle command hook mcp settings mod component`; `members` is the list of member refs for a bundle, else null. `upstreams[].status` ∈ `ok failed`. Item order: upstream file order, then plugin name (null first), then kind order `skill rule agent bundle`, other kinds after, then name, then version ascending. Plain: one table `Upstream | Kind | Name | Version | Status | Reason`.

| `status` | Meaning |
|---|---|
| `published` | pushed this run |
| `would-publish` | check mode, would push |
| `unchanged` | the version exists at the same revision (may carry warning `output-drift`, § 5.8) |
| `refused` | not published; `reason` names why; exit 65 under `--fail-on refused` |
| `skipped` | a kind, component or source the mirror does not republish; never affects exit |
| `failed` | infrastructure failure for this unit or group |

### 6.3 Exit codes

Values equal grim's of the same name (unit test against `src/cli/exit_code.rs`). Before work: 64 usage, 79 mirror file absent, 78 mirror file invalid, 81 offline, 69 grim missing or lacking a probed capability. After every upstream ran:

| `--fail-on` | Exit |
|---|---|
| `infra` (default for `sync`) | the first infrastructure code met by a `failed` upstream or item, in file order (§ 7.1); else 0, refused items reported only |
| `refused` (default for `check`) | that infrastructure code if any; else 65 if any item is `refused`; else 0 |

Every `failed` upstream or item maps to a non-zero code (§ 7.1), so a scheduled `sync` goes red only on infrastructure, and a standing refusal (a list defect) is caught by `check` on the list repository's pull requests instead of turning every scheduled run red.

## 7. Error taxonomy

| Reason / condition | Status | Remediation the message names |
|---|---|---|
| `license-missing` | refused | add `unit-licenses` or entry `license`, or `exclude` the path |
| `license-unrecognised` | refused | read the licence file; declare the unit in `unit-licenses` if it permits redistribution, else `exclude` |
| `license-not-allowed` | refused | `exclude` it, or widen `[mirror] licenses` as publisher of record |
| `license-conflict` | refused | the sources at one level name different licences; a declaration cannot override detection, so ask upstream to fix it or `exclude` the path |
| `unsupported-build-step` | refused | mirror a committed rendered tree by path, or `exclude` |
| `unsupported-entry` | refused | `exclude`; symlinks and submodules are not mirrored |
| `unsafe-path` | refused | upstream path escapes its root; `exclude` the plugin or unit |
| `invalid-name`, `name-collision` | refused | `exclude` one path |
| `upstream-moved` | refused | upstream moved a tag: investigate; retire or rename the entry |
| `version-collision` | refused | wait for the next upstream commit, or (two tags, one version) ask upstream to drop one |
| `bundle-drift` | refused | the member set changed under one version; wait for the next member or bundle commit |
| `empty-bundle` | refused | fix `members` globs |
| `convert-failed`, `grim-refused` | refused | grim's message (e.g. `skill-plugin-manifest`) |
| `upstream-unavailable` | refused | takedown runbook (`mirroring.md`) |
| `kind-unsupported`, `component-unsupported`, `source-unsupported`, `plugin-conflict` | skipped | none needed |
| `commit-unreachable`, `ref-unknown`, fetch failure | upstream failed | check `repo`, `ref`, `commit`, network |

### 7.1 Failure matrix

| Call | Outcome | Mirror result | Exit contribution |
|---|---|---|---|
| `git clone` / `git fetch` | non-zero | upstream `failed` | 69 |
| any git call | deadline passed | upstream `failed` | 75 |
| `git ls-tree`, `cat-file`, `log`, `tag` | non-zero | upstream `failed` | 74 |
| `grim describe` | 0 / 79 | published / not published | — |
| `grim describe` | 80, 81, 69, 74 | unit `failed` | same code |
| `grim describe` | 75 or deadline | one retry, then unit `failed` | 75 |
| reconcile dry run (§ 5.8) | non-zero | row `failed` with grim's exit code; `refused` `grim-refused` on 65 | same code / — |
| `grim describe` (post-publish re-read) | non-zero | row `failed` | same code |
| retire (§ 5.10) `describe` / `publish` failure | non-zero | row `failed` | grim's code (80, 81 …), never folded to 69 |
| `grim describe` | other exit, or stdout not the describe document | unit `failed`, "unexpected grim output" | 69 |
| `grim convert` | 0 / 65 | staged / `refused` `convert-failed` | — / — |
| `grim convert` | other exit | unit `failed` | same code, 69 if outside the enum |
| `grim publish` | items `Pushed`/`DryRun`/`Skipped` | § 5.8 | — |
| `grim publish` | item `Failed`, or exit 65 | units `refused` `grim-refused` | — |
| `grim publish` | 75 or deadline | one retry, then the group's units `failed` | 75 |
| `grim publish` | 69, 74, 80, 81 | the group's units `failed` | same code |
| `grim publish` | stdout not a publish report, or a group unit missing from `items` | each missing unit `failed` | 69 |

## 8. Edge cases

- `grim config nosuch` with `grim-nosuch` on PATH: no dispatch (not first positional), 64 as today.
- `grim Mirror` / `grim my.tool`: name grammar fails, no dispatch.
- `PATH=":/usr/bin"` with `./grim-x` in cwd: the empty entry is skipped; not found.
- A plugin that exits 3: grim exits 3. Killed by SIGTERM: the shell sees 143 (exec).
- Upstream with tags but `version = "snapshot"`: versions `M.m.<ts>` above the last tag (ADR D5).
- Upstream gains its first tag while in `auto`: the next run selects it (the watermark ignores snapshot-shaped versions, § 5.7). A `0.0.Z` tag sorts below the published `0.0.<ts>` snapshots, so floats stay on them until a release `≥ 0.1.0`; a `≥ 0.1.0` tag moves them.
- Snapshot mode in a repository tagged `v1.2.0` with one unit last changed before that tag: that unit sits on `0.0.<ts>`, the others on `1.2.<ts>` (§ 5.7).
- Two units changed in one commit: same timestamp, different repositories, no collision.
- Upstream deletes a unit: no row; its published versions stay (`retire.units` marks them, § 5.10).
- A Claude skill with `hooks:` in frontmatter: dropped by convert, warning, published.
- Same skill name in two plugins of one marketplace: different repositories (`…/{plugin}/skills/{name}`), no collision.
- An operator edits `members`: takes effect at the bundle's next new version (`unchanged` until then).
- Dual licence file holding two texts: no template matches, the level is unrecognised, `refused` `license-unrecognised` unless declared in `unit-licenses`.
- A tag moved upstream below the watermark and outside the backfill window: not re-checked (§ 5.7 residual).
- Two mirror runs against one target at once: the loser's `Skipped` items are re-read and reported `unchanged` or `refused` (§ 5.8).

## 9. Test strategy

No network: upstreams are local git repositories built by the test (`file://` URLs, fixed `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE`), the registry is the suite's local one (`test/conftest.py` `registry` fixture). `grim-mirror` is staged at `test/bin/grim-mirror` by the test taskfile beside `test/bin/grim` (override `GRIM_MIRROR_COMMAND`).

**Unit (Rust):**
- dispatch scan: every row of § 3.1 (flag consumption, `--flag=value`, `--`, nested subcommand, name grammar, a non-UTF-8 argument after the name passed through byte for byte on Unix), PATH walk skipping empty and relative entries, non-executable file skipped, `plugin_child_env` keys exactly `GRIM_BIN` plus the two forwarded flags, `GRIM_BIN` free of a `\\?\` prefix on Windows;
- `mirror` table: each key's grammar accept/reject, precedence over frontmatter for licence, source, revision, deprecated;
- convert: per registry row the semantic round-trip invariant (§ 3.4), each `FieldType` coercion row, shadowed, unknown, `license` kept on all three kinds, body tokens with line numbers;
- mirror: `mirror.toml` validation rows (§ 4), path containment (`..`, absolute, backslash), snapshot version from fixture timestamps (base `0.0`, base from merged tag, two units on different bases, UTC), snapshot-shape predicate, tag-pattern capture rules, eligible tags limited to `--merged <candidate>`, watermark ignoring snapshot-shaped versions, reconciliation and selection sets for a given published list and window, licence expression evaluation (lax `GPL-3.0`, `OR`, `AND`, `WITH`, `+`, `LicenseRef`), bundle licence join, the golden licence corpus (§ 5.6) including the must-not-match variants and GPL text alone → unrecognised, marketplace component resolution per § 5.3 row (`skills` adds, root-source `skills` replaces, `agents` replaces, `strict` cases, `pluginRoot` bare names only, `sha` over `ref`, shorthand), exit-code table against `src/cli/exit_code.rs`, `--fail-on` table, git command builder pinning args and env, report ordering.

**Acceptance (pytest):**
- `test_plugin_dispatch.py`: a `grim-hello` script printing argv and `GRIM_BIN`, `GRIM_OFFLINE`, `GRIM_LOCKED`; built-in wins over `grim-install` on PATH; not found keeps exit 64 and clap's `unrecognized subcommand` text with no extra line; `--format json` before a found name → 64 with the § 3.1 message; exit-code passthrough (0, 3); relative PATH entry ignored; nested `grim config nosuch` not dispatched. Windows: a copy of `grim.exe` itself as `grim-probe.exe` (so no second Windows binary is needed), asserting `grim probe --version` passthrough and a usage-error exit code from the child, and a `grim-batty.bat` on PATH never runs (`grim batty` stays clap's 64); the `test_mirror_*` suites skip on Windows in the first version (plan PD-6).
- `test_publish_mirror_table.py`: annotations emitted, precedence over an upstream `license` field, 65 per invalid key, manifest without the table byte-identical (digest equal to a pre-change golden).
- `test_convert.py`: a Claude skill and agent fixture; moved and dropped rows; `grim build` of the output passes; installed output for `--client claude` equals the input's Claude frontmatter for moved keys.
- `test_mirror_monorepo.py` (Done 1): one repository, three skills, two rules, one agent → six artifacts; second `sync` all `unchanged`, exit 0; an idle tag-mode run reports `unchanged` rows (reconciliation), not an empty report.
- `test_mirror_bundles.py` (Done 2): declared bundle with exact-version absolute members; `grim add` of the bundle installs all members; a member removed upstream without a newer commit → `bundle-drift`; `check` on a fresh target reports the bundle `would-publish` without running grim on it.
- `test_mirror_kinds.py` (Done 3): rules and agents publish; hook, mcp, settings, commands inside a plugin → `skipped`, exit 0.
- `test_mirror_marketplace.py` (Done 4): marketplace with a relative plugin and a `git-subdir` plugin in a second local repository; partial bundle with skipped members; `npm` and `git@` entries `skipped` `source-unsupported`; a `strict: false` conflict `skipped` `plugin-conflict`; `agents` declared in `plugin.json` replaces `agents/`; a source with `..` → `unsafe-path`.
- `test_mirror_license.py`: detected (file, field, SPDX header), declared recorded as `declared`, missing → refused 65 under `--fail-on refused`, not allowed, conflict, declaration ignored beside detection, an unrecognised licence file refused and then accepted once `unit-licenses` declares the unit, repository-level `LICENSE` copied into a skill without one, a single-file agent's notice in its `<name>.README.md` companion.
- `test_mirror_versions.py`: tag mode at a tag, backfill bound, snapshot `0.0.<ts>`, forced snapshot over a tag `M.m.<ts>`, moved tag (highest published) → `upstream-moved`, a tag not merged into `ref` ignored, two tags one version → `version-collision`, prerelease tag ignored, a first `v0.0.1` tag published after a snapshot phase, `output-drift` warning after a converter-relevant mirror-file change.
- `test_mirror_dialect.py`: Claude skill published with `metadata.claude.*`, body-token warnings, `grim install` renders it for each client as the natively published equivalent does.
- `test_mirror_staging.py`: template-only and LFS units refused, symlink unit refused, `.claude-plugin` at a skill root dropped with a warning.
- `test_mirror_retire.py`: retire publishes `X.Y.(Z+1)` deprecated with replaced-by; `grim status --check` shows both; the old exact tag still installs; a unit deleted upstream is retired through `retire.units`.
- `test_mirror_cli.py`: exit codes 64, 78, 79, 81, 69 (a `GRIM_BIN` grim lacking the probed capability), `--fail-on` defaults (`sync` with a refused unit exits 0, `check` exits 65), an unreachable `repo` exits 69, check mode pushes nothing, the report carries `grim` and `grim_mirror`.
- `test_smoke.py:69-90` stays unchanged and green.

## 10. Rollout and migration

1. grim: dispatch (§ 3.1–3.2), `mirror` table (§ 3.3), `grim convert` (§ 3.4), each with its tests; `task rust:verify`.
2. `crates/grim-mirror` (workspace member, `publish = false`, `dist = false`, not in `dist-workspace.toml`; `deny.toml` admits the `spdx` crate's licence), `examples/mirror.toml`, README; test taskfile stages `test/bin/grim-mirror`; Windows wintest stages it too.
3. Acceptance suites (§ 9); `task verify`.
4. Docs and records in one pass after the last code fix: `commands.md`, `json-interface.md`, `publishing.md`, `artifacts.md`, env reference, new `mirroring.md` (page declarations; example GitHub Actions job: `cargo install --locked --git <grimoire> --rev <pinned commit or tag> grim-mirror`, a `concurrency` group per target, `check` on pull requests, `sync` on a schedule on the default branch; takedown runbook; the single-file licence-notice residual), `stability.md`, `changelog.md`; ADR amendments 1–5; catalog drift review.
5. Follow-up issues: repository split, curated list repository and job, announce support, command translation, indexer badge.

**Migration:** none. Every grim change is additive: no existing input changes output, exit or JSON. An older grim installs mirrored artifacts (unknown annotations are ignored) and refuses a `publish.toml` with a `mirror` table, which only `grim-mirror` writes and which probes for that capability first. Consumers (indexer, extension) need no change; consumer release order is not engaged because no curated field is added.
