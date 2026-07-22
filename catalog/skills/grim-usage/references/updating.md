# Updating This Skill

You loaded this file because you maintain the grim-usage package and
need to refresh it against a newer grim release.

## Re-Verification Protocol

1. Run `grim --version` and `grim <cmd> --help` for every command this
   package narrates (init, config, add, lock, install, update, status,
   context, fetch, describe, remove, uninstall, search, schema,
   completions, tui, mcp, build, release, publish, login, logout). Diff
   the help output against what the reference files claim. Run `grim
   --help` too — the global flag set is narrated in `SKILL.md`.
2. Re-read the docs pages each reference file distills (links below) and
   diff against the file's claims — especially lifecycle semantics
   (pruning, effective declarations, integrity gates, deprecation
   warnings) and precedence chains (registry, clients).
3. Re-check the exit-code table in
   [troubleshooting.md](troubleshooting.md) against the docs' command
   reference — codes are a stable contract but new codes can appear.
4. Re-read the [JSON interface][json-interface] page and diff the report
   shapes this package names — the enveloped-report list and the item
   fields in [consume.md](consume.md), and the error-document `reason` /
   `retryable` / `forceable` set in
   [troubleshooting.md](troubleshooting.md). Fields and reasons are
   additive, so a new one is a gap here, never a contradiction.
5. Bump the `compatibility` frontmatter in `SKILL.md` to the verified
   version line. The prose and the footer stay version-neutral — they
   track the release the package ships beside, so nothing else to bump.

## What Drifts, and How Fast

Tier-1 invariants (the four kinds, reference syntax, exit-code classes,
cascade-tag semantics) are design commitments — they rarely move.
Tier-2 content (flag names, command lifecycles, precedence details)
drifts with **every minor release** — re-verify it on each new grim
minor. Anything resembling a flag list belongs in `--help`, not here; if
a reference file has accreted one, delete it and link instead.

## Durable Search Terms

- `grimoire grim oci package manager skills rules agents`
- `github grimoire-rs grimoire releases changelog`
- `grim release cascade tags pin bundle`
- `grim exit codes sysexits`

## Canonical Pages

- [Command reference][commands] — consume.md, publish.md, registries.md
- [Concepts][concepts] — consume.md (lock, bundles), registries.md
  (scopes, clients, offline)
- [Configuration][config] — consume.md (the two files), registries.md
  (env vars, precedence)
- [Publishing][publishing] — publish.md
- [Authentication][auth] — publish.md, troubleshooting.md
- [JSON interface][json-interface] — consume.md (report shapes),
  troubleshooting.md (error document)

[commands]: https://grimoire.rs/commands.html
[concepts]: https://grimoire.rs/concepts.html
[config]: https://grimoire.rs/configuration.html
[publishing]: https://grimoire.rs/publishing.html
[auth]: https://grimoire.rs/authentication.html
[json-interface]: https://grimoire.rs/json-interface.html
