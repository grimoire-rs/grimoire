# Worker briefs for /prepare-release

Fill in `<LAST>` (last tag), `<NEXT>` (next version), `<LEVEL>`, `<REPO>`
(absolute checkout path) and `<INVENTORY>` (absolute scratch path outside the
repository). Each brief is the worker's whole context, so send it complete.

## Inventory (read-only)

```text
Repo: <REPO> (Grimoire, binary `grim`). Run everything from there. READ-ONLY on
the repo: no edits, no commits, no pushes. Your one output file is <INVENTORY>.

Goal: we are preparing the next <LEVEL> release (<NEXT>). The last release tag
is <LAST>. Build the release inventory: every change in <LAST>..HEAD that a user
of grim would notice, grouped by outcome, with the doc page that documents it.
A docs worker and a changelog writer consume the file and will not re-read the
commits, so it must carry the facts.

How:
1. `git log --reverse --format='%h %s%n%b---' <LAST>..HEAD`. Skip chore, test,
   ci and refactor commits unless the body shows a user-visible effect.
2. Read the body of each remaining commit, and its diff wherever the subject
   does not say what a user can now do. Do not guess.
3. Group related commits into one item.
4. For every item, grep docs/src/content/docs/ for the page and `{#anchor}` that
   documents it, and docs/src/content/docs/upgrading.md for an upgrade note when
   the change needs action or changes behaviour. Check the doc text against the
   final code (clap definitions under src/command/, exit codes, config keys,
   defaults), not against the commit message.
5. For every breaking marker (`!` subject, BREAKING CHANGE footer): say exactly
   what breaks, for whom, and whether upgrading.md gives the migration.

Write <INVENTORY> in this shape, most notable first within a section:

## Features
### <outcome in user words>
- commits: <short shas>
- what: <2-4 sentences: the problem, what the user can do now, exact names>
- example: <one real command or config snippet, from docs, tests or help text>
- docs: <path#anchor> (verified matches code: yes | no — <what differs>)
- upgrading: <path#anchor | not needed | MISSING>
- gap: <none | what is undocumented or wrong, precise enough to fix>
## Behaviour changes and breaking changes
## Fixes a user would notice
## Dropped (not user-visible)
<counts by type, plus any commit you were unsure about and why>

Final message: the file path, item counts per section, and every `gap:` and
`upgrading: MISSING` as one line each. Mark what you inferred but did not check
against code as "unverified". Nothing else.
```

## Catalog drift (edits `catalog/` only)

```text
Repo: <REPO> (Grimoire, binary `grim`). Run everything from there. Do not
commit, push or stash. Edit only under catalog/. Do not touch the `version` in
catalog/publish.toml, CHANGELOG.md, Cargo.toml, or anything under docs/.

Goal: we are preparing release <NEXT>. Every package under catalog/ ships to
users at that version, so its content must describe grim as it is at HEAD.
Review the catalog for drift against everything in <LAST>..HEAD and fix what is
stale.

Read first: catalog/README.md ("Content drift tiers", "Keeping content
honest"), each skill's references/updating.md, AGENTS.md.

Scope: catalog/skills/* (SKILL.md and references/), bundles/*.toml, mcp/*.toml,
hooks/* (hook.toml and payload), descriptions/*.md, publish.toml entries.

Method:
1. `git diff <LAST>..HEAD -- <each trigger path in "Keeping content honest">`
   and read the diffs. `git log --format='%h %s' <LAST>..HEAD` for the feature
   set. The diffs are the source of truth, not the subjects.
2. A drift review that already landed inside the range does not clear the
   commits after it.
3. `cargo build`, then check every claim against `./target/debug/grim --help`
   and `grim <cmd> --help`. On conflict `--help` wins. Check command and flag
   names, exit codes, config keys, env vars, artifact kinds, hook events and
   tiers, the MCP tool list (src/mcp/), and every linked docs anchor against a
   `{#anchor}` in docs/src/content/docs/.
4. Look for the reverse too: a shipped feature a reader of grim-usage or
   grim-authoring needs and no skill mentions.
5. Fix in place and respect the tiers: narrative plus a docs link, never a
   copied flag table. Leave correct text as it is.
6. Gate: `env -u OCX_GLOBAL task catalog:verify`.

Report: `fixed:` (file:line, what was stale, what it says now, evidence),
`verified clean:`, `undecided:` (not edited), `gate:` (command and result).
Mark anything not checked against the CLI or source as "unverified".
```

## Docs pass (edits `docs/` only)

```text
Repo: <REPO> (Grimoire, binary `grim`). Run everything from there. Do not
commit, push or stash. Edit only under docs/, and never
docs/src/content/docs/changelog.md. Do not touch Cargo.toml or CHANGELOG.md.

Goal: we are preparing release <NEXT>. <INVENTORY> lists every user-visible
change since <LAST> with its doc page. Make the docs match the code at HEAD.

Read first: <INVENTORY>, .claude/skills/docs/SKILL.md,
.claude/rules/docs-style.md, .claude/rules/docs-quality.md.

Do:
1. Fix every `gap:` that is not `none` and every `upgrading: MISSING`. Check
   each against the code before writing (src/command/ clap definitions, the
   built `./target/debug/grim <cmd> --help`), not against the inventory alone.
2. Every "docs: … verified matches code: no" item: correct the page.
3. A page that states the version a thing arrived in must name <NEXT>.
4. Cross-linked headings keep explicit `{#anchor}` ids. Never rename an
   existing anchor.
   Remove or reword nothing the inventory does not name: report it instead.
5. Gates, on the pages you changed: the checks under
   .claude/rules/docs-quality/checks/ in the order docs-quality.md gives, then
   `env -u OCX_GLOBAL task docs:check` (needs Node 24; if it is absent, say so
   and report the checks that did run).

Report: `fixed:` (file:line, the inventory item, what changed, evidence),
`left open:` (with the reason), `gate:` (each command and its result).
```
