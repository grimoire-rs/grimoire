# Updating This Skill

You loaded this file because you maintain the grim-usage package and
need to refresh it against a newer grim release.

## Re-Verification Protocol

1. Run `grim --version` and `grim <cmd> --help` for every command this
   package narrates (init, config, add, lock, install, update, status,
   context, fetch, describe, export, remove, uninstall, search, rate, schema,
   completions, tui, mcp, hook, build, release, publish, login, logout; for
   export, both `plugin` and `marketplace`). Diff
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
5. **Re-count the kinds.** The kind list is the single claim most likely to
   be silently wrong, because it appears in the `SKILL.md` frontmatter
   `description`, its opening paragraph, the `--kind` value list in
   [consume.md](consume.md), the batch kind order in
   [publish.md](publish.md), and the tier note below — and a new kind
   lands in all five. Ground it in `grim add --help`'s
   `[possible values: …]`, which is generated from the CLI enum.
6. **Re-check the hook gates.** This package documents hooks as experimental
   and gated: the feature flag plus per-workspace consent. The arming gates,
   the consent record and `grim hook allow` / `revoke` live in
   [consume.md](consume.md#hooks) and
   [troubleshooting.md](troubleshooting.md#a-hook-that-never-fires). Before
   widening any of it: `grim config get options.experimental.hooks`,
   `grim hook --help`, `grim hook allow --help`, `grim hook list`.
   `grim hook run` stays framed as launcher-invoked, never as a command a
   person types.
7. **Re-verify the marketplace client commands.** The use-a-marketplace guide
   stamps each vendor section "Verified <date>". Vendor CLIs move faster than
   grim, so on each minor re-check the command shapes and the private-repo
   table in [marketplaces.md](marketplaces.md) against the guide, and the
   `[marketplace]` name and `clients` rules against `grim export marketplace
   --help` and [configuration#marketplace-table][marketplace-table].
8. Bump the `compatibility` frontmatter in `SKILL.md` to the verified
   version line. The prose and the footer stay version-neutral — they
   track the release the package ships beside, so nothing else to bump.

## What Drifts, and How Fast

Tier-1 invariants (the **six** kinds — skill, rule, agent, mcp, hook,
bundle — reference syntax, exit-code classes, cascade-tag semantics) are
design commitments and rarely move, but "rarely" is not "never": this line
said *four* while the skill body said *five*, so re-derive the count from
`grim add --help` rather than trusting either.
Tier-2 content (flag names, command lifecycles, precedence details)
drifts with **every minor release** — re-verify it on each new grim
minor. Anything resembling a flag list belongs in `--help`, not here; if
a reference file has accreted one, delete it and link instead.

## Durable Search Terms

- `grimoire grim oci package manager skills rules agents`
- `github grimoire-rs grimoire releases changelog`
- `grim release cascade tags pin bundle`
- `grim exit codes sysexits`
- `grim hook list allow revoke arming gated options.experimental.hooks`
- `claude plugin marketplace add`, `extraKnownMarketplaces enabledPlugins`,
  `strictKnownMarketplaces`, `copilot plugin marketplace`,
  `codex plugin marketplace add --ref`, `qoder plugins marketplace`,
  `chat.plugins.marketplaces`, `cursor team marketplaces import from repo`
- `grim export marketplace untracked-destination`,
  `actions create-github-app-token`
- `indexer init`, `index.config.json sync layout anchored legacy revalidate`,
  `c/root.json`, `index-policy.json trustedBots`, `GRIM_INDEX_BOT_TOKEN`,
  `rating_host`
- `GRIM_TELEMETRY OTEL_EXPORTER_OTLP_ENDPOINT telemetry payload installation-id`

## Canonical Pages

- [Command reference][commands] — consume.md, publish.md, registries.md
- [Concepts][concepts] — consume.md (lock, bundles), registries.md
  (scopes, clients, offline)
- [Configuration][config] — consume.md (the two files), registries.md
  (env vars, precedence)
- [Publishing][publishing] — publish.md
- [Authentication][auth] — publish.md, troubleshooting.md
- [JSON interface][json-interface] — consume.md (report shapes),
  troubleshooting.md (error document, hook arming causes)
- [Use a plugin marketplace][use-a-marketplace] — marketplaces.md
- [Host a plugin marketplace][hosting-a-marketplace] — marketplaces.md and the
  `grim-authoring` skill's marketplace reference
- [Hand a team a plugin][team-plugin] — publish.md (export)
- [Upgrading][upgrading] — publish.md (plugin version, Qoder, compressed
  layers), marketplaces.md (the 0.15.0 manifest requirement)
- [Hosting an index][hosting-an-index], [Ratings][ratings],
  [Self-hosted GitLab][self-hosted-gitlab] and the [own-index
  tutorial][own-index] — registries.md (index sources)
- [Hooks][hooks] and [Hooks troubleshooting][hooks-troubleshooting] —
  consume.md, troubleshooting.md
- [Telemetry][telemetry] — telemetry.md
- Source of truth for the verbatim workflows and scripts the guides quote:
  the [e2e-marketplace][e2e-marketplace] example repository, and the [GitLab
  components][gitlab-components] for GitLab CI.

[commands]: https://grimoire.rs/commands.html
[concepts]: https://grimoire.rs/concepts.html
[config]: https://grimoire.rs/configuration.html
[publishing]: https://grimoire.rs/publishing.html
[auth]: https://grimoire.rs/authentication.html
[json-interface]: https://grimoire.rs/json-interface.html
[use-a-marketplace]: https://grimoire.rs/guides/use-a-marketplace.html
[hosting-a-marketplace]: https://grimoire.rs/guides/hosting-a-marketplace.html
[team-plugin]: https://grimoire.rs/guides/team-plugin.html
[upgrading]: https://grimoire.rs/upgrading.html
[hosting-an-index]: https://grimoire.rs/hosting-an-index.html
[ratings]: https://grimoire.rs/ratings.html
[self-hosted-gitlab]: https://grimoire.rs/self-hosted-gitlab.html
[own-index]: https://grimoire.rs/tutorials/own-index.html
[hooks]: https://grimoire.rs/hooks.html
[hooks-troubleshooting]: https://grimoire.rs/hooks-troubleshooting.html
[telemetry]: https://grimoire.rs/telemetry.html
[marketplace-table]: https://grimoire.rs/configuration.html#marketplace-table
[e2e-marketplace]: https://github.com/grimoire-rs/e2e-marketplace
[gitlab-components]: https://gitlab.com/grimoire-rs/components
