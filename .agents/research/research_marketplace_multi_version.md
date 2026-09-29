# Research: several versions of one plugin in a harness marketplace

## Metadata
Date: 2026-09-28 · Lane: neutral fact-check, web docs + Codex source · Extends (does not redo): `research_marketplace_hosting_prior_art.md`, `research_marketplace_multi_harness_root.md`, `research_url_marketplace_trust.md`.
Sources (all accessed 2026-09-28):
- [C1] https://code.claude.com/docs/en/plugins/host-marketplace · [C2] https://code.claude.com/docs/en/plugins/marketplace-reference · [C3] https://code.claude.com/docs/en/plugins/cli-reference · [C4] https://code.claude.com/docs/en/plugins/loading · [C5] https://code.claude.com/docs/en/plugins/dependencies
- [G1] https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference · [G2] https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-finding-installing · [G3] https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/plugins-marketplace
- [X] Codex source, `github.com/openai/codex` shallow clone at `33a0f766a647` (2026-09-28); paths under `codex-rs/`. [X-doc] https://developers.openai.com/codex/plugins/build
- [F] https://docs.factory.ai/cli/configuration/plugins · [U] https://cursor.com/docs/reference/plugins, https://cursor.com/docs/plugins · [J] https://junie.jetbrains.com/docs/junie-cli-extensions.html
Caveat: WebFetch returns a small-model summary, not raw text. Claude pages were read at length; Copilot/Droid/Cursor/Junie quotes are summary-level. "Not stated" = absent from the summary, not proven absent upstream. Codex rows are read from source.

## Summary table
| | Claude Code | Copilot CLI | Codex CLI | Droid |
|---|---|---|---|---|
| Q1 same `name` twice in one file | `validate` error `Duplicate plugin name "x" found in marketplace` [C2]; runtime behaviour not stated | not stated (only MCP-server-name collisions: last loaded wins, warning) [G1] | no error: listing dedups on `name@marketplace`, first entry wins [X `manager.rs:2401-2445`]; install takes first matching entry [X `marketplace.rs:247-272`] | not stated [F] |
| Q2 install version selector | none. Form is `<plugin>[@marketplace]` [C3] | none documented; forms `plugin@marketplace`, `OWNER/REPO[:PATH]`, git URL, local path [G1] | none. `codex plugin add PLUGIN[@MARKETPLACE]` / `--marketplace` [X `cli/src/plugin_cmd.rs:60-100,773-806`] | none. `<plugin@marketplace>`; a pinned marketplace is part of the id (see Q5) [F] |
| Q3 per-entry immutable pin | yes: `github`/`url`/`git-subdir` take `ref`+`sha`; `npm` takes `version` [C2] | yes: `github`/`url` object with `ref`, `sha` (40-char), `path` [G1] | yes: `url`/`git-subdir` with `ref`, `sha`; `npm` with `version`, `registry` [X `marketplace.rs:596-645`; X-doc] | yes: `url`/`git-subdir` with `ref` or 40-char `sha` [F] |
| Q5 marketplace pin | `owner/repo#ref` or `@ref`, branch or tag only [C3, C2] | `owner/repo#ref` [G1]; SHA not stated | `--ref REF` or `owner/repo@ref`; full SHA accepted [X `marketplace_cmd.rs:63-72`, `marketplace_upgrade/git.rs:16-20`] | `#<ref>` or `@<40-char-sha>` [F] |

## Q1 duplicate names
- Claude: `claude plugin validate` reports an Error for two entries sharing a `name` [C2 validation table]. No doc says what install/list does with such a file if validate is skipped. One marketplace `name` per user; "a user can't have two marketplaces with the same name registered at once" [C2]. Across marketplaces, same plugin name is allowed: "When two marketplaces offer the same name, use the qualified form" `name@marketplace` [C3].
- Claude states the model outright: "One marketplace serves one version of each plugin at a time" and "Claude Code has no release-channel concept" [C1 "Hold users on one version", "Run release channels"].
- Codex: no error, silent first-wins. `list_marketplaces_for_config_with_states` keeps a `seen_plugin_keys` set of `name@marketplace` and drops later entries; comment: "duplicate plugin entries ... intentionally resolve to the first discovered source" [X manager.rs:2401-2445]. `find_marketplace_plugin` iterates entries and returns the first with a matching name that resolves [X marketplace.rs:247-272]. A test covers the two-marketplace-files case (`manager_tests.rs:5549`); the in-one-file path goes through the same key set. If two marketplace roots carry the same marketplace name, install errors `matched multiple marketplace roots` [X plugin_cmd.rs:890-926].
- Copilot: the only documented same-name rule is for MCP servers (last loaded wins, warning naming earlier definers); agents/skills first-found-wins, project over plugin [G1]. Nothing on marketplace entries. Cursor: checklist says `name` must be "unique, lowercase, kebab-case"; no collision behaviour [U]. Droid, Junie: not stated [F, J].

## Q2 version selector on install
- Claude: `claude plugin install <plugin> [--scope --config -y --accept-command --json]`; no `--version`, no third `@` segment [C3]. `plugin update` has no version flag ("latest version its marketplace offers") [C3].
- Codex: clap args are `plugin` (`PLUGIN[@MARKETPLACE]`), `--marketplace/-m`, `--json` only; `PluginId::parse` yields plugin + marketplace [X plugin_cmd.rs:85-102,773-806]. Store keeps `<plugin>/<version>/` dirs but tracks one `active_plugin_version` per plugin id [X store.rs:114-154].
- Copilot: "does not explicitly describe `@version` syntax for the install command"; "no syntax for pinning to specific versions or Git references" on the install page [G1, G2].
- Droid: `droid plugin install <plugin@marketplace> --scope`; "Droid splits on the first `@` after the first character" [F]. Version is the installed commit hash; `plugin.json` version is "release metadata" [F].
- Cursor/Junie: nothing on choosing a version at install [U, J]. Junie mentions `--use-version` only as a Junie build channel pin (search snippet, not the plugin install path); unverified.

## Q3 per-entry pin, distinct names (`foo` floating, `foo-1` pinned)
- Claude: `ref` = branch or tag (default: default branch); `sha` = 40-char lowercase; with both, `sha` is checked out; survives deleted `ref` on GitHub/GitLab/Bitbucket if commit reachable; not on CodeCommit-like servers [C2]. Entry `name` is the install id and "can't contain spaces"; it may differ from the plugin's own `plugin.json` `name`, but a mismatch makes install-by-manifest-name fail [C2, create-marketplace page]. Distinct entry names for one repo are therefore a schema-legal pattern; docs do not show it. Cache path uses the entry name: `cache/<marketplace>/<plugin>/<version>/` [C4].
- Version detection with a pin: `plugin.json` version, then entry `version`, then 12-char commit SHA [C4]. A pinned `sha` gives a stable computed version, so a pinned entry never sees an update until the entry changes.
- Codex: `Git { url, path, ref_name, sha }` from `url` and `git-subdir` sources; empty selectors normalised away [X marketplace.rs:596-632]. Local `path` entries have no pin.
- Copilot: source object `{source: github|url, repo, ref, sha, path}`; `sha` "pins to exact commits for reproducible installs" [G1]. Droid: same for `url`/`git-subdir` [F]. Cursor: docs list `source` and `version`, no `ref`/`sha` [U].
- No harness doc describes the `foo`/`foo-1` naming as a pattern. Claude adds an in-repo convention for tags: `<plugin>--v<version>` tags, `claude plugin tag`, used only by dependency version ranges [C5]; a dependency range `~2.1.0` "installs at the highest git tag that satisfies" it, on git-backed sources [C5].

## Q4 release channels
- Claude, exactly [C1 "Run release channels"]: two marketplaces with different `name` values (`stable-tools`, `latest-tools`), each holding `{ "name": "code-formatter", "source": { "source": "github", "repo": "your-org/code-formatter", "ref": "stable" | "latest" } }`. The user adds the marketplace for their channel. Give the two refs different `plugin.json` versions or omit `version` so the SHA differs: "Updates are detected by comparing versions, so a ref that moves without a version change leaves users on the cached copy." An admin can assign channels via `extraKnownMarketplaces`. Note: both channels use one plugin id `code-formatter@<marketplace>`; the qualified form disambiguates when both are added [C3].
- Second Claude mechanism: add one marketplace repo at a branch/tag, `your-org/your-marketplace#stable` [C1].
- Droid: pin-in-name makes a channel/version part of the marketplace id, `plugins@v1.2.0` (see Q5) [F]. Copilot, Codex, Cursor, Junie: no release-channel text found. Codex `--ref` on the marketplace is the mechanism that exists; Cursor Team Marketplaces track one branch with optional auto-refresh [U].

## Q5 marketplace pin and update
- Claude: `#ref`/`@ref` = branch or tag only [C2 field table]. `claude plugin marketplace update` : "A marketplace added with a branch or tag `ref` updates to the latest commit of that ref, not the repository's default branch" [C3]. So a branch pin follows its branch; a tag pin re-resolves the tag. No marketplace `sha` field documented. Background auto-update off by default for third-party marketplaces [C4].
- Codex: `--ref` stored as `ref_name`. Upgrade (`codex plugin marketplace upgrade [NAME]`, git marketplaces only) runs `git ls-remote <src> <ref>` and re-clones if the revision differs; a full 40-hex `ref` short-circuits (returned as the revision, no network), so a SHA-pinned marketplace upgrades to nothing [X marketplace_upgrade/git.rs:9-48, marketplace_upgrade.rs:234-262]. `--sparse` limits checkout paths.
- Droid: `#<ref>` follows a branch or tag; `@<sha>` pins a commit; "Updating the plugin clones the external source again at its configured `ref` or `sha`, or its default branch" [F]. Several pins of one repo register side by side under distinct names.
- Copilot: `marketplace add owner/repo#ref` accepted; `marketplace update [NAME]` (alias `refresh`) refreshes catalogs; `plugin update NAME [--all]` [G1]. Whether update honours the ref is not stated.
- Cursor: no ref pin documented; GitHub imports "track" a branch, manual Refresh or auto-refresh on push [U]. Junie: marketplace from git repo, local dir or HTTPS URL; ref pin not stated in the summary [J].

## negative:
- No harness documents an install-time version selector (`plugin@marketplace@1.2.0`, `--version`).
- No harness documents two same-named entries as a supported way to offer versions. Claude rejects it in `validate`; Codex silently keeps the first; Copilot, Droid, Cursor, Junie say nothing.
- Claude marketplace pin: branch or tag only, no `sha`.
- Copilot: no doc on update-vs-pin behaviour; no plugin-entry duplicate rule.
- Cursor plugin entries: `ref`/`sha` not documented.

## leads:
- Claude schema permits entry `name` != `plugin.json` `name`; behaviour of two differently-named entries pointing at one repo with different `sha` is untested here (prior art cache key is entry name + version).
- Droid's `plugins@v1.2.0` marketplace naming (pin appended to registered name) is the only documented multi-version coexistence mechanism found; re-read the raw page before relying on it.
- Codex `git_remote_revision` accepts a 40-hex `--ref` without ls-remote; behaviour with abbreviated SHAs or tags-that-look-like-hex not tested.
- Copilot CLI is closed source; `#ref` semantics on `update` would need a run against a real repo.
- Junie extensions page returned only a topic list from the fetcher; retry with the raw page or `.junie-extension/marketplace.json` examples.
